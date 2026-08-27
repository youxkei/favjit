//! A relaying run's own recording, and the case a later run is driven from it.
//!
//! ADR-0009's trace is what a run is read from after it has stopped, and this is
//! where that claim is held for the forwarding machine: what a source recorded
//! has to say which events it resolved, what each send answered with, and enough
//! for a second run to be driven from the same events.
//!
//! **A number and not whether it worked.** The two failures a person has to tell
//! apart when a link keeps dropping — a write that ran out of time and a
//! connection the other end reset — reach the run as different numbers and the
//! same "it failed". So they are scripted here as numbers, and what the tests
//! below assert is that the number is what survives into the recording: a
//! recording that held a flag could not have a test written against either case.

use favjit_engine::source::{self, Request};
use favjit_engine::trace::{Reader, Record, Trace};
use favjit_engine::{EventKind, HostEvent, Key};
use favjit_host::source::Driving;
use favjit_host_sim::{identity, SimSource};

/// The sink this machine is pinned to.
const SINK: u8 = 2;

/// The keyboard these scripts type on, and the vendor and product a rule names
/// it with.
const KEYBOARD: favjit_engine::DeviceId = favjit_engine::DeviceId(1);
const TRACKPOINT: (u16, u16) = (0x17ef, 0x60e1);

/// What the other machine's write answered with, as the two failures worth
/// telling apart.
///
/// Windows' own numbers, written here rather than read from a header: this suite
/// runs on either machine, and what a test is about is that the run carried the
/// number it was given rather than that this number means anything to `engine`.
const TIMED_OUT: i32 = 10060;
const RESET_BY_PEER: i32 = 10054;

/// A machine that has been asked to send its keyboard over, with a keyboard on
/// it and something typed.
///
/// The ask first, because a run comes up with the keyboard on the machine it is
/// running on and sends nothing until it is asked (ADR-0013).
fn typed_on() -> SimSource {
    let mut host = SimSource::default();
    host.asked_for(Driving::TheSink);
    host.pinned_to(identity(SINK));
    host.attach_external(KEYBOARD, TRACKPOINT.0, TRACKPOINT.1);
    host.press(KEYBOARD, Key::S);
    host.release(KEYBOARD, Key::S);
    host
}

/// Run one, recording into a buffer of this size, and hand back what it wrote.
fn recorded(host: &mut SimSource, bytes: usize) -> Vec<u8> {
    let mut memory = vec![0u8; bytes];
    source::run(&Request::Relaying, false, host, Some(&mut memory));
    memory
}

/// The events a recording holds, in the order the run resolved them.
fn events(trace: &Reader<'_>) -> Vec<HostEvent> {
    trace
        .records()
        .filter_map(|(_, record)| match record {
            Record::Event(event) => Some(event),
            _ => None,
        })
        .collect()
}

/// What each of its sends answered with, in order.
fn answers(trace: &Reader<'_>) -> Vec<i32> {
    trace
        .records()
        .filter_map(|(_, record)| match record {
            Record::Sent { code, .. } => Some(code),
            _ => None,
        })
        .collect()
}

#[test]
fn a_relaying_run_records_the_events_it_resolved_and_what_each_send_answered() {
    let mut host = typed_on();

    let memory = recorded(&mut host, 64 * 1024);
    let trace = Trace::read(&memory);

    // The resolved events and not the raw ones: what Windows said is a scancode
    // behind a prefix, and the run naming it is the step a later case starts
    // after (ADR-0006).
    let recorded: Vec<EventKind> = events(&trace).into_iter().map(|event| event.kind).collect();
    assert!(
        recorded.contains(&EventKind::KeyDown {
            device: KEYBOARD,
            key: Key::S
        }),
        "{recorded:?}"
    );
    assert!(
        recorded.contains(&EventKind::KeyUp {
            device: KEYBOARD,
            key: Key::S
        }),
        "{recorded:?}"
    );

    // Every send, and every one of them went: a recording that held only the
    // failures could not show a link that was working.
    let answered = answers(&trace);
    assert!(!answered.is_empty(), "nothing was recorded as sent");
    assert!(answered.iter().all(|code| *code == 0), "{answered:?}");
}

#[test]
fn a_write_that_ran_out_of_time_and_one_the_other_end_reset_are_different_in_the_recording() {
    // The same script twice, differing only in what the machine answered — which
    // is the whole of what a person reading two of these has to go on.
    let mut timed_out = typed_on();
    timed_out.answers_a_send_with(TIMED_OUT).link_gone();
    timed_out.press(KEYBOARD, Key::A);

    let mut reset = typed_on();
    reset.answers_a_send_with(RESET_BY_PEER).link_gone();
    reset.press(KEYBOARD, Key::A);

    let first = recorded(&mut timed_out, 64 * 1024);
    let second = recorded(&mut reset, 64 * 1024);

    assert!(
        answers(&Trace::read(&first)).contains(&TIMED_OUT),
        "{:?}",
        answers(&Trace::read(&first))
    );
    assert!(
        answers(&Trace::read(&second)).contains(&RESET_BY_PEER),
        "{:?}",
        answers(&Trace::read(&second))
    );
    // Neither carries the other's, so a reader of one is not left choosing
    // between them.
    assert!(!answers(&Trace::read(&first)).contains(&RESET_BY_PEER));
    assert!(!answers(&Trace::read(&second)).contains(&TIMED_OUT));
}

#[test]
fn the_events_a_run_recorded_drive_a_second_run_that_relays_the_same_messages() {
    // The claim ADR-0009 rests on, for this machine: a recording is a case. What
    // the first run resolved is scripted into a second machine, and what reaches
    // the sink is compared — a trace that could not do this would be a debugging
    // aid rather than something the suite can be extended with.
    let mut first = typed_on();
    let memory = recorded(&mut first, 64 * 1024);

    // The input, without the frames carrying this machine's own history to the
    // other one: what a replay of a recording reproduces is the run's decisions
    // about input, and the history is the recording being moved rather than
    // anything the run decided (ADR-0009).
    let over_the_link: Vec<_> = first
        .sent()
        .iter()
        .map(|sent| sent.message)
        .filter(|message| !matches!(message, favjit_engine::link::Message::Recorded(_)))
        .collect();
    let from_the_recording = source::replay(&events(&Trace::read(&memory)));

    assert_eq!(over_the_link, from_the_recording);
    assert!(
        !from_the_recording.is_empty(),
        "a recording of a run that relayed replayed as nothing"
    );
}
