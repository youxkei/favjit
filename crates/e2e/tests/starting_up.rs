//! What a run brings up before it takes a keyboard, and in what order.
//!
//! The order is not cosmetic. Suppression must never outlive the ability to process
//! input (ADR-0008), so every assertion here about what was *not* taken is that
//! rule: a keyboard held with nowhere to send its keystrokes is not "favjit stopped
//! converting" but "the keyboard stopped working".

use favjit_core::sink::{self, Ending, Request};
use favjit_core::{DeviceId, DeviceInfo, Key, Layout};
use favjit_host_sim::{Did, SimHost};

fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// The same, accepting the other machine's input as well.
///
/// A variant of the injecting one rather than a request of its own, because
/// listening is what a delivering run does as well: there is no shape for accepting
/// input with nowhere to send it.
fn listening() -> Request {
    Request::Injecting { listen: true }
}

fn run(mac: &mut SimHost, request: &Request) -> Ending {
    sink::run(request, Layout::dudrack(), None, mac, None)
}

#[test]
fn a_run_that_is_switched_off_waits_and_takes_nothing() {
    // The whole of what "off" means: nothing is captured and nothing is seized, so
    // the keyboards are the machine's own. The run waits rather than exiting at
    // once, because whatever supervises it would restart a process that exited —
    // and it ends when the switch comes back rather than carrying on, so the
    // keyboards are taken by a run that starts from nothing.
    let mut mac = SimHost::new().with_converting_off();
    mac.attach(DeviceInfo::built_in(DeviceId(1)))
        .tap(DeviceId(1), Key::K);

    assert_eq!(run(&mut mac, &converting()), Ending::SwitchedOff);
    assert_eq!(mac.did(), vec![Did::AskedIfSwitchedOn, Did::WaitedForOn]);
    assert_eq!(mac.took_input(), None);
    assert_eq!(mac.injected(), Vec::new());
}

#[test]
fn the_switch_is_asked_about_before_anything_else() {
    // Before the permission and before the output, because a machine that is
    // switched off should be asked for nothing at all.
    let mut mac = SimHost::new();

    run(&mut mac, &converting());

    assert_eq!(mac.did().first(), Some(&Did::AskedIfSwitchedOn));
}

#[test]
fn nothing_is_taken_when_the_output_will_not_come_up() {
    // The output comes up first so that this failure happens before anything is
    // taken: the keyboards stay the machine's own, and what stopped working is
    // favjit.
    let mut mac = SimHost::new().with_no_output();
    mac.attach(DeviceInfo::built_in(DeviceId(1)))
        .tap(DeviceId(1), Key::K);

    assert_eq!(run(&mut mac, &converting()), Ending::NoOutput);
    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput
        ]
    );
    assert_eq!(mac.took_input(), None, "no keyboard was taken");
    assert_eq!(mac.injected(), Vec::new());
}

#[test]
fn a_run_that_may_not_read_input_takes_nothing_and_opens_nothing() {
    // Refused before the output, because a converter that cannot read the keyboards
    // has nothing to convert — and bringing up a virtual keyboard first would leave
    // a device behind for a run that never happened.
    let mut mac = SimHost::new().with_no_permission();

    assert_eq!(run(&mut mac, &converting()), Ending::NoPermission);
    assert_eq!(
        mac.did(),
        vec![Did::AskedIfSwitchedOn, Did::AskedPermission]
    );
    assert_eq!(mac.took_input(), None);
}

#[test]
fn a_machine_that_refuses_its_keyboards_ends_the_run_there() {
    // The output is already up by then, and it is let go with the process: nothing
    // is left holding a device for a run that converts nothing.
    let mut mac = SimHost::new().with_no_input();
    mac.attach(DeviceInfo::built_in(DeviceId(1)))
        .tap(DeviceId(1), Key::K);

    assert_eq!(run(&mut mac, &converting()), Ending::NoInput);
    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput,
            Did::TunedOutput,
            Did::TookInput { suppressing: true },
        ]
    );
    assert_eq!(mac.injected(), Vec::new());
}

#[test]
fn the_output_is_up_and_tuned_before_a_keyboard_is_taken() {
    // The tuning belongs to the output device, so there is no service to carry it
    // until that device exists — and it is set on every run rather than once at
    // install, because it outlasts the process that set it.
    let mut mac = SimHost::new();

    run(&mut mac, &converting());

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput,
            Did::TunedOutput,
            Did::TookInput { suppressing: true },
        ]
    );
}

#[test]
fn the_link_is_opened_after_the_output() {
    // A link that let input in from the other machine before there was anywhere to
    // send it would convert keystrokes into nothing.
    let mut mac = SimHost::new();

    run(&mut mac, &listening());

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput,
            Did::TunedOutput,
            Did::BoundLink,
            Did::StartedTheLink,
            Did::TookInput { suppressing: true },
        ]
    );
}

#[test]
fn a_socket_that_cannot_be_opened_leaves_nothing_to_turn() {
    // Nothing is started, because there is nothing for it to serve: a loop turning
    // over an unbound link would be a link that reports every connection as failed
    // for as long as the run lasts.
    let mut mac = SimHost::new().with_no_link_socket();

    let ending = run(&mut mac, &listening());

    assert!(!mac.did().contains(&Did::StartedTheLink));
    assert_eq!(
        ending,
        Ending::Converted,
        "the keyboards in front of the person do not depend on the link"
    );
    assert_eq!(mac.took_input(), Some(true));
}

#[test]
fn a_link_nothing_can_turn_is_a_run_that_still_converts() {
    // The socket goes with the loop: it was bound and handed over, and a machine
    // that cannot turn the loop drops what it was handed rather than leaving a
    // socket that accepts a source and never answers it.
    let mut mac = SimHost::new().with_nothing_to_run_alongside();

    let ending = run(&mut mac, &listening());

    assert_eq!(
        mac.advertisements(),
        0,
        "nothing turned, so nothing said so"
    );
    assert_eq!(ending, Ending::Converted);
    assert_eq!(mac.took_input(), Some(true));
}

#[test]
fn the_link_says_this_machine_is_here_before_it_waits_for_anything() {
    // Nothing can connect to a machine it cannot find, so the advertisement is not
    // something to get round to after the first connection.
    let mut mac = SimHost::new();

    run(&mut mac, &listening());

    assert_eq!(mac.advertisements(), 1);
}

#[test]
fn a_dry_run_opens_no_output_and_no_link_and_takes_nothing_exclusively() {
    // What makes a dry run safe to start: it leaves neither a virtual keyboard nor
    // an open socket behind, and the keyboards stay the machine's own. There is
    // nothing to ask for beyond the mode, which is the point — suppressing and
    // listening are what a *delivering* run does, so a dry run has no way to be
    // asked for either of them.
    let mut mac = SimHost::new();

    run(&mut mac, &Request::DryRun);

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::TookInput { suppressing: false }
        ]
    );
}
