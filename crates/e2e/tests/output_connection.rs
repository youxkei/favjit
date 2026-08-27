//! What keeps the connection to the output device open once both devices are up.
//!
//! The service on the other end of it beats every few seconds and closes a
//! connection it has heard nothing on for five
//! (`docs/platform/macos/virtual-hid-device.md`), and a closed connection is a run
//! whose keystrokes go nowhere: the keyboards are handed back and the process
//! ends. So these are ADR-0008's rule about suppression outliving the ability to
//! process input, one step before the wedge — the keyboards do come back, and what
//! the person at the keyboard sees is favjit going away every few seconds.

use core::time::Duration;

use favjit_engine::sink::{self, Ending, InputConfig, Request};
use favjit_engine::trace::{Lost, Record, Served, Trace};
use favjit_engine::Layout;
use favjit_host::output::Took;
use favjit_host_sim::{OutputCall, ServiceEnded, SimHost};

/// Long enough that the service would have closed the connection several times
/// over on a run that never said anything.
const A_WHILE: Duration = Duration::from_secs(60);

/// What a machine says about a write to something that has gone.
const BROKEN_PIPE: i32 = 32;

fn converting() -> Request {
    Request::Injecting { listen: false }
}

fn run(mac: &mut SimHost) -> Ending {
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        None,
    )
    .0
}

/// The same, keeping a recording of it.
fn recorded(mac: &mut SimHost) -> Vec<u8> {
    let mut bytes = vec![0u8; 64 * 1024];
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        Some(&mut bytes),
    );
    bytes
}

fn records(bytes: &[u8]) -> Vec<Record> {
    Trace::read(bytes)
        .records()
        .map(|(_, record)| record)
        .collect()
}

/// Which way the loop serving the connection came back, as the recording has it.
fn how_it_ended(records: &[Record]) -> Option<Lost> {
    records.iter().find_map(|record| match record {
        Record::OutputEnded { why } => Lost::from_number(*why),
        _ => None,
    })
}

/// The heartbeat written out as the service reads one: a body of its type byte
/// alone, with that length big-endian in front of it.
const A_HEARTBEAT: [u8; 5] = [0, 0, 0, 1, 0];

fn beats(mac: &SimHost) -> usize {
    mac.output_calls()
        .iter()
        .filter(|call| matches!(call, OutputCall::Frame(bytes) if bytes == &A_HEARTBEAT))
        .count()
}

/// How many connections the run opened, counted by the bound it puts on the
/// writing end of each before taking it.
fn connections(mac: &SimHost) -> usize {
    mac.output_calls()
        .iter()
        .filter(|call| matches!(call, OutputCall::BoundWrites))
        .count()
}

#[test]
fn a_connection_that_went_is_opened_again_before_the_run_gives_up_on_it() {
    // The service closes this one in the ordinary course of things — on every
    // wake from sleep — so a run that took the first close for a failure would
    // hand the keyboards back every time, and the keys typed in each window
    // would reach applications as the keys they were physically typed on
    // (ADR-0008). What bounds the holding is the second close: this script's
    // output never comes back, so the attempt buys nothing and the run ends.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .with_output_lost_after(0);

    let ending = run(&mut mac);

    assert_eq!(
        connections(&mac),
        2,
        "the one the run came up on and the one it opened when that went"
    );
    assert_eq!(
        ending,
        Ending::OutputGone,
        "an attempt that bought nothing is the end of the run"
    );
}

#[test]
fn the_frames_this_end_writes_are_composed_where_the_arriving_ones_are_read() {
    // The transport's framing is one fact, and a run that read a length off the
    // connection while a host counted one onto the front of every write would
    // hold it in two places — where the two can come to disagree, and what says
    // so is a service that stops answering rather than anything either end
    // reports (ADR-0006).
    let mut mac = SimHost::new().whose_output_serves_for(A_WHILE);

    run(&mut mac);

    let beats: Vec<Vec<u8>> = mac
        .output_calls()
        .iter()
        .filter_map(|call| match call {
            OutputCall::Frame(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .collect();
    assert!(
        beats.iter().any(|bytes| bytes == &A_HEARTBEAT),
        "a heartbeat is a body of one byte, its length in front of it big-endian, and the type \
         the service numbers a heartbeat with: {beats:?}"
    );
}

#[test]
fn a_service_that_keeps_talking_still_hears_from_this_end() {
    // A beat sent only when a read came back empty is a beat that never goes:
    // the service beats every three seconds itself and a read is given the same
    // bound, so what comes back is almost always one of those. Nothing else
    // holds the connection up — the reports of somebody typing do, which makes a
    // connection that lasts exactly as long as the typing.
    let mut mac = SimHost::new().whose_output_serves_for(A_WHILE);

    run(&mut mac);

    assert_eq!(
        mac.output_ended(),
        Some(ServiceEnded::ServedItsTime),
        "the service closed because its time was up, not because it was left in silence"
    );
}

#[test]
fn a_beat_the_service_will_not_take_is_the_end_of_serving_the_connection() {
    // A refused beat is a connection already on its way out, so serving it
    // stops there. Reading on and beating again would be a loop turning against
    // a socket that has gone, and the run holding the keyboards behind it with
    // nowhere to send them (ADR-0008) — the fact worth having is that the loop
    // came back, which is what a run acts on.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_will_not_beat(BROKEN_PIPE);

    run(&mut mac);

    assert_eq!(beats(&mac), 1, "the one that failed, and no second try");
    assert_eq!(
        mac.output_ended(),
        None,
        "this end let go, so the service never had to"
    );
}

#[test]
fn the_recording_holds_what_went_out_on_the_connection_and_which_way_it_ended() {
    // A recording of the injections alone shows a run writing reports
    // successfully right up to the moment it stopped, which is what every one of
    // this loop's ways out looks like from there. The two facts that separate
    // them are the frames this end put on the connection and the way the loop
    // came back, so both are records of their own (ADR-0009).
    let mut mac = SimHost::new().whose_output_serves_for(A_WHILE);

    let records = records(&recorded(&mut mac));

    assert!(
        records.iter().any(|record| matches!(
            record,
            Record::Served {
                frame: Served::Beat,
                code: 0
            }
        )),
        "no beat is in the recording, so a reading cannot tell one that went from one that never happened"
    );
    assert_eq!(how_it_ended(&records), Some(Lost::ServiceClosedIt));
}

#[test]
fn a_frame_that_arrived_in_pieces_is_the_end_of_the_connection_and_is_named_as_such() {
    // Half a frame and then nothing is a stream with no way back: the bytes
    // already taken cannot be put back, so where the next frame starts is no
    // longer known. Reading on would take the middle of a frame for its length,
    // and a length read out of somebody else's bytes is a read that waits for
    // however many the number happened to say — which is a loop that stops
    // beating without ever saying it stopped (ADR-0009).
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_tears_a_frame();

    let records = records(&recorded(&mut mac));

    assert_eq!(how_it_ended(&records), Some(Lost::FrameArrivedInPieces));
}

#[test]
fn a_beat_the_service_would_not_take_is_in_the_recording_with_the_number_it_answered() {
    // The number and not that it failed: a write to a connection that has gone
    // and one that ran out of time are the same refusal and different problems,
    // and which it was is the whole of what a reading is for (ADR-0009).
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_will_not_beat(BROKEN_PIPE);

    let records = records(&recorded(&mut mac));

    assert!(records.iter().any(|record| matches!(
        record,
        Record::Served {
            frame: Served::Beat,
            code: BROKEN_PIPE
        }
    )));
    assert_eq!(how_it_ended(&records), Some(Lost::BeatWillNotGo));
}

#[test]
fn a_health_check_the_service_asks_is_answered() {
    // The service asks whoever is connected whether they are still there, and a
    // connection that did not answer is one it closes: a run that read the
    // question and said nothing would be handing its keyboards back a few seconds
    // later with nothing to say why.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_asks(favjit_host_sim::a_health_check());

    let records = records(&recorded(&mut mac));

    assert!(
        records.iter().any(|record| matches!(
            record,
            Record::Served {
                frame: Served::HealthCheckAnswer,
                code: 0
            }
        )),
        "the answer has to be in the recording, so a reading can tell a question \
         that was answered from one that was read and dropped: {records:?}"
    );
}

#[test]
fn a_request_the_service_makes_is_acknowledged_with_the_id_it_came_with() {
    // The id and not an acknowledgement on its own: the service has more than one
    // request outstanding, so one answered without saying which would be an answer
    // it cannot place — and it closes a connection whose answers it cannot place.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_asks(favjit_host_sim::a_request());

    let records = records(&recorded(&mut mac));

    assert!(
        records.iter().any(|record| matches!(
            record,
            Record::Served {
                frame: Served::Acknowledgement,
                code: 0
            }
        )),
        "{records:?}"
    );
}

#[test]
fn a_connection_whose_reads_cannot_be_bounded_is_never_converted_through() {
    // An unbounded read is a loop that never comes back round: it would stop
    // beating and stop answering, and the service would close the connection
    // while the run sat in that read holding the keyboards — which is ADR-0008's
    // outcome reached by the one path no keystroke goes through.
    //
    // Reported as there being no output rather than as the connection ending,
    // because that is where the bound is asked for: the read is bounded on the way
    // to finding out whether a keyboard is up, so a machine that refuses one never
    // gets far enough for there to be a connection to lose.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_will_not_bound_its_reads();

    assert_eq!(run(&mut mac), Ending::NoOutput);
}

#[test]
fn a_read_the_socket_cut_short_is_made_again_rather_than_read_as_anything() {
    // Neither a quiet cadence nor a connection going: no bytes came off the
    // stream and no bound came round, so the same read is worth making again —
    // and a run that beat on it would beat on a signal instead of on a clock.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_socket_answers(Took::Interrupted);

    let records = records(&recorded(&mut mac));

    assert_eq!(
        how_it_ended(&records),
        Some(Lost::ServiceClosedIt),
        "the run carries on to the end of the service's own time rather than \
         letting go at the interruption: {records:?}"
    );
}

#[test]
fn a_read_that_answers_with_no_bytes_at_all_is_the_connection_gone() {
    // The count and not what it means is what the socket answers, so reading
    // none as the far end having closed is the run's: a run that waited instead
    // would sit on a closed socket holding the keyboards, which is the outcome
    // ADR-0008 rules out.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_socket_answers(Took::Bytes(0));

    let records = records(&recorded(&mut mac));

    assert_eq!(how_it_ended(&records), Some(Lost::ServiceClosedIt));
}

#[test]
fn a_frame_whose_length_arrived_in_part_is_the_connection_let_go_of() {
    // Part of the length and then the bound coming round: the bytes taken cannot
    // be put back, so where the next frame starts is no longer known — and a
    // length read out of somebody else's bytes is a read that waits for however
    // many the number happened to say.
    let mut mac = SimHost::new()
        .whose_output_serves_for(A_WHILE)
        .whose_output_socket_answers(Took::Bytes(2))
        .whose_output_socket_answers(Took::Nothing);

    let records = records(&recorded(&mut mac));

    assert_eq!(how_it_ended(&records), Some(Lost::FrameArrivedInPieces));
}
