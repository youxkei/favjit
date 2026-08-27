//! What a run that changes nothing outside itself shows the person.
//!
//! The mode exists to answer "what would favjit do to this keystroke", so the
//! answer is the whole of what it produces: a run whose output went into the log
//! beside the warnings would bury it, which is why saying it is a host operation
//! of its own (ADR-0006) and why what is said is asserted here rather than in
//! `warnings.rs`.
//!
//! Matched on the key each line names rather than on the phrasing. Which
//! keystroke was described is the behaviour; the wording is not, and a suite that
//! pinned the prose would make every improvement to it a failing test.

use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::{DeviceId, Key, Layout};
use favjit_host_sim::SimHost;

const KEYBOARD: DeviceId = DeviceId(1);

fn dry_run(mac: &mut SimHost) {
    sink::run(
        &Request::DryRun,
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        None,
    );
}

#[test]
fn a_dry_run_says_what_it_would_have_sent() {
    // The keystroke converted, named by what the report holds rather than by the
    // bytes: a person reading this is checking the layout, and the bytes are what
    // `--replay` is for.
    let mut mac = SimHost::new();
    mac.attach_built_in(KEYBOARD).tap(KEYBOARD, Key::K);

    dry_run(&mut mac);

    let said = mac.said().join("\n");
    assert!(
        !said.is_empty(),
        "a run that injects nothing has nothing else to show; said: {said:?}"
    );
    assert!(
        said.to_lowercase().contains('n'),
        "`k` converts to `n` under this layout; said: {said}"
    );
}

#[test]
fn a_dry_run_shows_the_key_going_down_and_coming_back_up() {
    // Both halves, because what a person is checking is a rule that can differ
    // between them: a mode that showed only the press would pass a layout that
    // never let go.
    let mut mac = SimHost::new();
    mac.attach_built_in(KEYBOARD).tap(KEYBOARD, Key::K);

    dry_run(&mut mac);

    assert!(
        mac.said().len() >= 2,
        "a press and a release; said: {:?}",
        mac.said()
    );
}

#[test]
fn a_run_that_converts_nothing_says_nothing() {
    // Nothing typed is nothing to show, so an empty answer is the honest one: a
    // mode that printed a header regardless would make "it produced no output"
    // and "it produced nothing for what I typed" read the same.
    let mut mac = SimHost::new();
    mac.attach_built_in(KEYBOARD);

    dry_run(&mut mac);

    assert_eq!(mac.said(), Vec::<String>::new());
}
