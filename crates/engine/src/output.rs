//! What the device converted keystrokes go out through is asked for, and what
//! its answers mean (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
//!
//! Here rather than beside the socket because none of it touches the machine:
//! a frame is a length and then that many bytes, which of them is a health
//! check is a number, and what one is answered with is a decision. What the
//! host does — connect, read some bytes, ask for a device, beat — is the part no
//! test away from the platform can drive, and keeping this out of there is what
//! keeps that part small (ADR-0006).
//!
//! The service's own numbers are read here rather than named by the host, for
//! the reason a HID usage is: the host carries what arrived and this names it,
//! so a frame the service means one thing by cannot come to be named two ways.
//! Where one ends is such a number too, which is why the length in front of a
//! body is read here and not counted off behind the socket. What goes back out
//! is named for what it asks instead, because turning that into the service's
//! own framing is the argument conversion a host operation is allowed.

use core::time::Duration;

use favjit_host::output::{OutputHost, Took};
use favjit_host::NoOutput;

/// The kinds of frame this run reads, out of the service's own numbering
/// (`docs/platform/macos/virtual-hid-device.md`).
const HEALTH_CHECK: u8 = 2;
const REQUEST: u8 = 4;
const RESPONSE: u8 = 5;

/// How many bytes a frame's own length is spelled in, ahead of the body.
///
/// Big-endian, which the payload behind it is not: those are C++ objects handed
/// to `memcpy` and so are native-endian with their padding part of the format,
/// while the transport's length and request id are the other way round
/// (`docs/platform/macos/virtual-hid-device.md`).
const LENGTH: usize = 4;

/// What a run puts on that connection, and the ways the loop serving it comes
/// back.
///
/// Both live with the recording rather than here: they are what a reading of a
/// dropped connection is made of, and a reader of a trace has no output of its
/// own to ask (ADR-0009).
pub(crate) use crate::trace::{Lost, Served};

/// Where the id in a request sits inside a frame's body, and where the flag
/// pairs behind it start.
///
/// One fact rather than two: the transport pairs a request with its answer by
/// that id, so everything before the pairs is the request's own header.
const ID: core::ops::Range<usize> = 1..9;
const PAIRS: usize = 9;

/// What the service has said about itself so far.
///
/// Flags because that is all it sends: pairs of (kind, value) bytes, each
/// naming one thing it has or has not managed yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Status {
    driver_activated: bool,
    driver_connected: bool,
    version_mismatched: bool,
    keyboard_ready: bool,
    pointing_ready: bool,
}

impl Status {
    /// Whether both devices a run sends through are up.
    ///
    /// Both, because converted input goes out through either: a keyboard with
    /// no pointer beside it would drop every mouse report a source relays.
    pub(crate) fn ready(self) -> bool {
        self.keyboard_ready && self.pointing_ready
    }

    /// What a run that gave up waiting reports.
    ///
    /// The three things the service ever says about itself, since a wrong
    /// protocol version is silently ignored and a timeout is the only other
    /// signal it gives.
    pub(crate) fn why_not(self) -> NoOutput {
        NoOutput::NotReady {
            driver_activated: self.driver_activated,
            driver_connected: self.driver_connected,
            version_mismatched: self.version_mismatched,
        }
    }

    /// Take in what one frame said about it.
    ///
    /// A pair whose kind this run has no name for is passed over rather than
    /// ending the read: the service is free to say more about itself than this
    /// reads, and a frame dropped for one unknown pair would lose the
    /// readiness beside it.
    pub(crate) fn told(&mut self, body: &[u8]) {
        if !matches!(body.first(), Some(&REQUEST) | Some(&RESPONSE)) || body.len() < PAIRS {
            return;
        }
        for pair in body[PAIRS..].chunks(2) {
            let [kind, value] = pair else { break };
            let value = *value != 0;
            match kind {
                1 => self.driver_activated = value,
                2 => self.driver_connected = value,
                3 => self.version_mismatched = value,
                4 => self.keyboard_ready = value,
                5 => self.pointing_ready = value,
                _ => continue,
            }
        }
    }
}

/// What one frame the service sent asks for in reply.
///
/// A health check is answered with nothing but its own kind. A request expects
/// an answer even when there is nothing to say back, because the transport
/// pairs them by id and one left unanswered is one the service waits on. A
/// response is the end of an exchange this end started and asks for nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer<'a> {
    Nothing,
    HealthCheck,
    Acknowledge(&'a [u8]),
}

pub(crate) fn answer_to(body: &[u8]) -> Answer<'_> {
    match body.first() {
        Some(&HEALTH_CHECK) => Answer::HealthCheck,
        Some(&REQUEST) if body.len() >= PAIRS => Answer::Acknowledge(&body[ID]),
        _ => Answer::Nothing,
    }
}

/// Ask the service for the two devices a run sends through.
///
/// Before the loop below is turning and not after: readiness arrives as the
/// answer to one of these, and an answer nobody is reading yet is one the run
/// would wait out its whole bound for.
pub(crate) fn ask_for_the_devices(host: &mut dyn OutputHost) -> bool {
    host.ask_for_the_keyboard() && host.ask_for_the_pointer()
}

/// Read one frame and answer it, and say whether the connection is still worth
/// reading.
///
/// The one step both the wait for readiness and the loop below are built out
/// of, so that a health check arriving while a run is still waiting for its
/// devices is answered as readily as one arriving an hour later — and so that
/// what a frame said about the device is read the same way at either time.
pub(crate) fn one_frame(host: &mut dyn OutputHost, status: &mut Status) -> Option<Lost> {
    match read_frame(host) {
        Frame::Body(body) => {
            status.told(&body);
            match answer_to(&body) {
                Answer::Nothing => None,
                Answer::HealthCheck => {
                    let code = write(host, Served::HealthCheckAnswer, |host| {
                        host.answer_health_check()
                    });
                    (code != 0).then_some(Lost::HealthCheckAnswerWillNotGo)
                }
                // The id is copied out first: the borrow of `body` it is a slice
                // of cannot be held across a call taking the host, and the eight
                // bytes are cheaper than the frame they came in.
                Answer::Acknowledge(id) => {
                    let id = id.to_vec();
                    let code = write(host, Served::Acknowledgement, |host| host.acknowledge(&id));
                    (code != 0).then_some(Lost::AcknowledgementWillNotGo)
                }
            }
        }
        // Nothing arrived inside the wait, and nothing is what that asks for.
        // Not a beat: the service beats every few seconds itself, so a read
        // given a bound of the same length almost always comes back with one of
        // those instead — and a beat sent from here only when a read came back
        // empty is a beat that never goes while the service is talking.
        Frame::Quiet => None,
        Frame::Gone => Some(Lost::ServiceClosedIt),
        // Named apart from the service closing it, because it is this end that
        // is letting go and a reading that said otherwise would send whoever
        // reads it to the wrong machine.
        Frame::Torn => Some(Lost::FrameArrivedInPieces),
    }
}

/// One whole frame off the connection, or why none came.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Frame {
    /// One whole body, as the service framed it.
    Body(Vec<u8>),
    /// Nothing arrived inside the wait the read was given.
    Quiet,
    /// Part of a frame arrived inside the wait and the rest did not.
    ///
    /// Its own kind rather than either of the other two, because it is neither:
    /// the wait came round, but the bytes already taken cannot be put back, so
    /// where the next frame begins is no longer known. A connection reported as
    /// merely quiet would be read on from the middle of a frame, and a length
    /// taken out of somebody else's bytes is a read that waits for however many
    /// that number happened to say.
    Torn,
    /// Nothing more will arrive on it.
    Gone,
}

/// How much of what one fill asked for arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filled {
    /// All of it.
    Whole,
    /// None of it, because the bound the reads were given came round first.
    Nothing,
    /// Some of it, and then no more.
    Part,
    /// Nothing more will arrive on the connection.
    Gone,
}

/// Take exactly `into.len()` bytes, and say how far it got.
///
/// Reads until it has the count rather than asking once for it: a stream socket
/// is free to answer with fewer bytes than were wanted, so one read is not a
/// count and a fill that treated it as one would tear frames the service wrote
/// whole. The bound is the socket's own on each read and not on this, which is
/// what keeps a cadence: a frame trickling in a byte at a time comes back here
/// as one whole frame, and a socket that has gone quiet comes back at the first
/// bound that came round.
fn fill(host: &mut dyn OutputHost, into: &mut [u8]) -> Filled {
    let mut taken = 0;
    while taken < into.len() {
        match host.read_some(&mut into[taken..]) {
            // No bytes at all is the far end having closed, which is the whole
            // frame never arriving whether or not part of it did.
            Took::Bytes(0) => return Filled::Gone,
            Took::Bytes(more) => taken += more,
            Took::Interrupted => continue,
            Took::Nothing => {
                return match taken {
                    0 => Filled::Nothing,
                    _ => Filled::Part,
                }
            }
            // `Took` is `#[non_exhaustive]`, and a kind this crate does not yet
            // name is read as the connection going: the alternative is reading
            // on from a stream whose next byte is not a frame's beginning, and
            // that is the one answer nothing recovers from.
            _ => return Filled::Gone,
        }
    }
    Filled::Whole
}

/// One whole message off the connection, or why none came.
///
/// Two fills and no choice about it: the frame is length-prefixed, so how much
/// the second one wants is what the first one said.
///
/// A frame that came apart is not read on from. The bytes taken cannot be put
/// back, so the next length would come out of somebody else's bytes, and a
/// length like that is a read that waits for however many that number happened
/// to say — the service, meanwhile, closes a connection it has heard nothing on
/// (`docs/platform/macos/virtual-hid-device.md`). Letting go and saying so is
/// the answer the driver's own client gives it as well.
fn read_frame(host: &mut dyn OutputHost) -> Frame {
    let mut length = [0u8; LENGTH];
    let header = fill(host, &mut length);
    if !matches!(header, Filled::Whole) {
        return came_apart(header);
    }
    let mut body = vec![0u8; u32::from_be_bytes(length) as usize];
    if body.is_empty() {
        return Frame::Body(body);
    }
    match fill(host, &mut body) {
        Filled::Whole => Frame::Body(body),
        // A header taken and no body behind it: the same tear, whichever half of
        // the frame the wait fell in.
        Filled::Nothing => Frame::Torn,
        gave_up => came_apart(gave_up),
    }
}

/// What a fill that did not finish leaves the connection as.
fn came_apart(filled: Filled) -> Frame {
    match filled {
        Filled::Nothing => Frame::Quiet,
        Filled::Part => Frame::Torn,
        _ => Frame::Gone,
    }
}

/// Write one frame, and put it in the recording with what the machine said.
///
/// Both, and here rather than at each call site: a frame written without a
/// record of it is one a reading cannot tell from a frame nobody sent, and
/// telling those apart is the difference between a run that was holding the
/// connection up and one that only looked like it (ADR-0009).
fn write(
    host: &mut dyn OutputHost,
    frame: Served,
    call: impl FnOnce(&mut dyn OutputHost) -> i32,
) -> i32 {
    let code = call(host);
    host.deliver(favjit_host::EventKind::OutputFrame {
        frame: frame as u32,
        code,
    });
    code
}

/// Answer what the service says, and keep the connection alive, for as long as
/// it is there.
///
/// The beat is due on a clock of its own rather than whenever a read came back
/// empty. The two are not the same thing here: the service beats at this end
/// every few seconds as well, so a read given a bound of that length comes back
/// with a frame far more often than it comes back with nothing — and a
/// connection the service has heard nothing on for long enough is one it closes,
/// which reaches a person as the keyboard they are typing on going quiet.
///
/// Nor is it a tally of what went out. A health check answered and a request
/// acknowledged are both frames the service hears, so a run doing plenty of
/// answering needs no beat at all for as long as that lasts — but knowing that
/// means knowing what the service counts, and the beat is cheap where guessing
/// wrong costs the connection.
///
/// Returning means the connection is not being served any more, whichever way
/// it happened, and the machine is told so by the loop it handed over coming
/// back — which is what [`favjit_host::sink::SinkHost::run_output_alongside`]
/// turns this on, and what
/// [`favjit_host::sink::SinkInputHost::output_connected`] answers off
/// afterwards.
///
/// What arrives after the devices are up is read for the same flags and then
/// let go of: a run acts on the connection ending, which is the fact that
/// outlives any single frame.
pub(crate) fn serve(host: &mut dyn OutputHost, beat_every: Duration) {
    let lost = serving(host, beat_every);
    host.deliver(favjit_host::EventKind::OutputEnded { why: lost as u32 });
}

/// The loop itself, which comes back with the reason it did.
///
/// Split from [`serve`] so that every way out passes through one record of the
/// reason: a loop returning early from inside would be a way for the recording
/// to hold a connection that stopped being served and no reason for it, which is
/// the reading this record exists to make possible (ADR-0009).
fn serving(host: &mut dyn OutputHost, beat_every: Duration) -> Lost {
    if !host.set_read_timeout(beat_every) {
        return Lost::ReadsWillNotBound;
    }
    let mut status = Status::default();
    let mut beaten = host.now();
    loop {
        if let Some(lost) = one_frame(host, &mut status) {
            return lost;
        }
        let now = host.now();
        if now.nanos.saturating_sub(beaten.nanos) >= beat_every.as_nanos() as u64 {
            if write(host, Served::Beat, |host| host.beat()) != 0 {
                return Lost::BeatWillNotGo;
            }
            beaten = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection that answers each read with the next thing on this list.
    ///
    /// Answers and not bytes: what one read of a stream socket comes back with
    /// is the whole of what the framing above is built on, and a stand-in that
    /// only held bytes could not be asked what a socket that went quiet halfway
    /// through a frame does.
    struct Stream {
        reads: std::collections::VecDeque<Reply>,
    }

    /// What one read of it comes back with.
    enum Reply {
        /// These bytes, up to however many the read asked for, and the rest
        /// left on the stream for the read after it.
        These(Vec<u8>),
        /// The bound came round with nothing.
        Nothing,
        /// The read was cut short before it was answered.
        Interrupted,
        /// The socket said something nothing more will arrive after.
        Gone,
    }

    impl Stream {
        /// One that answers with these and then goes quiet, which is a service
        /// with nothing more to say rather than one that has gone.
        fn answering(reads: Vec<Reply>) -> Self {
            Self {
                reads: reads.into(),
            }
        }
    }

    impl OutputHost for Stream {
        fn now(&mut self) -> favjit_host::Instant {
            favjit_host::Instant::default()
        }

        fn set_read_timeout(&mut self, _timeout: Duration) -> bool {
            true
        }

        fn read_some(&mut self, into: &mut [u8]) -> Took {
            match self.reads.pop_front() {
                Some(Reply::These(mut bytes)) => {
                    let taken = bytes.len().min(into.len());
                    into[..taken].copy_from_slice(&bytes[..taken]);
                    let rest = bytes.split_off(taken);
                    if !rest.is_empty() {
                        self.reads.push_front(Reply::These(rest));
                    }
                    Took::Bytes(taken)
                }
                Some(Reply::Interrupted) => Took::Interrupted,
                Some(Reply::Gone) => Took::Gone,
                Some(Reply::Nothing) | None => Took::Nothing,
            }
        }

        fn ask_for_the_keyboard(&mut self) -> bool {
            true
        }

        fn ask_for_the_pointer(&mut self) -> bool {
            true
        }

        fn beat(&mut self) -> i32 {
            0
        }

        fn answer_health_check(&mut self) -> i32 {
            0
        }

        fn acknowledge(&mut self, _id: &[u8]) -> i32 {
            0
        }

        fn deliver(&mut self, _event: favjit_host::EventKind) -> bool {
            true
        }
    }

    /// One whole frame as the service writes it: its body's length, then the
    /// body.
    fn framed(body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(LENGTH + body.len());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn a_whole_frame_comes_back_as_its_body() {
        let frame = framed(&[HEALTH_CHECK]);
        let mut stream = Stream::answering(vec![Reply::These(frame)]);

        assert_eq!(read_frame(&mut stream), Frame::Body(vec![HEALTH_CHECK]));
    }

    #[test]
    fn a_frame_that_arrives_in_two_pieces_is_whole() {
        // The bound is on the wait and not on the frame, so a service that
        // writes a length and a body separately is not a service that tore one.
        let frame = framed(&[RESPONSE, 1, 2, 3]);
        let mut stream = Stream::answering(vec![
            Reply::These(frame[..LENGTH].to_vec()),
            Reply::These(frame[LENGTH..].to_vec()),
        ]);

        assert_eq!(
            read_frame(&mut stream),
            Frame::Body(vec![RESPONSE, 1, 2, 3])
        );
    }

    #[test]
    fn a_bound_that_comes_round_with_nothing_is_not_a_connection_that_has_gone() {
        // The ordinary case, and the one a cadence is built on: a service with
        // nothing to say leaves the socket quiet, and a run that read that as
        // the connection going would tear down a working device.
        let mut stream = Stream::answering(vec![Reply::Nothing]);

        assert_eq!(read_frame(&mut stream), Frame::Quiet);
    }

    #[test]
    fn a_read_that_was_cut_short_is_made_again() {
        // Nothing came off the stream and no bound came round, so the frame
        // behind it is still there to be read — and a run that counted it as a
        // quiet cadence would beat on a signal instead of on a clock.
        let mut stream = Stream::answering(vec![
            Reply::Interrupted,
            Reply::These(framed(&[HEALTH_CHECK])),
        ]);

        assert_eq!(read_frame(&mut stream), Frame::Body(vec![HEALTH_CHECK]));
    }

    #[test]
    fn a_length_with_no_body_behind_it_is_a_frame_that_came_apart() {
        // Not a wait that came round with nothing, which is what the same
        // socket answers for both: those four bytes are gone, so reading on
        // would take the next frame's own length for a body — and every length
        // after that comes out of somebody else's bytes.
        let frame = framed(&[HEALTH_CHECK]);
        let mut stream = Stream::answering(vec![Reply::These(frame[..LENGTH].to_vec())]);

        assert_eq!(read_frame(&mut stream), Frame::Torn);
    }

    #[test]
    fn part_of_a_length_is_a_frame_that_came_apart_as_well() {
        let mut stream = Stream::answering(vec![Reply::These(vec![0, 0])]);

        assert_eq!(read_frame(&mut stream), Frame::Torn);
    }

    #[test]
    fn a_read_that_took_nothing_at_all_is_a_far_end_that_has_closed() {
        // The one answer a count carries rather than a kind of its own: a
        // socket with a peer still there either has bytes or has a bound to
        // wait out, so no bytes and no wait is the peer being gone.
        let mut stream = Stream::answering(vec![Reply::These(Vec::new())]);

        assert_eq!(read_frame(&mut stream), Frame::Gone);
    }

    #[test]
    fn a_socket_that_answered_with_a_failure_is_a_connection_that_has_gone() {
        let mut stream = Stream::answering(vec![Reply::Gone]);

        assert_eq!(read_frame(&mut stream), Frame::Gone);
    }

    #[test]
    fn a_frame_with_no_body_at_all_is_a_body_of_no_bytes() {
        // Read as a whole frame and not as a tear: the length said zero, so
        // there is nothing behind it to wait for and the stream is left where
        // the next frame begins.
        let mut stream = Stream::answering(vec![Reply::These(framed(&[]))]);

        assert_eq!(read_frame(&mut stream), Frame::Body(Vec::new()));
    }

    /// One frame's body as the service sends it: a kind, an id, and pairs.
    fn body(kind: u8, pairs: &[(u8, u8)]) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend_from_slice(&7u64.to_be_bytes());
        for (kind, value) in pairs {
            out.push(*kind);
            out.push(*value);
        }
        out
    }

    #[test]
    fn a_health_check_is_answered() {
        // One left unanswered is a connection the service tears down, and a
        // torn-down connection is a keyboard whose keystrokes go nowhere.
        assert_eq!(answer_to(&[HEALTH_CHECK]), Answer::HealthCheck);
    }

    #[test]
    fn a_request_is_answered_with_the_id_it_carried() {
        // The transport pairs the two by that id, so an answer carrying another
        // is an answer to a question the service did not ask.
        let asked = body(REQUEST, &[(4, 1)]);

        assert_eq!(answer_to(&asked), Answer::Acknowledge(&asked[ID]));
    }

    #[test]
    fn a_response_is_the_end_of_an_exchange_rather_than_the_start_of_one() {
        assert_eq!(answer_to(&body(RESPONSE, &[(4, 1)])), Answer::Nothing);
    }

    #[test]
    fn a_request_too_short_to_carry_an_id_is_answered_with_nothing() {
        // Answering it would address the answer with whatever followed the
        // kind byte, which is an answer to a question nobody asked.
        assert_eq!(answer_to(&[REQUEST, 1, 2]), Answer::Nothing);
    }

    #[test]
    fn both_devices_have_to_be_up_before_a_run_sends_through_either() {
        // A keyboard with no pointer beside it would drop every mouse report a
        // source relays, which is the half of a relay nobody notices until
        // they move the mouse.
        let mut status = Status::default();
        status.told(&body(RESPONSE, &[(4, 1)]));
        assert!(!status.ready(), "a keyboard alone is not the output");

        status.told(&body(RESPONSE, &[(5, 1)]));
        assert!(status.ready());
    }

    #[test]
    fn what_it_last_said_about_itself_is_what_a_run_that_gave_up_reports() {
        // The three things it ever says: a person with the package missing and
        // a person whose driver is blocked need different things done about it.
        let mut status = Status::default();
        status.told(&body(RESPONSE, &[(1, 1), (2, 0), (3, 1)]));

        assert_eq!(
            status.why_not(),
            NoOutput::NotReady {
                driver_activated: true,
                driver_connected: false,
                version_mismatched: true,
            }
        );
    }

    #[test]
    fn a_pair_this_run_has_no_name_for_leaves_the_ones_beside_it_read() {
        // The service is free to say more about itself than this reads, and a
        // frame dropped for one unknown pair would lose the readiness in it.
        let mut status = Status::default();
        status.told(&body(RESPONSE, &[(9, 1), (4, 1), (5, 1)]));

        assert!(status.ready());
    }

    #[test]
    fn a_frame_that_is_not_a_status_frame_says_nothing_about_readiness() {
        let mut status = Status::default();
        status.told(&[HEALTH_CHECK]);

        assert_eq!(status, Status::default());
    }

    #[test]
    fn a_flag_the_service_takes_back_is_taken_back_here() {
        // A driver that deactivates says so on the same connection, and a run
        // that only ever set flags would report a device that is gone as up.
        let mut status = Status::default();
        status.told(&body(RESPONSE, &[(4, 1), (5, 1)]));
        status.told(&body(RESPONSE, &[(4, 0)]));

        assert!(!status.ready());
    }
}
