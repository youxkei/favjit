//! The two host operations that put the converter's control file in place, and
//! where the file they put it at is.
//!
//! Where it is has to be one answer, because more than one program reads it: the
//! daemon and the installer run as root, the menu runs as the person, and a
//! spelling that differed between them would be an off switch that turns off a
//! file nobody is looking at.

use std::path::{Path, PathBuf};

use favjit_host_sim::{ControlCall, SimControl};

#[test]
fn the_directory_is_made_before_the_file_is_written() {
    let mut host = SimControl::new();

    favjit_engine::control::disable(&mut host).expect("disabled");

    assert_eq!(
        host.calls(),
        [ControlCall::MadeDirectory, ControlCall::WroteDisabled]
    );
}

#[test]
fn a_directory_failure_stops_before_the_write() {
    let mut host = SimControl::new().that_fails_at(ControlCall::MadeDirectory);

    assert!(favjit_engine::control::disable(&mut host).is_err());
    assert_eq!(host.calls(), [ControlCall::MadeDirectory]);
}

#[test]
fn the_home_a_sudo_user_came_from_wins_over_the_process_own() {
    // The daemon and the installer run as root, and root's home is not where a
    // person's menu can reach: a run that took its own home would write the off
    // switch somewhere the thing that reads it never looks.
    let found = favjit_engine::control::console_home(
        Some(String::from("someone")),
        Some(PathBuf::from("/var/root")),
    );

    assert_eq!(found, Some(PathBuf::from("/Users/someone")));
}

#[test]
fn a_process_with_no_sudo_user_behind_it_uses_its_own_home() {
    // The menu itself, which nobody ran under `sudo`: its own home is the
    // person's, so there is nothing to prefer over it.
    let found = favjit_engine::control::console_home(None, Some(PathBuf::from("/Users/someone")));

    assert_eq!(found, Some(PathBuf::from("/Users/someone")));
}

#[test]
fn a_process_given_neither_home_has_nowhere_to_look() {
    // Answered as nothing rather than guessed at: a path made up here would be a
    // file written where nothing reads it, which reads to a person as an off
    // switch that did nothing.
    assert_eq!(favjit_engine::control::console_home(None, None), None);
}

#[test]
fn the_file_sits_under_that_home_where_every_program_reads_it() {
    // Spelled once, since the daemon, the installer and the menu all have to
    // arrive at the same path: this case is what says they do.
    assert_eq!(
        favjit_engine::control::path(Path::new("/Users/someone")),
        PathBuf::from("/Users/someone/Library/Application Support/favjit/disabled")
    );
}
