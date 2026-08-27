//! The virtual HID device service, spoken directly (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
//!
//! Two framings, one inside the other, and they disagree about byte order: the
//! transport's length and request id are big-endian, while the service's payloads
//! are C++ objects handed to `memcpy` and so are native-endian with their padding
//! part of the format. The sizes are `sizeof` on those types rather than a reading
//! of their fields — see `docs/platform/macos/virtual-hid-device.md`.
//!
//! Written here rather than bound to the driver's own header-only C++ client,
//! which would bring `asio`, `nod`, `type_safe` and `spdlog` into the build and a
//! C++ toolchain with them.
//!
//! **What a frame means is not here.** Which kind is a health check, which flag
//! a status pair sets, when the device counts as ready, and where one frame ends
//! and the next begins are read in `engine`, off the bytes this carries over
//! ([`favjit_host::output::OutputHost`]); what is here is the socket, the framing
//! an outgoing message takes, and the numbers the service would reject any other
//! value of (ADR-0006).

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use favjit_host::output::{OutputHost, Took};
use favjit_host::{NoOutput, OutputReport};

const SOCKET: &str = "/Library/Application Support/org.pqrs/tmp/rootonly/\
                      karabiner_virtual_hid_device_service.sock";

/// The version the daemon on this machine expects.
///
/// Pinned, and the pin is the whole of the version handling: a mismatch is not
/// refused and not reported — the connection stays open, status responses keep
/// arriving, and no virtual keyboard ever appears. So there is no error to check,
/// only readiness that never comes.
const CLIENT_PROTOCOL_VERSION: u16 = 7;

const HEARTBEAT: u8 = 0;
const HEALTH_CHECK_RESPONSE: u8 = 3;
const REQUEST: u8 = 4;
const RESPONSE: u8 = 5;

const VIRTUAL_HID_KEYBOARD_INITIALIZE: u8 = 0;
const VIRTUAL_HID_POINTING_INITIALIZE: u8 = 3;
const POST_KEYBOARD_INPUT_REPORT: u8 = 6;
const POST_CONSUMER_INPUT_REPORT: u8 = 7;
const POST_APPLE_VENDOR_KEYBOARD_INPUT_REPORT: u8 = 8;
const POST_APPLE_VENDOR_TOP_CASE_INPUT_REPORT: u8 = 9;
const POST_GENERIC_DESKTOP_INPUT_REPORT: u8 = 10;
const POST_POINTING_INPUT_REPORT: u8 = 11;

/// The id byte the keyboard report opens with, and how many bytes it and the
/// pointing report take, id included.
///
/// Only the two reports [`Drop`] writes. What every other report of the device
/// is numbered and sized as belongs where the bytes are built, which is
/// `favjit-hid`: a copy here would be the same wire format written down twice,
/// and this one is out of reach of anything that could check it (ADR-0006).
const KEYBOARD_REPORT_ID: u8 = 1;
const KEYBOARD_REPORT_LEN: usize = 67;
const POINTING_REPORT_LEN: usize = 8;

/// `us`, from HID's country code table.
const COUNTRY_CODE_US: u64 = 33;

/// The identity favjit gives the virtual keyboard it sends through.
///
/// Karabiner's defaults, and deliberately so: the keyboard type the OS gives the
/// resulting device is what a chord-resolving consumer reads back, and it does not
/// follow the country code, so there is nothing to gain by differing here.
///
/// **The device enumerates with these**, which is what lets capture leave it
/// alone: whoever initialises the virtual keyboard sets its vendor and product,
/// so ignoring these two numbers is ignoring exactly the device this process
/// sends through. Matching on the numbers someone else's client happened to use
/// would be matching an observation instead.
pub const VIRTUAL_KEYBOARD_VENDOR: u16 = 0x16c0;
pub const VIRTUAL_KEYBOARD_PRODUCT: u16 = 0x27db;

/// The socket, and what both ends of it share.
///
/// Behind a lock and an atomic because two loops write on it: the run's own
/// posts a report as a key is converted, while the loop serving the connection
/// answers what the service asks and keeps it alive. The id is shared for the
/// same reason it exists at all — the transport pairs a request with its answer
/// by it, and two writers picking the same one would leave each exchange
/// answering the other's.
struct Connection {
    writer: Arc<Mutex<UnixStream>>,
    next_request_id: Arc<AtomicU64>,
}

impl Clone for Connection {
    fn clone(&self) -> Self {
        Self {
            writer: Arc::clone(&self.writer),
            next_request_id: Arc::clone(&self.next_request_id),
        }
    }
}

impl Connection {
    /// One request, addressed with an id nothing else on this connection will
    /// use.
    fn request(&self, what: u8, payload: &[u8]) -> i32 {
        let id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let mut body = Vec::with_capacity(3 + payload.len());
        body.extend_from_slice(&CLIENT_PROTOCOL_VERSION.to_ne_bytes());
        body.push(what);
        body.extend_from_slice(payload);

        let body_size = 1 + 8 + body.len();
        let mut out = Vec::with_capacity(4 + body_size);
        out.extend_from_slice(&(body_size as u32).to_be_bytes());
        out.push(REQUEST);
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&body);
        send(&self.writer, &out)
    }
}

/// The keyboard's own last report, written as this goes.
///
/// Its own type rather than a line in a `Drop` beside the pointer's, because
/// posting one report is one call and posting the other is another: a `Drop`
/// doing both would be a host sequencing two calls into the machine (ADR-0006),
/// and the order between them is nothing to decide.
///
/// The bytes are fixed rather than rendered: a report with nothing held is its id
/// followed by zeros the whole length of it, which this crate can state without
/// the table that renders an ordinary one.
struct NothingIsHeld(Connection);

impl Drop for NothingIsHeld {
    fn drop(&mut self) {
        let mut keyboard = [0u8; KEYBOARD_REPORT_LEN];
        keyboard[0] = KEYBOARD_REPORT_ID;
        self.0.request(POST_KEYBOARD_INPUT_REPORT, &keyboard);
    }
}

/// The pointing device's, for the same reason.
struct NothingIsMoving(Connection);

impl Drop for NothingIsMoving {
    fn drop(&mut self) {
        self.0
            .request(POST_POINTING_INPUT_REPORT, &[0u8; POINTING_REPORT_LEN]);
    }
}

/// The end of the connection a run posts its reports on.
///
/// The device outlives this process — it belongs to the driver — so a modifier
/// left down in the last report stays down for whatever runs next. Terminating
/// the device instead would take it away from any other client also using it, so
/// what goes out on the way here is a report with nothing in it, one per device.
pub struct VirtualDevice {
    connection: Connection,
    /// Held for their `Drop` and read by nothing, which is what makes the two
    /// reports go out in the order the fields are declared in.
    #[allow(dead_code)]
    keyboard: NothingIsHeld,
    #[allow(dead_code)]
    pointer: NothingIsMoving,
}

/// The end of it the loop serving the connection reads and answers on.
pub struct Serving {
    connection: Connection,
    reader: UnixStream,
    /// The run's own clock, and not a base of this loop's: what this end has to
    /// say goes onto the run's stream, and an instant from another base would
    /// put it in the wrong place among the events it is read beside.
    clock: crate::capture::Clock,
    events: std::sync::mpsc::Sender<crate::capture::Captured>,
}

/// Put one message on the socket, and say whether it went.
/// `0` for the write that went, and this machine's own `errno` otherwise.
///
/// `-1` where the failure carries no number of the platform's: a poisoned lock
/// and an error with no `errno` are this process's own state rather than
/// something the socket said, and `take_device` answers the same way for a call
/// it could not make at all.
fn send(writer: &Arc<Mutex<UnixStream>>, bytes: &[u8]) -> i32 {
    written(held(writer.lock()).write_all(bytes))
}

/// The stream, whether or not a thread died holding it.
///
/// Taken back rather than treated as a failure: what a panic while holding this
/// leaves behind is a socket that is still open and a run that is still
/// delivering, and refusing to write to it would take the output away over
/// something that already happened somewhere else.
fn held<'a>(
    locked: Result<
        std::sync::MutexGuard<'a, UnixStream>,
        std::sync::PoisonError<std::sync::MutexGuard<'a, UnixStream>>,
    >,
) -> std::sync::MutexGuard<'a, UnixStream> {
    match locked {
        Ok(held) => held,
        Err(died) => died.into_inner(),
    }
}

/// What one read of the socket said.
///
/// A count and not what the count means: `Ok(0)` is carried over as the no bytes
/// it is, because what no bytes at all says about the connection is read beside
/// the framing that asked for them (ADR-0006) — and a wait that came round with
/// nothing is a kind of its own, since the socket answers the same error for it
/// as for a read that was cut short.
fn took(read: std::io::Result<usize>) -> Took {
    match read {
        Ok(bytes) => Took::Bytes(bytes),
        Err(error) => match error.kind() {
            ErrorKind::Interrupted => Took::Interrupted,
            ErrorKind::WouldBlock | ErrorKind::TimedOut => Took::Nothing,
            _ => Took::Gone,
        },
    }
}

/// Which of the device's requests posts this report.
///
/// The page decides the request as well as the report id, and the two orders are
/// not the same — the top case's report is posted by the later request and
/// carries the lower id — so this pairing cannot be inferred from either number
/// (`docs/platform/macos/virtual-hid-device.md`).
///
/// Every report and no arm for one this crate has no request for: what would go
/// out on such a call is either nothing or somebody else's report, and both are
/// worse than the compiler refusing until this says where it goes
/// ([`OutputReport`] is closed for that).
fn request_for(report: OutputReport) -> u8 {
    match report {
        OutputReport::Keyboard => POST_KEYBOARD_INPUT_REPORT,
        OutputReport::Consumer => POST_CONSUMER_INPUT_REPORT,
        OutputReport::AppleVendorTopCase => POST_APPLE_VENDOR_TOP_CASE_INPUT_REPORT,
        OutputReport::AppleVendorKeyboard => POST_APPLE_VENDOR_KEYBOARD_INPUT_REPORT,
        OutputReport::GenericDesktop => POST_GENERIC_DESKTOP_INPUT_REPORT,
        OutputReport::Pointing => POST_POINTING_INPUT_REPORT,
    }
}

/// What the machine said about a write: `0` for the one that went, and its own
/// number otherwise.
fn written(write: std::io::Result<()>) -> i32 {
    match write {
        Ok(()) => 0,
        Err(error) => error.raw_os_error().unwrap_or(-1),
    }
}

/// Open the socket the service listens on.
fn connect() -> Result<UnixStream, NoOutput> {
    reached(UnixStream::connect(SOCKET))
}

/// The stream a call answered with, or that there is no service to reach.
///
/// Which failure it was is not carried: a socket that is not there and one that
/// refused the connection are the same thing to a run — no device to deliver
/// through — and the driver not being installed is the case that matters.
fn reached(answered: std::io::Result<UnixStream>) -> Result<UnixStream, NoOutput> {
    answered.map_err(|_| NoOutput::NoService)
}

/// Bound how long a write waits, so one to a service that has stopped taking
/// them does not hold the loop that made it.
fn bound_writes(stream: &UnixStream, write_timeout: Duration) -> bool {
    stream.set_write_timeout(Some(write_timeout)).is_ok()
}

/// Bound how long a read waits, so a wait that came back with nothing can be
/// told from a connection that has gone.
fn bound_reads(stream: &UnixStream, timeout: Duration) -> bool {
    stream.set_read_timeout(Some(timeout)).is_ok()
}

/// One whole frame, ready to be written: its length, then its body.
fn framed(message_type: u8, payload: &[u8]) -> Vec<u8> {
    let body_size = 1 + payload.len();
    let mut out = Vec::with_capacity(4 + body_size);
    out.extend_from_slice(&(body_size as u32).to_be_bytes());
    out.push(message_type);
    out.extend_from_slice(payload);
    out
}

/// The identity the virtual keyboard should enumerate with, as the request's own
/// payload.
///
/// Here rather than handed in: the numbers are what makes capture able to leave
/// this device alone, so this crate is the one place that has to agree with
/// itself about them (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
fn keyboard_parameters() -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&u64::from(VIRTUAL_KEYBOARD_VENDOR).to_ne_bytes());
    out.extend_from_slice(&u64::from(VIRTUAL_KEYBOARD_PRODUCT).to_ne_bytes());
    out.extend_from_slice(&COUNTRY_CODE_US.to_ne_bytes());
    out
}

/// The open socket, before anything has been asked of the service on it.
///
/// The clock and the stream it will be read on travel with it because they
/// belong to the end handed over rather than to the open: which requests go out,
/// in what order, and how long the answers are allowed are the run's (ADR-0006).
pub struct Reaching {
    stream: UnixStream,
    clock: crate::capture::Clock,
    events: std::sync::mpsc::Sender<crate::capture::Captured>,
}

/// Open the socket the service listens on, and hand it over as it is.
pub fn reach(
    clock: crate::capture::Clock,
    events: std::sync::mpsc::Sender<crate::capture::Captured>,
) -> Result<Reaching, NoOutput> {
    reaching(connect(), clock, events)
}

/// The connection an open answered with, or what the machine said about there
/// being no service to reach.
fn reaching(
    connected: Result<UnixStream, NoOutput>,
    clock: crate::capture::Clock,
    events: std::sync::mpsc::Sender<crate::capture::Captured>,
) -> Result<Reaching, NoOutput> {
    connected.map(|stream| Reaching {
        stream,
        clock,
        events,
    })
}

impl Reaching {
    /// Stop a write to it from blocking the loop that made it.
    pub fn bound_writes(&mut self, write_timeout: Duration) -> bool {
        bound_writes(&self.stream, write_timeout)
    }

    /// Take the second handle the reading end needs, and hand both over.
    pub fn both_ends(self) -> Option<(VirtualDevice, Serving)> {
        let reader = self.stream.try_clone();
        ends(reader, self)
    }
}

/// The two ends of the connection, where a second handle on it was had.
///
/// Two handles rather than the reading loop taking the lock the writes go
/// through: that loop sits in a read for as long as its bound allows, and a
/// report the run is posting meanwhile would wait behind it.
fn ends(
    cloned: std::io::Result<UnixStream>,
    reaching: Reaching,
) -> Option<(VirtualDevice, Serving)> {
    let Reaching {
        stream,
        clock,
        events,
    } = reaching;
    let connection = Connection {
        writer: Arc::new(Mutex::new(stream)),
        next_request_id: Arc::new(AtomicU64::new(1)),
    };
    cloned.ok().map(|reader| {
        (
            VirtualDevice {
                keyboard: NothingIsHeld(connection.clone()),
                pointer: NothingIsMoving(connection.clone()),
                connection: connection.clone(),
            },
            Serving {
                connection,
                reader,
                clock,
                events,
            },
        )
    })
}

impl OutputHost for Serving {
    fn now(&mut self) -> favjit_host::Instant {
        self.clock.now()
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> bool {
        bound_reads(&self.reader, timeout)
    }

    /// One read of the socket, however much of what was asked for it answers
    /// with.
    ///
    /// `read_exact` is not what this is. On a socket carrying a read timeout it
    /// answers the same error for "the wait came round with nothing" and "half
    /// of a frame arrived", the bytes it took are gone from the socket either
    /// way, and it stays inside for as long as bytes keep arriving at all —
    /// which is a call with no cadence while a frame trickles in.
    fn read_some(&mut self, into: &mut [u8]) -> Took {
        took(self.reader.read(into))
    }

    fn ask_for_the_keyboard(&mut self) -> bool {
        self.connection
            .request(VIRTUAL_HID_KEYBOARD_INITIALIZE, &keyboard_parameters())
            == 0
    }

    fn ask_for_the_pointer(&mut self) -> bool {
        self.connection
            .request(VIRTUAL_HID_POINTING_INITIALIZE, &[])
            == 0
    }

    fn beat(&mut self) -> i32 {
        send(&self.connection.writer, &framed(HEARTBEAT, &[]))
    }

    fn answer_health_check(&mut self) -> i32 {
        send(&self.connection.writer, &framed(HEALTH_CHECK_RESPONSE, &[]))
    }

    fn acknowledge(&mut self, id: &[u8]) -> i32 {
        send(&self.connection.writer, &framed(RESPONSE, id))
    }

    fn deliver(&mut self, event: favjit_host::EventKind) -> bool {
        self.events
            .send(crate::capture::Captured::Event(favjit_host::HostEvent {
                at: self.clock.now(),
                kind: event,
            }))
            .is_ok()
    }
}

impl VirtualDevice {
    /// Write these bytes to whichever of the device's own reports `report`
    /// names.
    ///
    /// The page decides the request as well as the report id, and the two orders
    /// are not the same — the top case's report is posted by the later request and
    /// carries the lower id — so this pairing cannot be inferred from either
    /// number (`docs/platform/macos/virtual-hid-device.md`).
    pub fn send(&mut self, report: OutputReport, bytes: &[u8]) -> i32 {
        self.connection.request(request_for(report), bytes)
    }
}

/// What a real socket answers one read with.
///
/// A pair of connected sockets rather than the service: what is under test is
/// the three answers a bounded read of a stream socket gives, which the service
/// cannot be asked to produce on demand — and a fake reader would be this file's
/// own idea of a socket, which is the thing that has to be checked. What is done
/// with those answers is `engine`'s and is driven there (ADR-0006).
#[cfg(test)]
mod tests {
    use super::*;

    /// The reading end of one connection, bounded the way a run bounds it, and
    /// the end that writes to it.
    fn connected(bound: Duration) -> (Serving, UnixStream) {
        let (reading, writing) = UnixStream::pair().expect("a pair of sockets");
        assert!(bound_reads(&reading, bound));
        let (events, _) = std::sync::mpsc::channel();
        let serving = Serving {
            connection: Connection {
                writer: Arc::new(Mutex::new(
                    reading.try_clone().expect("a second handle on it"),
                )),
                next_request_id: Arc::new(AtomicU64::new(1)),
            },
            reader: reading,
            clock: crate::capture::Clock::start(),
            events,
        };
        (serving, writing)
    }

    #[test]
    fn a_bound_that_comes_round_with_nothing_is_told_from_bytes() {
        // The ordinary case, and the one a cadence is built on: a service with
        // nothing to say leaves the socket quiet, and a read that reported that
        // as no bytes would be a working device read as one that has gone.
        let (mut serving, _writing) = connected(Duration::from_millis(10));

        assert_eq!(serving.read_some(&mut [0u8; 4]), Took::Nothing);
    }

    #[test]
    fn a_read_answers_with_what_is_there_rather_than_with_what_it_asked_for() {
        // A stream socket's own right, and the whole reason a frame is read as
        // one read repeated: a service that wrote a length and a body
        // separately is answered here in two pieces.
        let (mut serving, mut writing) = connected(Duration::from_millis(10));
        writing.write_all(&[1u8, 2]).expect("written");

        assert_eq!(serving.read_some(&mut [0u8; 4]), Took::Bytes(2));
    }

    #[test]
    fn what_arrived_is_what_lands_in_the_buffer() {
        let (mut serving, mut writing) = connected(Duration::from_millis(10));
        writing.write_all(&framed(HEARTBEAT, &[])).expect("written");
        let mut into = [0u8; 5];

        assert_eq!(serving.read_some(&mut into), Took::Bytes(5));
        assert_eq!(into, [0, 0, 0, 1, HEARTBEAT]);
    }

    #[test]
    fn a_far_end_that_has_closed_answers_with_no_bytes_at_all() {
        // No bytes and no wait to come round is the one thing a count says on
        // its own, and it is what a peer being gone looks like from here.
        let (mut serving, writing) = connected(Duration::from_millis(10));
        drop(writing);

        assert_eq!(serving.read_some(&mut [0u8; 4]), Took::Bytes(0));
    }
}
