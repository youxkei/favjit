//! How long the link lasts, and what the run does when it stops being served.
//!
//! Two failures pull in opposite directions. A link that gave up on the first
//! connection it could not use would be a Windows keyboard that stops working
//! because something scanned the Mac. A link that never gave up would spin on a
//! socket that has stopped answering, holding the port with nothing being served.
//!
//! And when it does stop, the run ends. Nothing rebinds the socket inside a run and
//! the advertisement goes with it, so a converter that stayed up would look well
//! here while being unreachable from the other machine (ADR-0017) — ending is what
//! lets whatever supervises favjit open the socket again (ADR-0008).

use favjit_core::link::FAILURES;
use favjit_core::pairing::Authorized;
use favjit_core::sink::{self, Ending, Request};
use favjit_core::{DeviceId, DeviceInfo, Injected, Key, Layout, ModifierKeys as M};
use favjit_host_sim::{SimHost, SimLink};

fn key(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

const PAIRED: u8 = 0xaa;
const REMOTE: DeviceId = DeviceId(7);

fn paired_list() -> String {
    Authorized::added("", &key(PAIRED))
}

fn converting() -> Request {
    Request::Injecting { listen: true }
}

/// A whole run, and how it ended, with that script on the other end of its link.
fn served(script: impl FnOnce(&mut SimLink)) -> (SimHost, Ending) {
    let mut link = SimLink::new(paired_list());
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
    let ending = sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    (mac, ending)
}

/// One keystroke over the link, for a script that wants the link to have worked.
fn types(link: &mut SimLink) {
    link.connect(key(PAIRED))
        .attach(DeviceInfo::external(REMOTE, 1, 2))
        .press(REMOTE, Key::K)
        .release(REMOTE, Key::K)
        .hang_up();
}

#[test]
fn a_link_that_is_serving_does_not_end_the_run() {
    // The ordinary case, and the one the others are worth measuring against: a run
    // ends because it was asked to, not because it has a link.
    let (mac, ending) = served(types);

    assert_eq!(ending, Ending::Converted);
    assert!(!mac.injected().is_empty());
}

#[test]
fn a_connection_that_cannot_be_used_does_not_end_the_link() {
    // What a port scan produces, and what a machine on the same desk being switched
    // off produces. The source that connects afterwards has to get in.
    let (mac, ending) = served(|link| {
        link.rejected_times(FAILURES - 1);
        types(link);
    });

    assert_eq!(ending, Ending::Converted);
    assert!(
        !mac.injected().is_empty(),
        "the source after them was served"
    );
}

#[test]
fn a_connection_that_worked_sets_the_count_back() {
    // Otherwise a run left going for a week would end on its own, having collected
    // enough unusable connections between working ones to reach the bound.
    let (mac, ending) = served(|link| {
        link.rejected_times(FAILURES - 1);
        types(link);
        link.rejected_times(FAILURES - 1);
    });

    assert_eq!(ending, Ending::Converted);
    assert!(!mac.injected().is_empty());
}

#[test]
fn enough_of_them_in_a_row_and_the_link_gives_up() {
    // A socket that fails every call would otherwise be a loop turning at full
    // speed for the rest of the run, with nothing being served at the end of it.
    let (mac, ending) = served(|link| {
        link.rejected_times(FAILURES);
        types(link);
    });

    assert_eq!(ending, Ending::LinkGone);
    assert_eq!(
        mac.injected(),
        Vec::new(),
        "it gave up before the source that came after them"
    );
}

#[test]
fn a_listener_that_is_gone_ends_the_run() {
    // The socket itself, rather than a connection on it: there is nothing left to
    // accept on, and no run rebinds one.
    let (_, ending) = served(|link| {
        link.listener_gone();
    });

    assert_eq!(ending, Ending::LinkGone);
}

#[test]
fn what_the_link_delivered_before_it_went_is_converted_first() {
    // The keystrokes happened. A run that ended the moment its link did would drop
    // the last of them — and the release of a held key is the one that matters,
    // since a shift left down is down in every application (ADR-0002).
    let (mac, ending) = served(|link| {
        link.connect(key(PAIRED))
            .attach(DeviceInfo::external(REMOTE, 1, 2))
            .press(REMOTE, Key::LeftShift)
            .hang_up();
        link.listener_gone();
    });

    assert_eq!(ending, Ending::LinkGone);
    assert_eq!(
        mac.injected(),
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

#[test]
fn a_run_that_never_listened_is_not_ended_by_a_link() {
    // There is no loop turning alongside this one, so there is nothing to come back
    // and nothing to report — a dry run must not end as though its link had gone.
    let mut mac = SimHost::new();
    mac.attach(DeviceInfo::built_in(DeviceId(1)))
        .tap(DeviceId(1), Key::K);

    let ending = sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        None,
        &mut mac,
        None,
    );

    assert_eq!(ending, Ending::Converted);
}
