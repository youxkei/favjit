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

use favjit_core::source::{self, Request};
use favjit_host_sim::SimSource;

fn run(request: &Request, windows: &mut SimSource) {
    source::run(request, windows);
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

    run(&Request::Relaying, &mut windows);

    assert_eq!(warnings_about("watchdog", &windows), 1);
}

#[test]
fn relaying_under_a_watchdog_is_the_arrangement_with_nothing_to_say() {
    // Asked for rather than assumed: whether anything watches this process is the
    // machine's answer, and where that changes it is the answer that changes.
    let mut windows = SimSource::new();

    run(&Request::Relaying, &mut windows);

    assert_eq!(windows.warnings(), Vec::<String>::new());
}

#[test]
fn a_dry_run_says_nothing_even_with_nothing_watching() {
    // It refuses nothing, so there is no keyboard being held for a watchdog to be
    // missing from.
    let mut windows = SimSource::new().with_no_watchdog();

    run(&Request::DryRun, &mut windows);

    assert_eq!(windows.warnings(), Vec::<String>::new());
}

#[test]
fn the_warning_comes_before_the_keyboards_are_taken() {
    // The order is the point: a cost named after the keyboards are refused is one
    // the person can no longer decide about.
    let mut windows = SimSource::new().with_no_watchdog();

    run(&Request::Relaying, &mut windows);

    assert!(
        windows.warned_before_taking_input(),
        "input was taken before anything was said"
    );
}
