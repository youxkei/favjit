//! The two host operations that put the converter's control file in place.

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
