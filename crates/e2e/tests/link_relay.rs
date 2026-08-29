//! What the Windows source sends, and what the macOS sink makes of it.
//!
//! The property the whole topology rests on: input that crossed the link is
//! converted by exactly the same pipeline as input from a keyboard attached here
//! (ADR-0003). So each test does the same script twice — once into a sink
//! directly, once through a source and back out of a sink — and compares.

use core::time::Duration;

use favjit_core::pairing::Authorized;
use favjit_core::sink::Request;
use favjit_core::{
    link::Message, sink, source, DeviceId, DeviceInfo, Injected, Key, Layout, ModifierKeys as M,
    PointerReport,
};
use favjit_host_sim::{SimHost, SimLink, SimSource};

/// The key the sink has pinned, since a source it has not paired is refused before
/// a single frame is read (ADR-0004).
const PAIRED: u8 = 0xaa;

fn key(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

/// A run that converts, since what these are about is that input from either side
/// converts the same way rather than how the machine was brought up.
fn converting() -> Request {
    Request::Injecting { listen: true }
}

/// The forwarding machine's side of the same, for the same reason.
fn forwarding() -> source::Request {
    source::Request::Relaying
}

/// The keyboard on the Windows side. External, so it takes the raw-JIS remaps
/// rather than the layers the MacBook's own keyboard gets.
const REMOTE: DeviceId = DeviceId(7);

/// Run a script against a sink here, with the keyboard attached locally.
fn converted_here(script: impl Fn(&mut SimHost)) -> Vec<Injected> {
    let mut host = SimHost::new();
    host.attach(DeviceInfo::external(REMOTE, 1234, 5678));
    script(&mut host);
    sink::run(&converting(), Layout::dudrack(), None, &mut host, None);
    host.injected()
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
fn converted_over_the_link(script: impl Fn(&mut SimSource)) -> Vec<Injected> {
    let mut source_host = SimSource::new();
    script(&mut source_host);
    source::run(&forwarding(), &mut source_host);

    let mut link = SimLink::new(Authorized::added("", &key(PAIRED)));
    link.connect(key(PAIRED)).relay(source_host.sent());

    let mut mac = SimHost::new().with_link(link);
    sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    mac.injected()
}

#[test]
fn a_key_from_the_source_is_converted_like_one_from_here() {
    // `k` in Dudrack's raw-JIS remap for an external keyboard: the one thing the
    // link exists for has to come out the same either way.
    let here = converted_here(|host| {
        host.tap(REMOTE, Key::K);
    });
    let over = converted_over_the_link(|source| {
        source.attach(DeviceInfo::external(REMOTE, 1234, 5678));
        source.tap(REMOTE, Key::K);
    });

    assert_eq!(over, here);
    assert!(!here.is_empty(), "the script should convert to something");
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
        source.attach(DeviceInfo::external(REMOTE, 1234, 5678));
        source.tap(REMOTE, Key::RightControl);
    });
    let here = converted_here(|host| {
        host.tap(REMOTE, Key::RightControl);
    });

    assert_eq!(
        over.first(),
        Some(&Injected::KeyDown {
            key: Key::LeftCommand,
            modifiers: M::of(&[Key::LeftCommand])
        }),
        "right control from the source should be a command key: {over:?}"
    );
    assert_eq!(
        here.first(),
        Some(&Injected::KeyDown {
            key: Key::RightControl,
            modifiers: M::of(&[Key::RightControl])
        }),
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
        source.attach(DeviceInfo::external(REMOTE, 1234, 5678));
        source
            .press(REMOTE, Key::Spacebar)
            .advance(Duration::from_millis(300))
            .press(REMOTE, Key::J)
            .release(REMOTE, Key::J)
            .release(REMOTE, Key::Spacebar);
    });

    assert_eq!(over, here);
}

#[test]
fn the_pointer_crosses_too() {
    // The TrackPoint keyboard is one device, so a source that suppressed its keys
    // has its pointer as well and nothing else can relay it.
    let report = PointerReport::moved(4, -2);
    let over = converted_over_the_link(|source| {
        source.attach(DeviceInfo::external(REMOTE, 1234, 5678));
        source.pointer(REMOTE, report);
    });

    assert_eq!(over, vec![Injected::Pointer(report)]);
}

#[test]
fn the_source_sends_input_and_nothing_else() {
    // A probe is the local watchdog asking this process whether it is alive, and
    // a timer is this process's own wake-up. Relaying either would be asking the
    // other machine about the state of this one.
    let mut source_host = SimSource::new();
    source_host.attach(DeviceInfo::external(REMOTE, 1234, 5678));
    source_host.probe();
    source_host.tap(REMOTE, Key::K);
    source::run(&forwarding(), &mut source_host);

    let sent: Vec<Message> = source_host.sent().iter().map(|s| s.message).collect();
    assert_eq!(
        sent,
        vec![
            Message::DeviceAttached(DeviceInfo::external(REMOTE, 1234, 5678)),
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
    let mut source_host = SimSource::new();
    source_host.attach(DeviceInfo::external(REMOTE, 1234, 5678));
    source_host.link_gone();
    source_host.press(REMOTE, Key::K);
    source_host.press(REMOTE, Key::J);
    source::run(&forwarding(), &mut source_host);

    assert_eq!(source_host.sent().len(), 1);
}

#[test]
fn the_source_answers_its_own_watchdog() {
    // ADR-0008's supervisor is per machine: the source's loop has to report that
    // it came round, or the process holding the Windows keyboards would be killed
    // for being quiet while it was working.
    let mut source_host = SimSource::new();
    source_host.attach(DeviceInfo::external(REMOTE, 1234, 5678));
    source_host.probe();
    source::run(&forwarding(), &mut source_host);

    // One for the attach, one for the probe.
    assert_eq!(source_host.heartbeats().len(), 2);
}

#[test]
fn keys_the_source_cannot_name_do_not_reach_the_sink() {
    // The source is thin by design, and a key it has no name for is a key the
    // layout could not have a rule for either. Sending it would be sending the
    // sink something it can only drop, one round trip later.
    let over = converted_over_the_link(|source| {
        source.attach(DeviceInfo::external(REMOTE, 1234, 5678));
        source.tap(REMOTE, Key::K);
    });
    let with_a_detach = converted_over_the_link(|source| {
        source.attach(DeviceInfo::external(REMOTE, 1234, 5678));
        source.tap(REMOTE, Key::K);
        source.detach(REMOTE);
    });

    // The detach is relayed, and it releases nothing because nothing was held.
    assert_eq!(with_a_detach, over);
}

#[test]
fn a_key_held_when_the_link_drops_is_released() {
    // The sink is what the OS believes: a source that goes away mid-keystroke
    // must not leave a modifier held down in every application, which is the
    // failure ADR-0002 puts on the sink.
    let over = converted_over_the_link(|source| {
        source.attach(DeviceInfo::external(REMOTE, 1234, 5678));
        source.press(REMOTE, Key::LeftShift);
        source.detach(REMOTE);
    });

    assert_eq!(
        over,
        vec![
            Injected::KeyDown {
                key: Key::LeftShift,
                modifiers: M::of(&[Key::LeftShift])
            },
            Injected::KeyUp {
                key: Key::LeftShift,
                modifiers: M::NONE
            },
        ]
    );
}
