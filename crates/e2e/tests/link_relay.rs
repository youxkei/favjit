//! What the Windows source sends, and what the macOS sink makes of it.
//!
//! The property the whole topology rests on: input that crossed the link is
//! converted by exactly the same pipeline as input from a keyboard attached here
//! (ADR-0003). So each test does the same script twice — once into a sink
//! directly, once through a source and back out of a sink — and compares.
//!
//! Compared in the bytes the sink actually wrote, decoded back with
//! `favjit-hid`'s inverse of the encoding: `Injected` is `engine`'s own record of
//! what it decided, not anything either path's own device would receive.

use core::time::Duration;

use favjit_engine::sink::{InputConfig, Request, Settings};
use favjit_engine::{pointer::Tuning, sink, source, DeviceId, Key, Layout, PointerReport};
use favjit_hid::report::{modifier_bit, pointing_from_bytes, Report};
use favjit_host::OutputReport;

/// The key the sink has pinned, since a source it has not paired is refused before
/// a single frame is read (ADR-0004).
const PAIRED: u8 = 0xaa;

/// The file `favjit --pair` leaves behind, holding that one key.
///
/// Written here rather than built with the simulator's own writer: a script that
/// started from the same function the simulator writes that file with would be
/// checking that function against itself.
fn paired_list() -> String {
    format!("{}\n", hex(identity(PAIRED).public()))
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A run that converts, since what these are about is that input from either side
/// converts the same way rather than how the machine was brought up.
fn converting() -> Request {
    Request::Injecting { listen: true }
}
use favjit_engine::link::{Attached, Message};
use favjit_host::source::Driving;
use favjit_host_sim::{identity, SimHost, SimLink, SimSource};

/// The sink the source runs below are pinned to — unrelated to [`PAIRED`],
/// which is the Mac's own list.
const THIS_MACHINES_SINK: u8 = 21;

/// The forwarding machine's side of the same, for the same reason.
fn forwarding() -> source::Request {
    source::Request::Relaying
}

/// Run the forwarding machine, pinned to its own sink.
fn forward(host: &mut SimSource) {
    host.pinned_to(identity(THIS_MACHINES_SINK));
    source::run(&forwarding(), false, host, None);
}

/// The beats a run makes on its way to a link, beside the one each event gets.
///
/// Two: one before the connection and one before the handshake, which are the waits on
/// that path a run cannot take in pieces — so what it owes its supervisor there is a
/// beat on each side of them (ADR-0008). Counted rather than left out of the sums below,
/// because a third blocking call added there with no beat in front of it is exactly the
/// failure this catches: a run ended while it was reaching the other machine.
const BEATS_PER_LINK: usize = 2;

/// The Windows machine, asked to send its keyboard over before anything else happens.
///
/// A run comes up with the keyboard on the machine it is running on and sends nothing
/// until it is asked to (ADR-0013). Which way it was asked is `switching.rs`'s subject;
/// here it is what has to be true for anything to reach the Mac at all.
fn sending() -> SimSource {
    let mut host = SimSource::default();
    host.asked_for(Driving::TheSink);
    host
}

/// The keyboard on the Windows side. External, so it takes the raw-JIS remaps
/// rather than the layers the MacBook's own keyboard gets.
const REMOTE: DeviceId = DeviceId(7);

/// Every report a run wrote, raw — some tests are about a keyboard report and
/// one is about a pointing report, so which to decode is the caller's to say.
type Reports = Vec<(OutputReport, Vec<u8>)>;

/// The keyboard state each report described, in order — nothing here types on
/// a control's page.
fn keyboard_reports(reports: &Reports) -> Vec<Report> {
    reports
        .iter()
        .map(|(report, bytes)| {
            assert_eq!(*report, OutputReport::Keyboard, "not a control");
            Report::from_bytes(bytes).expect("the bytes of a keyboard report")
        })
        .collect()
}

/// The pointer state each report described, in order.
fn pointer_reports(reports: &Reports) -> Vec<PointerReport> {
    reports
        .iter()
        .map(|(report, bytes)| {
            assert_eq!(*report, OutputReport::Pointing, "not a keyboard report");
            pointing_from_bytes(bytes).expect("the bytes of a pointing report")
        })
        .collect()
}

/// The report a keyboard holding only this modifier, nothing else, would carry.
fn holding_modifier(key: Key) -> Report {
    Report {
        modifiers: modifier_bit(key).expect("a modifier key"),
        ..Report::default()
    }
}

/// Run a script against a sink here, with the keyboard attached locally.
fn converted_here(script: impl Fn(&mut SimHost)) -> Reports {
    let mut host = SimHost::new();
    host.attach_external(REMOTE, 1234, 5678);
    script(&mut host);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );
    host.reports()
        .iter()
        .map(|(_, report, bytes)| (*report, bytes.clone()))
        .collect()
}

/// The same script, driven through a run of the source and a run of the sink.
///
/// Everything between the two is the real path: the messages the source's loop
/// handed to its link arrive as the records the sink's link reads, and what the
/// sink converts is what its own link put into its stream.
///
/// The link has no latency here, so each message keeps the time the source saw it.
/// Anything else would answer a tap-versus-hold question differently on the two
/// sides of this comparison and make it meaningless.
fn converted_over_the_link(script: impl Fn(&mut SimSource)) -> Reports {
    let mut source_host = sending();
    script(&mut source_host);
    forward(&mut source_host);

    let mut link = SimLink::new(paired_list());
    link.connect(identity(PAIRED)).relay(&source_host.sent());

    let mut mac = SimHost::new().with_link(link);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );
    mac.reports()
        .iter()
        .map(|(_, report, bytes)| (*report, bytes.clone()))
        .collect()
}

#[test]
fn a_key_from_the_source_is_converted_like_one_from_here() {
    // `k` in Dudrack's raw-JIS remap for an external keyboard: the one thing the
    // link exists for has to come out the same either way.
    let here = converted_here(|host| {
        host.tap(REMOTE, Key::K);
    });
    let over = converted_over_the_link(|source| {
        source.attach_external(REMOTE, 1234, 5678);
        source.tap(REMOTE, Key::K);
    });

    assert_eq!(keyboard_reports(&over), keyboard_reports(&here));
    assert!(
        !keyboard_reports(&here).is_empty(),
        "the script should convert to something"
    );
}

#[test]
fn right_control_from_the_source_is_a_command_key() {
    // A PC keyboard has right control where a Mac has nothing much, and command is the
    // modifier a person reaches for most on the machine they are typing *into*. The
    // Dudrack keyboards have their own answer to that in caps lock and tab.
    //
    // Scoped to what arrives over the link and not to external keyboards in general,
    // because the two are different keyboards: one is at the other machine and one is
    // under the person's other hand.
    let over = converted_over_the_link(|source| {
        source.attach_external(REMOTE, 1234, 5678);
        source.tap(REMOTE, Key::RightControl);
    });
    let here = converted_here(|host| {
        host.tap(REMOTE, Key::RightControl);
    });

    let over = keyboard_reports(&over);
    let here = keyboard_reports(&here);
    assert_eq!(
        over.first(),
        Some(&holding_modifier(Key::LeftCommand)),
        "right control from the source should be a command key: {over:?}"
    );
    assert_eq!(
        here.first(),
        Some(&holding_modifier(Key::RightControl)),
        "the same keyboard plugged in here is untouched: {here:?}"
    );
}

#[test]
fn a_hold_is_still_a_hold_at_the_other_end() {
    // The rules that read time are the ones a link could break: a hold is a hold
    // because of how long the key was down, and the source is where that was
    // observed.
    let here = converted_here(|host| {
        host.press(REMOTE, Key::Spacebar)
            .advance(Duration::from_millis(300))
            .press(REMOTE, Key::J)
            .release(REMOTE, Key::J)
            .release(REMOTE, Key::Spacebar);
    });
    let over = converted_over_the_link(|source| {
        source.attach_external(REMOTE, 1234, 5678);
        source
            .press(REMOTE, Key::Spacebar)
            .advance(Duration::from_millis(300))
            .press(REMOTE, Key::J)
            .release(REMOTE, Key::J)
            .release(REMOTE, Key::Spacebar);
    });

    assert_eq!(keyboard_reports(&over), keyboard_reports(&here));
}

#[test]
fn the_pointer_crosses_too() {
    // The TrackPoint keyboard is one device, so a source that suppressed its keys
    // has its pointer as well and nothing else can relay it.
    let report = PointerReport::moved(4, -2);
    let over = converted_over_the_link(|source| {
        source.attach_external(REMOTE, 1234, 5678);
        source.pointer(REMOTE, report);
    });

    assert_eq!(pointer_reports(&over), vec![report]);
}

#[test]
fn the_source_sends_input_and_nothing_else() {
    // A probe is the local watchdog asking this process whether it is alive, and
    // a timer is this process's own wake-up. Relaying either would be asking the
    // other machine about the state of this one.
    let mut source_host = sending();
    source_host.attach_external(REMOTE, 1234, 5678);
    source_host.probe();
    source_host.tap(REMOTE, Key::K);
    forward(&mut source_host);

    let sent: Vec<Message> = source_host.sent().iter().map(|s| s.message).collect();
    assert_eq!(
        sent,
        vec![
            Message::DeviceAttached(Attached {
                device: REMOTE,
                is_built_in: false,
                vendor_id: Some(1234),
                product_id: Some(5678),
            }),
            Message::KeyDown {
                device: REMOTE,
                key: Key::K
            },
            Message::KeyUp {
                device: REMOTE,
                key: Key::K
            },
        ]
    );
}

#[test]
fn a_source_whose_link_has_gone_stops_sending() {
    // Ending the run rather than writing into a closed socket: the process is
    // supervised, so stopping is what gets it started again — and a source that
    // kept reading keyboards it can no longer relay would be suppressing input on
    // that machine for nothing.
    let mut source_host = sending();
    source_host.attach_external(REMOTE, 1234, 5678);
    source_host.link_gone();
    source_host.press(REMOTE, Key::K);
    source_host.press(REMOTE, Key::J);
    forward(&mut source_host);

    assert_eq!(source_host.sent().len(), 1);
}

#[test]
fn the_source_answers_its_own_watchdog() {
    // ADR-0008's supervisor is per machine: the source's loop has to report that
    // it came round, or the process holding the Windows keyboards would be killed
    // for being quiet while it was working.
    let mut source_host = sending();
    source_host.attach_external(REMOTE, 1234, 5678);
    source_host.probe();
    forward(&mut source_host);

    // One for the ask that sent the keyboard over, one for the attach, one for the
    // probe — and the two a run makes on its way to a link.
    assert_eq!(source_host.heartbeats().len(), 3 + BEATS_PER_LINK);
}

#[test]
fn keys_the_source_cannot_name_do_not_reach_the_sink() {
    // The source is thin by design, and a key it has no name for is a key the
    // layout could not have a rule for either. Sending it would be sending the
    // sink something it can only drop, one round trip later.
    let over = converted_over_the_link(|source| {
        source.attach_external(REMOTE, 1234, 5678);
        source.tap(REMOTE, Key::K);
    });
    let with_a_detach = converted_over_the_link(|source| {
        source.attach_external(REMOTE, 1234, 5678);
        source.tap(REMOTE, Key::K);
        source.detach(REMOTE);
    });

    // The detach is relayed, and it releases nothing because nothing was held.
    assert_eq!(keyboard_reports(&with_a_detach), keyboard_reports(&over));
}

#[test]
fn a_key_held_when_the_link_drops_is_released() {
    // The sink is what the OS believes: a source that goes away mid-keystroke
    // must not leave a modifier held down in every application, which is the
    // failure ADR-0002 puts on the sink.
    let over = converted_over_the_link(|source| {
        source.attach_external(REMOTE, 1234, 5678);
        source.press(REMOTE, Key::LeftShift);
        source.detach(REMOTE);
    });

    assert_eq!(
        keyboard_reports(&over),
        vec![holding_modifier(Key::LeftShift), Report::default()]
    );
}

#[test]
fn invert_horizontal_wheel_flips_the_sign_of_a_relayed_wheel() {
    // The horizontal wheel is real HID, but not on the keyboard the sink converts
    // locally: that stick has never been observed to report one
    // (`docs/platform/macos/input-suppression.md`). A mouse relayed from Windows
    // reports it in `RAWMOUSE`, so this is where `Tuning::invert_horizontal_wheel`
    // is checked against a device that can actually carry the axis end to end.
    let mut source_host = sending();
    source_host.attach_external(REMOTE, 1234, 5678);
    source_host.pointer(
        REMOTE,
        PointerReport {
            horizontal_wheel: 2,
            ..PointerReport::default()
        },
    );
    forward(&mut source_host);

    let mut link = SimLink::new(paired_list());
    link.connect(identity(PAIRED)).relay(&source_host.sent());

    let mut mac = SimHost::new().with_link(link);
    sink::run(
        &converting(),
        Layout::dudrack(),
        Settings {
            pointer: Tuning {
                invert_horizontal_wheel: true,
                ..Tuning::default()
            },
            ..Settings::default()
        },
        InputConfig::default(),
        &mut mac,
        None,
    );

    let over: Reports = mac
        .reports()
        .iter()
        .map(|(_, report, bytes)| (*report, bytes.clone()))
        .collect();
    assert_eq!(
        pointer_reports(&over),
        vec![PointerReport {
            horizontal_wheel: -2,
            ..PointerReport::default()
        }]
    );
}
