//! What a forwarding run says about how it was asked to start.
//!
//! One cost, and it is not the request's doing: a relaying run refuses this
//! machine's input, so a wedge leaves it with none at all and nothing to end the
//! process holding it (ADR-0008). It is not an error — the run goes ahead — so the
//! only record that it was said is what the machine was told, which is why saying it
//! is a host operation and why it is checked here (ADR-0006).
//!
//! Nothing about the flags, because a combination that costs something is not
//! expressible: refusing and relaying are one mode.
//!
//! Matched on a word rather than on the whole sentence. Which warning fired is the
//! behaviour; how it is phrased is not.

use favjit_engine::source::{self, Request};
use favjit_engine::DeviceId;
use favjit_host::source::Driving;
use favjit_host::EventKind;
use favjit_host_sim::{identity, SimSource};

/// The sink this machine is pinned to.
const SINK: u8 = 2;

fn run(request: &Request, windows: &mut SimSource) {
    source::run(request, false, windows, None);
}

fn warnings_about(word: &str, windows: &SimSource) -> usize {
    windows
        .warnings()
        .iter()
        .filter(|said| said.contains(word))
        .count()
}

#[test]
fn relaying_with_no_watchdog_warns_that_nothing_would_notice_a_wedge() {
    // The keyboards are refused, so a wedge leaves this machine with no input at
    // all and nothing to end the process holding it.
    let mut windows = SimSource::new().with_no_watchdog();
    windows.pinned_to(identity(SINK));

    run(&Request::Relaying, &mut windows);

    assert_eq!(warnings_about("watchdog", &windows), 1);
}

#[test]
fn relaying_under_a_watchdog_is_the_arrangement_with_nothing_to_say() {
    // Asked for rather than assumed: whether anything watches this process is the
    // machine's answer, and where that changes it is the answer that changes.
    let mut windows = SimSource::new();
    windows.pinned_to(identity(SINK));

    run(&Request::Relaying, &mut windows);

    assert_eq!(windows.warnings(), Vec::<String>::new());
}

#[test]
fn a_make_code_nothing_names_is_said_and_relayed_as_nothing() {
    // The keyboards here are PC keyboards, and one can report a key this table has
    // no name for. Saying so is what makes it findable: relayed as some other key
    // it would type the wrong thing on the Mac, and dropped in silence it looks
    // exactly like a key that never reported at all.
    //
    // Scripted as the raw make code rather than as a `Key`, because a `Key` is
    // what does not exist for it.
    const UNNAMED: u16 = 0x7f;
    let mut windows = SimSource::new();
    // Sent over first, so there is a link for the announcement to cross and the key not
    // to: a run comes up with the keyboard on the machine it is running on (ADR-0013).
    windows.asked_for(Driving::TheSink);
    windows.pinned_to(identity(SINK));
    windows.attach_external(DeviceId(1), 1, 2);
    windows.script(EventKind::HookedKey {
        device: DeviceId(1),
        make_code: UNNAMED,
        // A virtual key, because the filler value is what says a report is not a
        // key at all — and this one is a key, at a position nothing names.
        vkey: 0x41,
        flags: 0,
    });

    run(&Request::Relaying, &mut windows);

    let said = windows.warnings().join("\n");
    assert!(
        said.contains(&format!("{UNNAMED:#04x}")),
        "the make code is what a person adds to the table, so the line has to \
         carry it; said: {said}"
    );
    assert_eq!(
        windows.sent().len(),
        1,
        "the device's own announcement crossed and the key did not"
    );
}

#[test]
fn a_dry_run_says_nothing_even_with_nothing_watching() {
    // It refuses nothing, so there is no keyboard being held for a watchdog to be
    // missing from.
    let mut windows = SimSource::new().with_no_watchdog();
    windows.pinned_to(identity(SINK));

    run(&Request::DryRun, &mut windows);

    assert_eq!(windows.warnings(), Vec::<String>::new());
}

#[test]
fn the_warning_comes_before_the_keyboards_are_taken() {
    // The order is the point: a cost named after the keyboards are refused is one
    // the person can no longer decide about.
    let mut windows = SimSource::new().with_no_watchdog();
    windows.pinned_to(identity(SINK));

    run(&Request::Relaying, &mut windows);

    assert!(
        windows.warned_before_taking_input(),
        "input was taken before anything was said"
    );
}
