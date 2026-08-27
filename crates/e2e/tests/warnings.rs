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
use favjit_engine::{DeviceId, Key, Layout};
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

#[test]
fn a_keyboard_that_could_not_be_taken_says_which_refusal_it_was() {
    // The two send a person looking in different places: one is a run to make
    // again with privilege, and the other is another program to close. The machine
    // reports the code, and which failure a code is is said here — a reading kept
    // beside the call that produced it would be a host deciding what a person has
    // to do (ADR-0006).
    const REFUSED: DeviceId = DeviceId(2);

    let mut not_privileged = SimHost::new().with_a_keyboard_it_cannot_take(REFUSED, NOT_PRIVILEGED);
    not_privileged.attach_external(REFUSED, 1, 2);
    run(&delivering(), &mut not_privileged);
    let said = not_privileged.warnings().join("\n");
    assert!(
        said.contains("not privileged"),
        "a refusal a person answers with privilege; said: {said}"
    );

    let mut held_elsewhere =
        SimHost::new().with_a_keyboard_it_cannot_take(REFUSED, EXCLUSIVE_ACCESS);
    held_elsewhere.attach_external(REFUSED, 1, 2);
    run(&delivering(), &mut held_elsewhere);
    let said = held_elsewhere.warnings().join("\n");
    assert!(
        said.contains("something else holds it"),
        "a refusal a person answers by closing that program; said: {said}"
    );
}

/// The two codes the machine answers a refused seizure with
/// (`docs/platform/macos/permissions-through-a-signed-bundle.md`).
const NOT_PRIVILEGED: i32 = 0xE00002C1u32 as i32;
const EXCLUSIVE_ACCESS: i32 = 0xE00002C5u32 as i32;

#[test]
fn beats_that_reach_nothing_are_said_once_per_loop_and_not_per_beat() {
    // Said at all, because what happens next is the watchdog ending this process
    // for a broken link rather than for a fault, and those send a person looking
    // in different places (ADR-0008).
    //
    // Once per loop that beats, because each of them beats several times a second:
    // a line per beat would be the loudest thing in the log for the rest of the
    // run, and the run goes on — a beat that will not go is not a reason to hand
    // the keyboards back.
    //
    // Per loop rather than once for the process, because the two are separately
    // unheard: a converting loop nothing is reading and a loop serving the output
    // device that nothing is reading are two facts, and a line said for only the
    // first would leave the other silent.
    let mut mac = SimHost::new().whose_beats_reach_nothing();
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    run(&delivering(), &mut mac);

    assert_eq!(warnings_about("heartbeat", &mac), 2, "{:?}", mac.warnings());
}

#[test]
fn a_run_nothing_is_supervising_says_nothing_about_its_beats() {
    // A machine with no watchdog behind it has no pipe to write a beat down, and
    // reporting that as a beat which failed would say the watchdog is about to end
    // a process no watchdog started.
    let mut mac = SimHost::new().with_no_watchdog();
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    run(&delivering(), &mut mac);

    assert_eq!(warnings_about("heartbeat", &mac), 0, "{:?}", mac.warnings());
}
