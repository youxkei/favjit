//! Putting favjit where Windows will start it, and taking it back out.
//!
//! A forwarding run that only exists while a terminal is open is one somebody has to
//! remember to start, and the Mac's half has not needed remembering since it became a
//! launchd daemon (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`). This is the same thing for this machine, in the shape this
//! machine has: a **logon task**, because a low-level hook is called on the thread that
//! installed it and raw input is delivered to a window — both session objects, so there
//! is no service to be
//! (`docs/platform/windows/privileges.md`, and
//! `docs/platform/windows/logon-tasks.md` for what a task takes to start a program
//! invisibly and what its settings are wrong about by default).
//!
//! **The task runs the watchdog, not favjit**, for the reason launchd is given favjit's
//! supervisor rather than favjit: suppression must not outlive the ability to process
//! input (ADR-0008), and only the parent can end a wedged child. The task's own restart
//! is then what brings the pair back, which is the job launchd's `KeepAlive` does there.
//!
//! **Nothing here asks for a privilege.** The task runs as the person, its files are
//! under their profile, and every mode favjit has runs unelevated
//! (`docs/platform/windows/privileges.md`). That is the whole difference in shape from
//! the Mac's half, which needs root twice over.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use favjit_host_windows::link;
use log::{error, info, warn};

/// The task that forwards, and the task that draws the tray item.
///
/// Two, mirroring the Mac's daemon and agent: they fail independently, they are stopped
/// independently, and a person who wants the item without the forwarding — or the other
/// way round — has that without a flag being invented for it.
pub const TASK: &str = "favjit";
pub const TRAY_TASK: &str = "favjit-tray";

/// Where the binaries are copied to.
///
/// Copied rather than pointed at where they were built, which is the other machine's
/// reason as well (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`):
/// a task naming a path inside `target/` stops working the first time somebody runs
/// `cargo clean`, and what it does then is nothing, quietly.
fn bin_directory() -> PathBuf {
    link::favjit_directory().join("bin")
}

/// Where the pair writes what it has to say.
///
/// One file for both, because the only question ever asked of it is why the keys stopped
/// arriving, and answering that means reading what the supervisor saw beside what the run
/// was doing at the time. A task's child has no console behind its stderr, so without a
/// file named here the whole of favjit's log is discarded by the system.
fn log_path() -> PathBuf {
    link::favjit_directory().join("favjit.log")
}

/// The three binaries the pair is made of.
const BINARIES: [&str; 3] = ["favjit.exe", "favjit-watchdog.exe", "favjit-tray.exe"];

pub fn install() -> i32 {
    let Some(from) = beside_this_one() else {
        error!("cannot tell where this binary is, so there is nothing to copy from");
        return 1;
    };

    let into = bin_directory();
    if let Err(error) = std::fs::create_dir_all(&into) {
        error!("cannot make {}: {error}", into.display());
        return 1;
    }

    // Ended before anything is copied, because a running binary cannot be copied over: an
    // install on top of an install is the ordinary case, and the file being locked is what
    // it looks like. After the directory exists, so that the commands below have somewhere
    // local to be started from.
    for task in [TASK, TRAY_TASK] {
        end(task);
    }
    for binary in BINARIES {
        let source = from.join(binary);
        let target = into.join(binary);
        if let Err(error) = copy_once_it_is_free(&source, &target) {
            error!(
                "cannot copy {} to {}: {error}",
                source.display(),
                target.display()
            );
            if !source.exists() {
                error!("{binary} is not beside this one; build the workspace before installing it");
            } else {
                error!(
                    "if a favjit started by hand is still running, stop it and run this again — \
                     `--install` only ends the ones it registered"
                );
            }
            return 1;
        }
    }
    info!("copied {} binaries into {}", BINARIES.len(), into.display());

    let watchdog = into.join("favjit-watchdog.exe");
    let favjit = into.join("favjit.exe");
    let log = log_path();
    // `--dry-run false` spelled out, because it is the whole of what an installed run is
    // for and the default is deliberately the other one. It is not a run that moves the
    // keyboard on its own: it comes up sending nothing and waits for the chord, which is
    // what makes it something to have started at every logon (ADR-0013).
    let arguments = format!(
        "--log \"{}\" -- \"{}\" --dry-run false",
        log.display(),
        favjit.display()
    );
    let forwarding = task(
        &watchdog.display().to_string(),
        &arguments,
        "favjit: forwards this machine's keyboard and mouse to the Mac, under its watchdog",
    );
    let tray = task(
        &into.join("favjit-tray.exe").display().to_string(),
        "",
        "favjit: the tray item that says where the keyboard is",
    );

    for (name, xml) in [(TASK, forwarding), (TRAY_TASK, tray)] {
        if !register(name, &xml) {
            return 1;
        }
    }

    // Started here rather than left for the next logon, because an install on top of an
    // install is the ordinary case and what it is for is to be running the new binaries:
    // registering a task does not start it, so without this a re-install leaves the
    // machine with nothing forwarding until somebody logs out.
    //
    // Safe to start unasked because a run comes up with the keyboard on the machine it is
    // running on (ADR-0013): what this puts back is a favjit refusing one chord, not one
    // that has taken the keyboard away.
    for name in [TASK, TRAY_TASK] {
        match schtasks(&["/run", "/tn", name]) {
            Some(0) => info!("started the {name} task"),
            // Said rather than failed on: the task is registered either way, so the next
            // logon brings it up even when starting it now did not.
            _ => warn!("the {name} task is registered but would not start now"),
        }
    }

    info!(
        "installed and running. It comes up again at every logon, and {} is where both it \
         and the run it supervises write",
        log.display()
    );
    0
}

/// Copy over a file that may still be held by a process on its way out.
///
/// `schtasks /end` returns before the process it ended has gone, so the copy that follows
/// it can land while the file is still open — which is a re-install failing on a race
/// rather than on anything being wrong. Retried for as long as a process takes to leave,
/// and no longer: a file still held after that is held by something this did not end, and
/// waiting on it would be waiting for a person.
fn copy_once_it_is_free(source: &Path, target: &Path) -> std::io::Result<()> {
    /// Long enough for a process asked to stop to have stopped, short enough that a file
    /// held by something else is reported while the person is still watching.
    const GIVE_UP_AFTER: Duration = Duration::from_secs(3);
    const BETWEEN_TRIES: Duration = Duration::from_millis(100);

    let started = Instant::now();
    loop {
        match std::fs::copy(source, target) {
            Ok(_) => return Ok(()),
            // The source not being there is not something waiting fixes.
            Err(error) if !source.exists() => return Err(error),
            Err(error) if started.elapsed() >= GIVE_UP_AFTER => return Err(error),
            Err(_) => std::thread::sleep(BETWEEN_TRIES),
        }
    }
}

pub fn uninstall() -> i32 {
    let mut ok = true;
    for name in [TASK, TRAY_TASK] {
        end(name);
        let deleted = schtasks(&["/delete", "/tn", name, "/f"]);
        match deleted {
            Some(0) => info!("removed the {name} task"),
            // Not a failure worth stopping for: a task that is not there is the state
            // this was asked to reach.
            Some(code) => {
                warn!("the {name} task was not removed ({code}); it may not have existed")
            }
            None => ok = false,
        }
    }

    // The binaries after the tasks, so nothing is left pointing at files that have gone.
    let bin = bin_directory();
    for binary in BINARIES {
        let path = bin.join(binary);
        if path.exists() {
            if let Err(error) = std::fs::remove_file(&path) {
                warn!("cannot remove {}: {error}", path.display());
                ok = false;
            }
        }
    }
    // Only if it is empty, and unreported if not: what else is in there is the identity
    // and the log, which an uninstall has no business taking with it — the identity is
    // what the Mac has paired, and pairing again is not free.
    let _ = std::fs::remove_dir(&bin);

    if ok {
        info!(
            "uninstalled. {} and the identity beside it are left where they are",
            log_path().display()
        );
        0
    } else {
        1
    }
}

/// The directory this binary is in, which is where the others were built too.
fn beside_this_one() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// A task definition.
///
/// XML rather than `schtasks /create`'s own flags, because the flags cannot say three
/// things this task needs and two of them are wrong by default: a task stops after three
/// days unless `ExecutionTimeLimit` says otherwise, it does not start on battery and stops
/// when a machine goes onto one, and there is no flag at all for restarting a task that
/// failed — which is the whole of what makes this the Mac's `KeepAlive`.
fn task(command: &str, arguments: &str, description: &str) -> String {
    let user = user();
    let arguments = match arguments {
        "" => String::new(),
        given => format!("\n      <Arguments>{}</Arguments>", escape(given)),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{description}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>{arguments}
    </Exec>
  </Actions>
</Task>
"#,
        escape(command)
    )
}

/// Who the task runs as.
///
/// The person at this session, because that is who favjit is for: the keyboard it takes
/// is theirs, the identity it authenticates with is under their profile, and nothing it
/// does wants more than they have.
fn user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Hand one definition to Task Scheduler.
fn register(name: &str, xml: &str) -> bool {
    // Beside the binaries rather than in a scratch directory, so that a failure leaves
    // the definition that failed where it can be read.
    let path = link::favjit_directory().join(format!("{name}.xml"));
    // UTF-16 with a byte order mark, which is what the file says it is in its own
    // declaration and what `schtasks /xml` reads: handed UTF-8, it answers that the XML
    // is malformed and says nothing about the encoding.
    if let Err(error) = std::fs::write(&path, utf16le(xml)) {
        error!("cannot write {}: {error}", path.display());
        return false;
    }
    match schtasks(&[
        "/create",
        "/xml",
        &path.display().to_string(),
        "/tn",
        name,
        "/f",
    ]) {
        Some(0) => {
            info!("registered the {name} task, to start at logon");
            let _ = std::fs::remove_file(&path);
            true
        }
        Some(code) => {
            error!(
                "Task Scheduler refused the {name} task ({code}); the definition is at {}",
                path.display()
            );
            false
        }
        None => false,
    }
}

/// Stop a task that is running, and say nothing if it was not.
fn end(name: &str) {
    let _ = schtasks(&["/end", "/tn", name]);
}

/// Run `schtasks`, and say what it exited with.
///
/// `None` when it could not be run at all, which is a different thing from a task it
/// would not create: one is a machine without Task Scheduler and the other is an answer
/// about this task.
fn schtasks(arguments: &[&str]) -> Option<i32> {
    // Started somewhere local on purpose. An install run from a build directory on a
    // network path hands its children that path as their working directory, and a Windows
    // command line started in one refuses it — which would read as Task Scheduler
    // declining the task rather than as the directory the installer happened to be in.
    let mut command = Command::new("schtasks.exe");
    command.args(arguments);
    let here = link::favjit_directory();
    if here.is_dir() {
        command.current_dir(&here);
    }
    match command.status() {
        Ok(status) => Some(status.code().unwrap_or(1)),
        Err(error) => {
            error!("cannot run schtasks.exe: {error}");
            None
        }
    }
}

/// The bytes of a UTF-16 file, byte order mark and all.
fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_says_it_is_utf16_and_is() {
        let bytes = utf16le(&task("C:\\favjit.exe", "--dry-run false", "a description"));
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "no byte order mark");
        // Every code unit is two bytes, so an odd length is a mixed-encoding file — which
        // is what `schtasks /xml` reports only as malformed XML.
        assert_eq!(bytes.len() % 2, 0);
    }

    #[test]
    fn a_task_with_no_arguments_has_no_arguments_element() {
        let xml = task("C:\\favjit-tray.exe", "", "a description");
        assert!(!xml.contains("<Arguments>"), "{xml}");
        assert!(
            xml.contains("<Command>C:\\favjit-tray.exe</Command>"),
            "{xml}"
        );
    }

    #[test]
    fn the_settings_that_are_wrong_by_default_are_all_said() {
        let xml = task("C:\\favjit-watchdog.exe", "-- favjit.exe", "a description");
        // Each of these is a run that stops on its own without them: after three days,
        // on battery, and after a wedge nothing restarted.
        for setting in [
            "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<RestartOnFailure>",
        ] {
            assert!(xml.contains(setting), "{setting} missing from {xml}");
        }
    }

    #[test]
    fn a_path_with_an_ampersand_in_it_does_not_end_the_element() {
        let xml = task("C:\\a & b\\favjit.exe", "", "a description");
        assert!(xml.contains("C:\\a &amp; b\\favjit.exe"), "{xml}");
    }
}
