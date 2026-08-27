//! Turning favjit off and on again, run as the real binary.
//!
//! Off has to mean the keyboards are the machine's own — a converter that stopped
//! converting while still holding them would be the failure ADR-0008 rules out. So
//! what is asserted here is that nothing is captured at all, and these run the
//! process rather than the loop: which devices get opened is a property of the
//! binary's startup, and the simulated suite (ADR-0007) cannot see it.
//!
//! Nothing here needs privilege or a device, because the point of the off state is
//! that it touches neither.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Paths of this test's own, so two tests cannot turn each other off.
fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("favjit-toggle-{name}-{}", std::process::id()))
}

/// The log goes to a file rather than a pipe, so it can be read while the process
/// is still running.
///
/// That is what makes these tests sequenced rather than timed: a control file
/// removed before favjit has looked at it leaves it converting, with no deadline
/// and nothing to end the run, so a test that slept instead of waiting for the log
/// would hang exactly when it was wrong.
fn favjit(log: &Path) -> Command {
    let file = std::fs::File::create(log).expect("create the log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_favjit"));
    command.env("RUST_LOG", "info");
    command.stdout(Stdio::null());
    command.stderr(Stdio::from(file));
    command
}

fn log_of(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Wait until the log says this, or fail.
fn wait_for_log(path: &Path, needle: &str, within: Duration) {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if log_of(path).contains(needle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "the log never said {needle:?} within {within:?}; it said: {}",
        log_of(path)
    );
}

/// Wait for the process to end, or kill it and fail.
fn finish(mut child: Child, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => return,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("favjit was still running after {within:?}");
            }
        }
    }
}

#[test]
fn off_holds_no_keyboard_and_ends_when_turned_back_on() {
    let control = scratch("waits");
    let log = scratch("waits.log");
    std::fs::write(&control, b"").expect("write the control file");

    let mut child = favjit(&log)
        .args(["--control", &control.display().to_string()])
        .spawn()
        .expect("favjit runs");

    wait_for_log(&log, "off", Duration::from_secs(5));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the process has to stay put: launchd would restart one that exited"
    );
    assert!(
        !log_of(&log).contains("watching device"),
        "off means no keyboard is opened at all; said: {}",
        log_of(&log)
    );

    // Turning it back on ends the run rather than taking the keyboards from where
    // it stands: seizing devices it had already declined would need capture to be
    // started a second time, and exiting hands that to whatever started it.
    std::fs::remove_file(&control).expect("remove the control file");
    finish(child, Duration::from_secs(5));
    assert!(
        log_of(&log).contains("this run is over"),
        "it should say the control file went away; said: {}",
        log_of(&log)
    );

    let _ = std::fs::remove_file(&log);
}

#[test]
fn disable_writes_the_control_file_and_enable_removes_it() {
    let control = scratch("flags");
    let log = scratch("flags.log");
    let _ = std::fs::remove_file(&control);

    let status = favjit(&log)
        .args(["--disable", "--control", &control.display().to_string()])
        .status()
        .expect("favjit runs");
    assert!(status.success());
    assert!(control.exists(), "--disable writes the control file");

    let status = favjit(&log)
        .args(["--enable", "--control", &control.display().to_string()])
        .status()
        .expect("favjit runs");
    assert!(status.success());
    assert!(!control.exists(), "--enable removes it");

    // Twice, because a menu will do that: the second one is a person pressing the
    // same item again, not an error.
    let status = favjit(&log)
        .args(["--enable", "--control", &control.display().to_string()])
        .status()
        .expect("favjit runs");
    assert!(
        status.success(),
        "enabling what is already on is not a failure"
    );

    let _ = std::fs::remove_file(&log);
}

#[test]
fn a_term_is_answered_by_letting_go_rather_than_by_dying_where_it_stands() {
    // The watchdog asks with SIGTERM before it insists with SIGKILL, and the ask is
    // the only chance to put the virtual keyboard back: a key down when the kill
    // lands stays down, because that device belongs to a daemon and outlives this
    // process. A stuck modifier is worse than a dead keyboard — the dead one is
    // obvious.
    //
    // Run without output here, so what is asserted is the shape of the shutdown and
    // not the device: no privilege, nothing seized, nothing typed.
    let control = scratch("term");
    let log = scratch("term.log");
    let _ = std::fs::remove_file(&control);
    std::fs::write(&control, b"").expect("write the control file");

    let mut child = favjit(&log)
        .args(["--control", &control.display().to_string()])
        .spawn()
        .expect("favjit runs");
    wait_for_log(&log, "off", Duration::from_secs(5));

    unsafe { kill(child.id() as i32, 15) };
    finish(child_of(&mut child), Duration::from_secs(3));

    assert!(
        log_of(&log).contains("asked to stop"),
        "it should say it was asked, so a log after a kill says which happened; said: {}",
        log_of(&log)
    );

    let _ = std::fs::remove_file(&control);
    let _ = std::fs::remove_file(&log);
}

/// Take the child out of the borrow, so `finish` can own it.
fn child_of(child: &mut Child) -> Child {
    std::mem::replace(child, Command::new("/usr/bin/true").spawn().expect("true"))
}

extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[test]
fn status_says_whether_it_is_converting_and_needs_no_privilege() {
    // What a menu draws its checkmark from, so it has to answer without a password
    // and without a running daemon.
    let control = scratch("status");
    let log = scratch("status.log");
    let _ = std::fs::remove_file(&control);

    let output = Command::new(env!("CARGO_BIN_EXE_favjit"))
        .args(["--status", "--control", &control.display().to_string()])
        .output()
        .expect("favjit runs");
    assert!(output.status.success());
    let said = String::from_utf8_lossy(&output.stdout).to_lowercase();
    assert!(
        said.contains("converting: on"),
        "with no control file it is on; said: {said}"
    );

    std::fs::write(&control, b"").expect("write the control file");
    let output = Command::new(env!("CARGO_BIN_EXE_favjit"))
        .args(["--status", "--control", &control.display().to_string()])
        .output()
        .expect("favjit runs");
    let said = String::from_utf8_lossy(&output.stdout).to_lowercase();
    assert!(
        said.contains("converting: off"),
        "with one it is off; said: {said}"
    );
    assert!(
        said.contains("installed:"),
        "a menu also needs to know whether it is installed at all; said: {said}"
    );

    let _ = std::fs::remove_file(&control);
    let _ = std::fs::remove_file(&log);
}

#[test]
fn installing_without_privilege_refuses_rather_than_half_doing_it() {
    // Run as an ordinary user, which is how it will be run by mistake. Refusing
    // early is the difference between a message and a job that launchd cannot
    // start for reasons that are never logged.
    let log = scratch("install.log");
    let status = favjit(&log).arg("--install").status().expect("favjit runs");

    assert!(!status.success());
    assert!(
        log_of(&log).contains("root"),
        "it should say what is missing; said: {}",
        log_of(&log)
    );

    let _ = std::fs::remove_file(&log);
}
