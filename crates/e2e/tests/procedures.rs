//! What a run answers a procedure the platform calls with.
//!
//! Windows reads this machine's keys from a hook, because a low-level hook is the
//! only one that can refuse an event rather than merely watch it — and raw input
//! is downstream of it, so a key the hook refuses reaches nothing at all, favjit
//! included. That makes one procedure both where keys come from and where they
//! are turned down, and the order between those is the whole of what it does.
//!
//! **Nothing about it can be reached from a run.** The OS calls the procedure on
//! the thread the event arrived on, through a bare function pointer with nowhere
//! to hang anything off. So what is driven here is the answer rather than the
//! procedure: the run installs one, and this hands it events the way the platform
//! would (ADR-0006, ADR-0007).

use favjit_engine::source::{self, Arrived, Request, Suppressing};
use favjit_host_sim::{Answered, SimSource};

/// Where the chord's two keys are on this machine's keyboard.
///
/// Two positions this file makes up, because what a chord costs does not depend
/// on which they are: what matters is that one of them is not the other, and that
/// a key at neither is an ordinary key.
const TO_THE_SINK: (u16, bool) = (0x31, false);
const BACK_HERE: (u16, bool) = (0x1f, false);
const AN_ORDINARY_KEY: (u16, bool) = (0x20, false);

/// The same position as one of the chord's, on the other half of the keyboard.
///
/// A make code says which position only with the extended prefix beside it: the
/// arrow cluster shares numbers with the keypad, so a comparison that ignored the
/// prefix would refuse a key nobody named.
const THE_SAME_NUMBER_EXTENDED: (u16, bool) = (0x31, true);

/// A machine reading its keyboards, with the chord published on it.
///
/// Run to a standstill first, because installing the answer is part of the run:
/// a machine nobody asked to read its keyboards has no answer to hand an event to.
fn reading() -> SimSource {
    let mut host = SimSource::new();
    host.with_chord_at(TO_THE_SINK, BACK_HERE);
    source::run(&Request::DryRun, false, &mut host, None);
    host
}

fn a_key(at: (u16, bool)) -> Arrived {
    Arrived::Key {
        at,
        packed: 0x1234,
        flags: 0,
    }
}

/// The same key let go of, in the flags a hook spells the transition with.
fn a_release(at: (u16, bool)) -> Arrived {
    Arrived::Key {
        at,
        packed: 0x1234,
        flags: i64::from(favjit_hid::scancode::LLKHF_UP),
    }
}

fn answered(host: &SimSource, arrived: Arrived) -> Answered {
    host.present(arrived)
        .expect("a run that read its keyboards has installed an answer")
}

#[test]
fn every_key_is_handed_over_including_the_ones_this_machine_will_not_get() {
    let mut host = reading();
    for refusing in [
        Suppressing::Nothing,
        Suppressing::TheSwitch,
        Suppressing::Everything,
    ] {
        host.refuse(refusing);
        let answered = answered(&host, a_key(AN_ORDINARY_KEY));
        assert_eq!(
            answered.handed_over,
            Some((0x1234, 0)),
            "refusing {refusing:?}"
        );
    }
}

#[test]
fn a_key_carries_on_while_nothing_is_refused() {
    let mut host = reading();
    host.refuse(Suppressing::Nothing);
    let answered = answered(&host, a_key(AN_ORDINARY_KEY));
    assert!(answered.carried_on);
    assert!(!answered.ended_here);
}

#[test]
fn every_key_ends_here_while_everything_is_refused() {
    let mut host = reading();
    host.refuse(Suppressing::Everything);
    let answered = answered(&host, a_key(AN_ORDINARY_KEY));
    assert!(answered.ended_here);
    assert!(!answered.carried_on);
}

// A key this machine saw go down is one it believes is down until it sees the
// release, and a hook that refuses that release leaves the key held here for as
// long as nobody presses it again — a right control the machine kept down through a
// whole trip of the keyboard, read off a trace (ADR-0013 names the hazard for the
// sink; this is the same one on this side). So the one thing let through while
// everything is refused is the release of a key this machine still holds.

#[test]
fn the_release_of_a_key_this_machine_saw_go_down_carries_on_while_everything_is_refused() {
    let mut host = reading();
    host.refuse(Suppressing::Nothing);
    assert!(answered(&host, a_key(AN_ORDINARY_KEY)).carried_on);

    host.refuse(Suppressing::Everything);
    let answered = answered(&host, a_release(AN_ORDINARY_KEY));
    assert!(
        answered.carried_on,
        "the release the machine is waiting for"
    );
    assert!(!answered.ended_here);
    assert_eq!(
        answered.handed_over,
        Some((0x1234, i64::from(favjit_hid::scancode::LLKHF_UP))),
        "still reported, so the sink lets go of it too"
    );
}

#[test]
fn the_release_of_a_key_this_machine_never_saw_go_down_is_refused() {
    let mut host = reading();
    host.refuse(Suppressing::Everything);
    let answered = answered(&host, a_release(AN_ORDINARY_KEY));
    assert!(answered.ended_here);
    assert!(!answered.carried_on);
}

#[test]
fn a_key_let_go_of_once_is_not_let_go_of_again() {
    let mut host = reading();
    host.refuse(Suppressing::Nothing);
    answered(&host, a_key(AN_ORDINARY_KEY));
    host.refuse(Suppressing::Everything);
    assert!(answered(&host, a_release(AN_ORDINARY_KEY)).carried_on);
    assert!(
        answered(&host, a_release(AN_ORDINARY_KEY)).ended_here,
        "a second release for a key this machine no longer holds"
    );
}

#[test]
fn a_press_refused_here_leaves_nothing_for_its_release_to_let_go_of() {
    // The chord's own key: refused on the way down, so this machine never saw it
    // go down, and its release has nothing to release.
    let mut host = reading();
    host.refuse(Suppressing::TheSwitch);
    host.with_the_modifier_down(true);
    assert!(answered(&host, a_key(TO_THE_SINK)).ended_here);

    host.refuse(Suppressing::Everything);
    assert!(answered(&host, a_release(TO_THE_SINK)).ended_here);
}

#[test]
fn a_press_refused_while_everything_is_does_not_make_the_key_held_here() {
    let mut host = reading();
    host.refuse(Suppressing::Everything);
    assert!(answered(&host, a_key(AN_ORDINARY_KEY)).ended_here);
    assert!(answered(&host, a_release(AN_ORDINARY_KEY)).ended_here);
}

#[test]
fn what_is_held_here_is_the_position_and_not_the_number_alone() {
    let mut host = reading();
    host.refuse(Suppressing::Nothing);
    answered(&host, a_key(THE_SAME_NUMBER_EXTENDED));
    host.refuse(Suppressing::Everything);
    assert!(
        answered(&host, a_release((THE_SAME_NUMBER_EXTENDED.0, false))).ended_here,
        "the key on the other half of the keyboard, which this machine never saw go down"
    );
    assert!(answered(&host, a_release(THE_SAME_NUMBER_EXTENDED)).carried_on);
}

#[test]
fn only_the_chord_ends_here_while_the_switch_is_refused() {
    let mut host = reading();
    host.refuse(Suppressing::TheSwitch);
    host.with_the_modifier_down(true);
    for at in [TO_THE_SINK, BACK_HERE] {
        assert!(answered(&host, a_key(at)).ended_here, "the chord at {at:?}");
    }
    assert!(
        answered(&host, a_key(AN_ORDINARY_KEY)).carried_on,
        "a key the chord does not name"
    );
}

#[test]
fn the_chord_without_its_modifier_is_an_ordinary_key() {
    let mut host = reading();
    host.refuse(Suppressing::TheSwitch);
    host.with_the_modifier_down(false);
    let answered = answered(&host, a_key(TO_THE_SINK));
    assert!(answered.carried_on);
    assert!(!answered.ended_here);
}

#[test]
fn a_position_is_the_make_code_and_the_prefix_together() {
    let mut host = reading();
    host.refuse(Suppressing::TheSwitch);
    host.with_the_modifier_down(true);
    let answered = answered(&host, a_key(THE_SAME_NUMBER_EXTENDED));
    assert!(
        answered.carried_on,
        "a key sharing the chord's make code with the prefix beside it"
    );
}

#[test]
fn a_key_with_nowhere_to_go_is_still_refused() {
    let mut host = reading();
    host.refuse(Suppressing::Everything);
    host.with_nowhere_for_keys();
    let answered = answered(&host, a_key(AN_ORDINARY_KEY));
    assert_eq!(answered.handed_over, None);
    assert!(answered.ended_here);
}

#[test]
fn an_event_of_nobody_elses_carries_on_and_is_handed_to_nothing() {
    let mut host = reading();
    host.refuse(Suppressing::Everything);
    let answered = answered(&host, Arrived::NothingOfOurs);
    assert_eq!(answered.handed_over, None);
    assert!(answered.carried_on);
    assert!(!answered.ended_here);
}

#[test]
fn a_pointer_is_refused_and_counted_only_while_everything_is() {
    let mut host = reading();
    host.refuse(Suppressing::TheSwitch);
    let carried_on = answered(&host, Arrived::Pointer);
    assert!(carried_on.carried_on);
    assert_eq!(carried_on.pointers_refused, 0);

    host.refuse(Suppressing::Everything);
    let ended = answered(&host, Arrived::Pointer);
    assert!(ended.ended_here);
    assert_eq!(ended.pointers_refused, 1);
}

#[test]
fn the_number_the_platform_is_told_is_the_machines_own() {
    let mut host = reading();
    host.refuse(Suppressing::Everything);
    assert_eq!(answered(&host, a_key(AN_ORDINARY_KEY)).told, 1);
    host.refuse(Suppressing::Nothing);
    assert_eq!(answered(&host, a_key(AN_ORDINARY_KEY)).told, 0);
}
