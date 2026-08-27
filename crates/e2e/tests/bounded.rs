//! A run given how long to last, and what it does when that is up.
//!
//! Stated as a duration rather than as a moment, because the clock a run is
//! bounded against is the host's: whoever asks has a number of seconds, and
//! placing it is one read of that clock as the run comes up.
//!
//! Two things to do with the moment and never both, so a run asked for one gets
//! neither the other's behaviour nor a race between them: it stops, or it hangs
//! for the watchdog to end (ADR-0008) — and a hang reached on purpose is the only
//! way to find out whether the watchdog reaches one.

use core::time::Duration;

use favjit_engine::sink::{self, Bound, Ending, InputConfig, Request, Settings};
use favjit_engine::{DeviceId, Key, Layout};
use favjit_host_sim::{Did, SimHost};

const BUILT_IN: DeviceId = DeviceId(1);

const AFTER: Duration = Duration::from_secs(3);

fn run(mac: &mut SimHost, bound: Bound) -> Ending {
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        Settings {
            bound,
            ..Settings::default()
        },
        InputConfig::default(),
        mac,
        None,
    )
    .0
}

/// No time at all, so the bound is up on the run's first look.
///
/// Placed against the host's clock as the run comes up, so advancing that clock
/// beforehand moves the bound with it: what a case can shorten is the bound and
/// not the clock, and a real number of seconds would be one this suite sat
/// through.
const AT_ONCE: Duration = Duration::ZERO;

fn typing() -> SimHost {
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN).tap(BUILT_IN, Key::K);
    mac
}

#[test]
fn a_run_told_how_long_to_last_ends_as_asked_once_that_has_passed() {
    // The same ending a stop gives, because that is what it is: a bound is the
    // run being asked to stop at a moment it was told in advance, and an ending
    // of its own would make whatever supervises favjit act differently on the two.
    let mut mac = typing();

    assert_eq!(run(&mut mac, Bound::Stops(AFTER)), Ending::Converted);
}

#[test]
fn a_run_with_no_bound_is_not_ended_by_the_clock() {
    // What the default has to be: a run started at logon is one nobody gave a
    // number of seconds to, and one the clock ended would give the keyboards back
    // while the person was still typing on them.
    let mut mac = typing();

    assert_eq!(run(&mut mac, Bound::Whenever), Ending::Converted);
    assert!(
        !mac.did().contains(&Did::Parked),
        "and it hangs for nothing either: {:?}",
        mac.did()
    );
}

#[test]
fn a_run_told_to_wedge_hands_its_thread_to_the_machine_instead_of_stopping() {
    // What ADR-0008 rules out is a process holding this machine's input with
    // nothing left that would end it, and the watchdog is what covers it. A run
    // that reaches that on purpose is the only thing that can find out whether the
    // cover works, so asking for it has to reach the machine rather than end the
    // loop quietly.
    let mut mac = typing();

    run(&mut mac, Bound::Wedges(AT_ONCE));

    assert!(
        mac.did().contains(&Did::Parked),
        "the thread goes to the machine: {:?}",
        mac.did()
    );
}

#[test]
fn a_run_that_stops_at_its_bound_hands_its_thread_to_nothing() {
    // The other half of the pair: the two are one setting because ending at the
    // moment and hanging at it are two things to do with one moment, so a run that
    // did both would be a race whose loser never happened.
    let mut mac = typing();

    run(&mut mac, Bound::Stops(AT_ONCE));

    assert!(!mac.did().contains(&Did::Parked), "{:?}", mac.did());
}

#[test]
fn a_bound_says_so_before_it_hangs() {
    // A hang says nothing by itself — the process simply stops answering — so the
    // one record that it was asked for rather than arrived at is the line said on
    // the way in.
    let mut mac = typing();

    run(&mut mac, Bound::Wedges(AT_ONCE));

    let said = mac.warnings().join("\n");
    assert!(
        said.contains("wedged"),
        "so a person reading the log can tell a deliberate hang from a real one; \
         said: {said}"
    );
}

#[test]
fn a_run_switched_off_still_comes_back_at_its_bound() {
    // Otherwise the wait for the switch is one a bound cannot end: a run given
    // three seconds on a machine where converting is off would sit there for as
    // long as it stayed off, which is not what three seconds was asked for.
    //
    // Reported as the switch and not as the bound, because both are true and only
    // one of them says why nothing was converted: a person who gave a number of
    // seconds already knows it passed.
    let mut mac = SimHost::new().with_converting_off();

    assert_eq!(run(&mut mac, Bound::Stops(AT_ONCE)), Ending::SwitchedOff);
}
