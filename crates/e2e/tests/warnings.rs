//! What a run says about how it was asked to start.
//!
//! One cost, and it is not the request's doing: a run that delivers takes the
//! keyboards exclusively, and on a machine with nothing watching it a wedge keeps
//! them (ADR-0008). It is not an error — the run goes ahead — so the only record
//! that it was said is what the machine was told, which is why saying it is a host
//! operation and why it is checked here (ADR-0006): the wording is `engine`'s, and a
//! front-end that forgot to say it cannot exist.
//!
//! Nothing about the flags, because a combination that costs something is not
//! expressible: `asking_for_nothing.rs` is where that is recorded.
//!
//! Matched on a word rather than on the whole sentence. Which warning fired is the
//! behaviour; how it is phrased is not, and a suite that pinned the prose would
//! make every improvement to it a failing test.

use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::Layout;
use favjit_host_sim::{Did, SimHost};

fn run(request: &Request, mac: &mut SimHost) {
    sink::run(
        request,
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        None,
    );
}

/// Delivering without listening, since what these are about is this machine.
fn delivering() -> Request {
    Request::Injecting { listen: false }
}

fn warnings_about(word: &str, mac: &SimHost) -> usize {
    mac.warnings().iter().filter(|w| w.contains(word)).count()
}

#[test]
fn delivering_with_no_watchdog_warns_that_nothing_would_notice_a_wedge() {
    // ADR-0008: a run holding the keyboards must not be able to wedge with nothing
    // to end it, and the watchdog is what ends it.
    let mut mac = SimHost::new().with_no_watchdog();

    run(&delivering(), &mut mac);

    assert_eq!(warnings_about("watchdog", &mac), 1);
}

#[test]
fn delivering_under_a_watchdog_is_the_arrangement_with_nothing_to_say() {
    // Everything as it should be: input taken exclusively, delivered, and a wedge
    // would be noticed.
    let mut mac = SimHost::new();

    run(&delivering(), &mut mac);

    assert_eq!(mac.warnings(), Vec::<String>::new());
}

#[test]
fn a_dry_run_says_nothing_even_with_nothing_watching() {
    // It takes nothing, so there is no keyboard being held for a watchdog to be
    // missing from.
    let mut mac = SimHost::new().with_no_watchdog();

    run(&Request::DryRun, &mut mac);

    assert_eq!(mac.warnings(), Vec::<String>::new());
}

#[test]
fn the_warning_comes_before_the_keyboards_are_taken() {
    // The order is the point: a cost named after the keyboards are held is one the
    // person can no longer decide about.
    let mut mac = SimHost::new().with_no_watchdog();

    run(&delivering(), &mut mac);

    let did = mac.did();
    let warned = did.iter().position(|did| *did == Did::Warned);
    let took = did.iter().position(|did| *did == Did::LookedForDevices);
    assert!(
        warned.is_some() && took.is_some() && warned < took,
        "{did:?}"
    );
}

#[test]
fn an_output_that_never_came_up_says_which_of_the_two_answers_it_was() {
    // One answer is a package or a privilege and the other is a driver that
    // never activated, and they send a person looking in different places. The
    // machine reports which; what it means is said here (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
    let mut nothing_listening = SimHost::new().with_no_output();
    run(&delivering(), &mut nothing_listening);
    let said = nothing_listening.warnings().join("\n");
    assert!(
        said.contains("needs root") && said.contains("package"),
        "nothing answered on the socket; said: {said}"
    );

    let mut never_ready = SimHost::new().whose_output_is_never_ready(true, false, false);
    run(&delivering(), &mut never_ready);
    let said = never_ready.warnings().join("\n");
    assert!(
        said.contains("never reported a ready keyboard")
            && said.contains("driver_activated=true")
            && said.contains("driver_connected=false"),
        "it answered and nothing arrived; said: {said}"
    );
}
