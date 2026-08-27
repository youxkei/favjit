//! What keeps the connection to the output device open once both devices are up.
//!
//! The service on the other end of it beats every few seconds and closes a
//! connection it has heard nothing on for fifteen
//! (`docs/platform/macos/virtual-hid-device.md`), and a closed connection is a run
//! whose keystrokes go nowhere: the keyboards are handed back and the process
//! ends. So these are ADR-0008's rule about suppression outliving the ability to
//! process input, one step before the wedge — the keyboards do come back, and what
//! the person at the keyboard sees is favjit going away every few seconds.

use core::time::Duration;

use favjit_engine::sink::{self, Ending, InputConfig, Request};
use favjit_engine::trace::{Lost, Record, Served, Trace};
use favjit_engine::Layout;
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

fn beats(mac: &SimHost) -> usize {
    mac.output_calls()
        .iter()
        .filter(|call| matches!(call, OutputCall::Beat))
        .count()
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
