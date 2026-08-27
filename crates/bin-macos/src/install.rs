//! Putting favjit where launchd can run it, and taking it back out.
//!
//! A converter that only runs while a terminal is open is not one you can type
//! on, and the privilege it needs is root, which on macOS means a launchd daemon
//! (`docs/platform/macos/input-permissions.md`).
//!
//! launchd runs the **watchdog**, not favjit: suppression must not outlive the
//! ability to process input (ADR-0008), and the supervisor has to be the parent to
//! kill a wedged child. So the job is the watchdog with favjit as its argument,
//! and launchd's own restart is what brings the pair back afterwards.

use std::path::{Path, PathBuf};

use favjit_host_macos::control;
use log::{error, info, warn};

/// The launchd job's name, and the file it lives in.
pub const LABEL: &str = "dev.youxkei.favjit.daemon";

/// The menu bar item's job, which is a second one.
///
/// Separate because it is a different domain and a different user: the converter is
/// a root daemon with no session, and a menu bar item exists only inside a login
/// session's window server. One job cannot be both.
pub const MENU_LABEL: &str = "dev.youxkei.favjit.menu";

/// Where launchd is told to look.
///
/// A plist anywhere else is refused: launchd's system domain wants it root-owned
/// in a directory only root can write, and a plist in a scratch directory fails as
/// `Bootstrap failed: 5` with no job created and nothing logged
/// (`docs/platform/macos/input-permissions.md`).
fn plist_path() -> PathBuf {
    PathBuf::from("/Library/LaunchDaemons").join(format!("{LABEL}.plist"))
}

/// Where launchd is told to look for the menu.
///
/// `/Library/LaunchAgents` rather than the person's own `~/Library/LaunchAgents`,
/// so that one `sudo favjit --install` covers whoever logs in — and so that the
/// agent's plist is root-owned like the daemon's, out of reach of the session it
/// exists to rescue.
fn agent_plist_path() -> PathBuf {
    PathBuf::from("/Library/LaunchAgents").join(format!("{MENU_LABEL}.plist"))
}

/// The code identity everything favjit does runs under.
///
/// Everything hangs off this one string. macOS records the input permissions against
/// a code identity, and the identity of a bare binary is nothing a person can be
/// asked about — so the binaries go inside a bundle with this identifier, signed as
/// a whole, and the grant that covers one of them covers all of them
/// (`docs/platform/macos/input-permissions.md`).
pub const BUNDLE_ID: &str = "dev.youxkei.favjit";

/// The bundle the binaries live in.
///
/// `/Applications` because that is where an application goes, and because favjit is
/// opened as one: asking for the permissions means a process with a login session,
/// which means Launch Services opening a bundle rather than launchd starting a
/// binary.
fn bundle() -> PathBuf {
    PathBuf::from("/Applications/favjit.app")
}

/// Where the binaries are copied to.
///
/// Copied rather than pointed at where they were built: launchd would keep running
/// whatever is at the path it was given, so a job pointed into a build directory
/// changes behaviour when the directory does, and stops working when it is cleaned.
fn bundled(name: &str) -> PathBuf {
    bundle().join("Contents/MacOS").join(name)
}

fn info_plist() -> String {
    // `LSUIElement`, because the only reason this is an application at all is to have
    // a session to be prompted in. Without it, asking for permissions would put a
    // converter in the Dock and take the front window from whatever was there.
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>favjit</string>
  <key>CFBundleDisplayName</key>
  <string>favjit</string>
  <key>CFBundleIdentifier</key>
  <string>{BUNDLE_ID}</string>
  <key>CFBundleExecutable</key>
  <string>favjit</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{version}</string>
  <key>CFBundleVersion</key>
  <string>{version}</string>
  <key>LSUIElement</key>
  <true/>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
</dict>
</plist>
"#,
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// Where `favjit` goes so that typing `favjit` finds it.
///
/// A link rather than a second copy, so there is one binary and no way for the one
/// a person runs to be older than the one launchd runs. `/usr/local/bin` because it
/// is on the default `PATH` and `/usr/local/libexec` is not — and the commands that
/// matter after installing are the ones a person types.
fn on_path() -> PathBuf {
    PathBuf::from("/usr/local/bin/favjit")
}

/// The user whose home the control file lives in.
///
/// `SUDO_USER` when there is one, because the daemon runs as root and root's home
/// is not where the person's menu can reach.
pub fn console_user() -> Option<String> {
    std::env::var("SUDO_USER").ok()
}

fn plist(watchdog: &Path, favjit: &Path, control: &Path, forwarded: &[String]) -> String {
    // KeepAlive, because the watchdog exits after killing a wedged favjit and the
    // pair has to come back — launchd's own throttle is what keeps a systematically
    // broken build from thrashing. It is also what makes the control file work: a
    // change of state is favjit exiting and being started again, which is how the
    // seize is released, and the platform releasing it with the process is measured
    // (`docs/platform/macos/input-suppression.md`).
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>

  <key>ProgramArguments</key>
  <array>
    <string>{watchdog}</string>
    <string>--</string>
    <string>{favjit}</string>
    <string>--dry-run</string>
    <string>false</string>
    <string>--control</string>
    <string>{control}</string>
{forwarded}  </array>

  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>

  <key>StandardOutPath</key>
  <string>/var/log/favjit.log</string>
  <key>StandardErrorPath</key>
  <string>/var/log/favjit.log</string>
</dict>
</plist>
"#,
        watchdog = watchdog.display(),
        favjit = favjit.display(),
        control = control.display(),
        // The flags this install was given, carried into the job so that what is in
        // force is readable in the plist rather than remembered.
        forwarded = forwarded
            .iter()
            .map(|arg| format!("    <string>{arg}</string>\n"))
            .collect::<String>(),
    )
}

/// The flags the daemon is to run with, taken from this install's own arguments.
///
/// Copied through rather than interpreted here: the converter is what decides what
/// they mean, and an installer that parsed them as well would be a second place for
/// the defaults to disagree. Named one at a time rather than forwarded wholesale,
/// because `--install` is itself in this list and a daemon started with it would
/// install over itself at every launch.
fn forwarded_arguments(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for flag in ["--pointer-resolution", "--pointer-acceleration"] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            if let Some(value) = args.get(i + 1) {
                out.push(flag.to_string());
                out.push(value.clone());
            }
        }
    }
    // Only what turns something off. What a run does when told nothing is the
    // converter's own default, so an install that says nothing installs that — which
    // is what keeps the useful configuration from depending on a flag being
    // remembered at every install.
    for flag in ["--no-invert-scroll", "--no-listen"] {
        if args.iter().any(|a| a == flag) {
            out.push(flag.to_string());
        }
    }
    out
}

fn agent_plist(menu: &Path, control: &Path) -> String {
    // KeepAlive, because this is the way back when the keyboard is unusable: an
    // item that died quietly at some point during the login session would be
    // missing exactly when it is needed, and nothing else would say so.
    //
    // `LimitLoadToSessionType` Aqua so it is loaded once the window server session
    // exists. Without it launchd also starts it in the background and pre-login
    // session types, where there is no menu bar to appear in.
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{MENU_LABEL}</string>

  <key>ProgramArguments</key>
  <array>
    <string>{menu}</string>
    <string>--control</string>
    <string>{control}</string>
  </array>

  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>

  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>

  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        menu = menu.display(),
        control = control.display(),
        log = agent_log(control).display(),
    )
}

/// Where the menu's own output goes.
///
/// Beside the control file, in the person's own directory: the daemon's
/// `/var/log/favjit.log` is root-owned, and an agent that cannot open its log file
/// is one launchd refuses to start.
fn agent_log(control: &Path) -> PathBuf {
    control
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join("menu.log")
}

/// Copy the binaries in, write the plist, and start the job.
pub fn install() -> i32 {
    if !is_root() {
        error!("--install needs root: it writes to /Library/LaunchDaemons and /usr/local/libexec");
        return 1;
    }
    let Some(user) = console_user() else {
        error!(
            "--install needs to know whose menu will turn favjit off, and SUDO_USER is unset; \
             run it with sudo from your own session"
        );
        return 1;
    };
    let home = PathBuf::from("/Users").join(&user);
    let control = control::path(&home);

    let Some(here) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        error!("cannot tell where this binary is, so cannot copy it anywhere");
        return 1;
    };

    // Stopped before anything is replaced, which is what makes this the way to
    // update as well as to install: the running favjit gives the keyboards back
    // here, and the binaries about to be swapped are no longer executing.
    if launchctl(&["bootout", &format!("system/{LABEL}")]) {
        info!("stopped the running job first");
    }
    if let Some(uid) = console_uid(&user) {
        let _ = launchctl_quiet(&["bootout", &format!("gui/{uid}/{MENU_LABEL}")]);
    }

    if let Err(error) = std::fs::create_dir_all(bundle().join("Contents/MacOS")) {
        error!("cannot create {}: {error}", bundle().display());
        return 1;
    }
    for name in ["favjit", "favjit-watchdog", "favjit-menu"] {
        let from = here.join(name);
        let to = bundled(name);
        // Written beside and renamed into place rather than copied over: a rename
        // is atomic, so a run interrupted here leaves the old binary whole instead
        // of a half-written one that launchd would go on trying to start.
        let temporary = bundled(&format!("{name}.new"));
        if let Err(error) = std::fs::copy(&from, &temporary) {
            error!(
                "cannot copy {} to {}: {error}",
                from.display(),
                temporary.display()
            );
            return 1;
        }
        if let Err(error) = std::fs::rename(&temporary, &to) {
            error!("cannot put {} in place: {error}", to.display());
            let _ = std::fs::remove_file(&temporary);
            return 1;
        }
        // Root-owned and not writable by anyone else: launchd runs this as root,
        // and a binary the user could replace would be a way to run anything as
        // root.
        let _ = std::process::Command::new("/usr/sbin/chown")
            .args(["root:wheel", &to.display().to_string()])
            .status();
        let _ = std::process::Command::new("/bin/chmod")
            .args(["755", &to.display().to_string()])
            .status();
    }

    if let Err(error) = std::fs::write(bundle().join("Contents/Info.plist"), info_plist()) {
        error!("cannot write the bundle's Info.plist: {error}");
        return 1;
    }
    // Signed as one bundle, so the three binaries and the application a person opens
    // are a single code identity for the permissions to be recorded against. Ad hoc
    // — there is no Developer ID here, and none is needed: the request that matters
    // is Accessibility, and that one prompts for an ad-hoc identity too
    // (`docs/platform/macos/input-permissions.md`).
    let signed = std::process::Command::new("/usr/bin/codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--identifier",
            BUNDLE_ID,
            &bundle().display().to_string(),
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !signed {
        error!(
            "could not sign {}. Without a signature the permissions cannot be granted to it",
            bundle().display()
        );
        return 1;
    }

    // The directory the control file goes in, owned by the person who will write
    // it. Created here because a menu should not have to.
    if let Some(parent) = control.parent() {
        let _ = std::fs::create_dir_all(parent);
        let _ = std::process::Command::new("/usr/sbin/chown")
            .args([
                "-R",
                &format!("{user}:staff"),
                &parent.display().to_string(),
            ])
            .status();
    }

    let forwarded = forwarded_arguments(&std::env::args().collect::<Vec<_>>());
    let listening = !forwarded.iter().any(|a| a == "--no-listen");
    let contents = plist(
        &bundled("favjit-watchdog"),
        &bundled("favjit"),
        &control,
        &forwarded,
    );
    if let Err(error) = std::fs::write(plist_path(), contents) {
        error!("cannot write {}: {error}", plist_path().display());
        return 1;
    }
    let _ = std::process::Command::new("/usr/sbin/chown")
        .args(["root:wheel", &plist_path().display().to_string()])
        .status();
    let _ = std::process::Command::new("/bin/chmod")
        .args(["644", &plist_path().display().to_string()])
        .status();

    if !launchctl(&["bootstrap", "system", &plist_path().display().to_string()]) {
        error!(
            "launchctl refused the job. Its own message is above; the usual cause is the plist \
             or the binaries not being root-owned"
        );
        return 1;
    }

    if let Err(error) = std::fs::write(
        agent_plist_path(),
        agent_plist(&bundled("favjit-menu"), &control),
    ) {
        error!("cannot write {}: {error}", agent_plist_path().display());
        return 1;
    }
    let _ = std::process::Command::new("/usr/sbin/chown")
        .args(["root:wheel", &agent_plist_path().display().to_string()])
        .status();
    let _ = std::process::Command::new("/bin/chmod")
        .args(["644", &agent_plist_path().display().to_string()])
        .status();

    // Bootstrapped into the person's own GUI domain rather than left for the next
    // login: an install whose escape hatch only appears after a reboot is one where
    // the first wrong conversion has nothing to press.
    match console_uid(&user) {
        Some(uid)
            if launchctl(&[
                "bootstrap",
                &format!("gui/{uid}"),
                &agent_plist_path().display().to_string(),
            ]) =>
        {
            info!("the menu bar item is up");
        }
        _ => warn!(
            "installed, but could not start the menu bar item now; it comes up at the next login \
             (log: {})",
            agent_log(&control).display()
        ),
    }

    // Replaced rather than left alone, because an install over an older one has to
    // move this too: a link to a path that no longer holds the current binary is
    // worse than no link.
    let _ = std::fs::remove_file(on_path());
    if let Some(parent) = on_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::os::unix::fs::symlink(bundled("favjit"), on_path()) {
        warn!(
            "installed, but could not link {} ({error}); the commands below need the full path \
             {}",
            on_path().display(),
            bundled("favjit").display()
        );
    }

    ask_for_permissions(&user, &control);

    info!("installed and started as {LABEL}, and it starts again at boot");
    info!("nothing to launch: the square in the menu bar turns converting off and on");
    info!("the same from a terminal: favjit --status, --disable, --enable");
    info!("to update after a rebuild, or to take it out: sudo favjit --install / --uninstall");
    if listening {
        info!("the link is on; pair the other machine with sudo favjit --pair");
    } else {
        info!("input from the other machine is off, because this install asked for --no-listen");
    }
    info!(
        "logs: /var/log/favjit.log and {}",
        agent_log(&control).display()
    );
    0
}

/// What a menu needs to draw itself: whether favjit is installed, whether the job
/// is loaded, and whether it is converting.
///
/// On stdout rather than through the log, because it is what this mode exists to
/// produce and a caller may be reading it (`favjit --status | grep`).
pub fn status(path: &Path) -> i32 {
    let installed_here = plist_path().exists();
    // `launchctl print` rather than `list`: it answers for one label in the system
    // domain, where `list` needs the output parsed to find out whether the label is
    // there at all.
    let loaded = launchctl_quiet(&["print", &format!("system/{LABEL}")]);
    let menu = launchctl_quiet(&["print", &format!("gui/{}/{MENU_LABEL}", session_uid())]);

    println!("installed: {}", yes(installed_here));
    println!("job loaded: {}", yes(loaded));
    println!("menu loaded: {}", yes(menu));
    println!(
        "converting: {}",
        if control::is_converting(path) {
            "on"
        } else {
            "off"
        }
    );
    println!("control file: {}", path.display());
    0
}

fn yes(answer: bool) -> &'static str {
    if answer {
        "yes"
    } else {
        "no"
    }
}

/// Ask for the permissions favjit needs, in the person's own session.
///
/// The converter cannot ask for itself. A request from a daemon neither prompts nor
/// leaves anything to switch on, because there is no session to prompt in — so the
/// bundle is opened as an application, where the Accessibility request puts a dialog
/// on screen and an entry in the list. Granting that also grants input monitoring
/// (`docs/platform/macos/input-permissions.md`), which is what the converter needs.
///
/// Through `launchctl asuser` because the installer runs as root and has no session
/// of its own. `-W` waits for the application to finish, so the answers it writes are
/// there to be read.
fn ask_for_permissions(user: &str, control: &Path) {
    let Some(uid) = console_uid(user) else { return };
    let answers = control
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join("permissions.txt");

    if ask(uid, &answers).contains("input monitoring: Granted") {
        info!("input monitoring is granted; favjit can read the keyboards");
        return;
    }

    // Cleared and asked again, because an ad hoc signature ties the grant to the
    // binary's hash: a rebuilt favjit is a different identity, and what the previous
    // one was granted comes back as a flat refusal — with no dialog, since macOS
    // considers the question already answered. Only a Developer ID signature, which
    // would match by identifier and team instead, would survive an update
    // (`docs/platform/macos/input-permissions.md`).
    for service in ["Accessibility", "ListenEvent"] {
        let _ = std::process::Command::new("/bin/launchctl")
            .args([
                "asuser",
                &uid.to_string(),
                "/usr/bin/tccutil",
                "reset",
                service,
                BUNDLE_ID,
            ])
            .stdout(std::process::Stdio::null())
            .status();
    }

    let said = ask(uid, &answers);
    if said.contains("input monitoring: Granted") {
        info!("input monitoring is granted; favjit can read the keyboards");
        return;
    }
    info!("favjit needs permission to read the keyboards, and has just asked for it:");
    info!("  say yes to the dialog, or turn favjit on under System Settings, Privacy &");
    info!("  Security, Accessibility — granting that grants input monitoring with it");
    for line in said.lines() {
        info!("  {line}");
    }
}

/// Open the bundle as an application and collect what it says about its permissions.
fn ask(uid: u32, answers: &Path) -> String {
    let _ = std::fs::remove_file(answers);
    let opened = std::process::Command::new("/bin/launchctl")
        .args([
            "asuser",
            &uid.to_string(),
            "/usr/bin/open",
            // A new instance, because the daemon and the menu bar item run from
            // inside this bundle: without it Launch Services considers the
            // application already running, brings it to the front and drops these
            // arguments on the floor.
            "-n",
            "-a",
            &bundle().display().to_string(),
            "--args",
            "--permission-check",
            &answers.display().to_string(),
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !opened {
        warn!("could not open {} to ask", bundle().display());
        return String::new();
    }

    // Waited for by looking for the file rather than with `open -W`, which cannot
    // block on an accessory application — it says so and returns immediately.
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Ok(contents) = std::fs::read_to_string(answers) {
            return contents;
        }
    }
    String::new()
}

/// The console user's numeric id, which is the GUI domain the menu is loaded into.
///
/// `SUDO_UID` when sudo set it, and `id -u` otherwise: the name is not what
/// `launchctl` accepts, and `gui/` with a wrong number is a domain that either does
/// not exist or belongs to somebody else.
fn console_uid(user: &str) -> Option<u32> {
    if let Some(uid) = sudo_uid() {
        return Some(uid);
    }
    let output = std::process::Command::new("/usr/bin/id")
        .args(["-u", user])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// The GUI domain to ask about, from a process that may be under `sudo`.
///
/// `getuid()` alone would answer 0 there, and `gui/0` is not a session anybody is
/// looking at a menu bar in.
fn session_uid() -> u32 {
    sudo_uid().unwrap_or_else(|| unsafe { getuid() })
}

fn sudo_uid() -> Option<u32> {
    std::env::var("SUDO_UID").ok().and_then(|s| s.parse().ok())
}

/// Stop the job and take everything back out.
pub fn uninstall() -> i32 {
    if !is_root() {
        error!("--uninstall needs root: it removes from /Library/LaunchDaemons");
        return 1;
    }
    // Booting out first: removing the plist under a running job leaves it running
    // with nothing to describe it, and the keyboards seized.
    let stopped = launchctl(&["bootout", &format!("system/{LABEL}")]);
    if !stopped {
        info!("no running job to stop");
    }
    // The menu goes too. Left behind it would be an item that offers to turn a
    // converter off and on when there is no longer a converter.
    if let Some(uid) = console_user().as_deref().and_then(console_uid) {
        let _ = launchctl_quiet(&["bootout", &format!("gui/{uid}/{MENU_LABEL}")]);
    }
    let _ = std::fs::remove_file(agent_plist_path());
    let _ = std::fs::remove_file(plist_path());
    let _ = std::fs::remove_dir_all(bundle());
    // The permissions go with it. Left behind, they would be a switch in System
    // Settings for something that is no longer installed — and a grant waiting for
    // whatever next claims this identity.
    for service in ["Accessibility", "ListenEvent"] {
        let _ = std::process::Command::new("/usr/bin/tccutil")
            .args(["reset", service, BUNDLE_ID])
            .stdout(std::process::Stdio::null())
            .status();
    }
    // The link goes with what it points at, or `favjit --status` answers about a
    // binary that is no longer there.
    let _ = std::fs::remove_file(on_path());
    info!("uninstalled; the keyboards are back");
    0
}

/// Write the control file, so the running favjit gives the keyboards back.
pub fn disable(path: &Path) -> i32 {
    report(
        control::disable(path),
        path,
        "converting off; the keyboards are yours",
    )
}

/// Remove it again.
pub fn enable(path: &Path) -> i32 {
    report(control::enable(path), path, "converting on")
}

/// What the two of them have in common: an exit code and a line saying which.
///
/// The writing itself is `host-macos`'s, shared with the menu bar item rather than
/// written twice — two implementations of "off" would be two answers, and the one on
/// screen would be the one nobody checked.
fn report(outcome: std::io::Result<()>, path: &Path, said: &str) -> i32 {
    match outcome {
        Ok(()) => {
            info!("{said}");
            0
        }
        Err(error) => {
            error!("cannot write {}: {error}", path.display());
            1
        }
    }
}

fn launchctl(args: &[&str]) -> bool {
    std::process::Command::new("/bin/launchctl")
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The same, with `launchctl`'s own output swallowed.
///
/// For the questions rather than the changes: `print` writes a page of the job's
/// state to stdout, which would drown the four lines a menu is reading.
fn launchctl_quiet(args: &[&str]) -> bool {
    std::process::Command::new("/bin/launchctl")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn is_root() -> bool {
    // Asked of the effective uid rather than of `SUDO_USER`, which says how the
    // process was started and not what it may do.
    unsafe { geteuid() == 0 }
}

extern "C" {
    fn geteuid() -> u32;
    fn getuid() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_launchd_starts_lives_inside_the_bundle() {
        // What the permissions are granted to is the bundle's code identity, and a
        // binary outside it is a different identity that nobody has approved
        // (`docs/platform/macos/input-permissions.md`). So the two jobs and the
        // command a person types all have to point in here.
        for path in [
            bundled("favjit"),
            bundled("favjit-watchdog"),
            bundled("favjit-menu"),
        ] {
            assert!(
                path.starts_with(bundle()),
                "{} is outside {}",
                path.display(),
                bundle().display()
            );
        }

        let daemon = plist(
            &bundled("favjit-watchdog"),
            &bundled("favjit"),
            Path::new("/Users/someone/Library/Application Support/favjit/disabled"),
            &[],
        );
        let agent = agent_plist(&bundled("favjit-menu"), Path::new("/a/control"));
        for contents in [&daemon, &agent] {
            assert!(
                contents.contains(&bundle().display().to_string()),
                "a job outside the bundle would run an identity with no permissions: {contents}"
            );
        }
    }

    #[test]
    fn the_bundle_declares_favjit_as_its_application() {
        let contents = info_plist();
        let path = std::env::temp_dir().join("favjit-info-plist-test.plist");
        std::fs::write(&path, &contents).expect("write");
        let parsed = std::process::Command::new("/usr/bin/plutil")
            .args(["-lint", &path.display().to_string()])
            .output()
            .expect("plutil");
        let _ = std::fs::remove_file(&path);
        assert!(
            parsed.status.success(),
            "plutil rejected it: {}",
            String::from_utf8_lossy(&parsed.stdout)
        );

        // The executable Launch Services runs when the bundle is opened, which is
        // how the permission request gets a session to be prompted in.
        assert!(contents.contains("<key>CFBundleExecutable</key>"));
        assert!(contents.contains("<string>favjit</string>"));
        assert!(contents.contains(BUNDLE_ID));
        // An accessory application: opening it to ask for permissions should not put
        // a converter in the Dock or steal the front window.
        assert!(contents.contains("<key>LSUIElement</key>"));
    }

    #[test]
    fn the_plist_names_the_watchdog_and_favjit_under_it() {
        let contents = plist(
            Path::new("/usr/local/libexec/favjit/favjit-watchdog"),
            Path::new("/usr/local/libexec/favjit/favjit"),
            Path::new("/Users/someone/Library/Application Support/favjit/disabled"),
            &["--pointer-resolution".to_string(), "80".to_string()],
        );

        // The watchdog is the program and favjit is its argument, which is the
        // arrangement ADR-0008 asks for: the supervisor has to be the parent.
        let watchdog = contents.find("favjit-watchdog").expect("the watchdog");
        let separator = contents.find("<string>--</string>").expect("the separator");
        assert!(watchdog < separator, "the watchdog comes first");

        // The one flag that says this is the run that delivers, and the value that
        // says so: a plist carrying the flag alone would install a daemon that
        // converts and injects nothing.
        assert!(contents.contains("<string>--dry-run</string>"));
        assert!(contents.contains("<string>false</string>"));
        assert!(
            !contents.contains("--seconds"),
            "a daemon that stopped after a while would be a converter that stops"
        );
        assert!(
            !contents.contains("--skip-built-in"),
            "the Mac's own keyboard is the one the layout was written for"
        );
        // The pointer tuning reaches the job, or the daemon converts keys with a
        // pointer that feels like the one the flags were meant to fix.
        assert!(contents.contains("<string>--pointer-resolution</string>"));
        assert!(contents.contains("<string>80</string>"));
    }

    #[test]
    fn an_install_told_nothing_forwards_nothing() {
        // Which is what installs the converter's own defaults, link included. An
        // install that had to be given a flag to reach the useful configuration
        // reaches it only while somebody remembers the flag, and the run that forgot
        // it says nothing about what it is missing.
        let forwarded = forwarded_arguments(&["favjit".to_string(), "--install".to_string()]);
        assert!(forwarded.is_empty(), "{forwarded:?}");
    }

    #[test]
    fn the_flags_that_switch_things_off_reach_the_job() {
        // The only ones worth forwarding: a daemon started without them is a daemon
        // running what a bare `favjit` runs, and these are how a person departs from
        // that for good rather than for one run.
        let forwarded = forwarded_arguments(&[
            "favjit".to_string(),
            "--install".to_string(),
            "--no-listen".to_string(),
            "--no-invert-scroll".to_string(),
            "--port".to_string(),
            "9000".to_string(),
        ]);
        let contents = plist(
            Path::new("/a/watchdog"),
            Path::new("/a/favjit"),
            Path::new("/a/control"),
            &forwarded,
        );
        assert!(contents.contains("<string>--no-listen</string>"));
        assert!(contents.contains("<string>--no-invert-scroll</string>"));
        // No port among them: the machine picks one and the advertisement carries it,
        // so a number in the plist would be a second answer to a question mDNS has
        // already answered.
        assert!(!contents.contains("port"), "{contents}");
        // `--install` itself is not among them, or the daemon would install over
        // itself every time launchd started it.
        assert!(!contents.contains("<string>--install</string>"));
    }

    #[test]
    fn the_plist_is_valid_property_list_syntax() {
        // Written by hand as a format string, so nothing else checks it. A plist
        // launchd cannot parse fails as an unexplained bootstrap error.
        let contents = plist(
            Path::new("/a/watchdog"),
            Path::new("/a/favjit"),
            Path::new("/a/control"),
            &["--no-invert-scroll".to_string()],
        );
        let path = std::env::temp_dir().join("favjit-plist-test.plist");
        std::fs::write(&path, &contents).expect("write");
        let status = std::process::Command::new("/usr/bin/plutil")
            .args(["-lint", &path.display().to_string()])
            .output()
            .expect("plutil");
        let _ = std::fs::remove_file(&path);

        assert!(
            status.status.success(),
            "plutil rejected it: {}",
            String::from_utf8_lossy(&status.stdout)
        );
    }

    #[test]
    fn the_menu_runs_in_the_login_session_and_touches_no_device() {
        let contents = agent_plist(
            Path::new("/usr/local/libexec/favjit/favjit-menu"),
            Path::new("/Users/someone/Library/Application Support/favjit/disabled"),
        );

        assert!(contents.contains("<string>/usr/local/libexec/favjit/favjit-menu</string>"));
        // Aqua, because a menu bar item without a window server session is a
        // process launchd would restart forever with nothing on screen.
        assert!(
            contents.contains("<string>Aqua</string>"),
            "the agent has to be limited to a graphical session: {contents}"
        );
        // The daemon's arguments would ask a process with no privilege to seize the
        // keyboards, which fails — and would take the escape hatch down with it.
        assert!(
            !contents.contains("--dry-run"),
            "the menu captures nothing: {contents}"
        );
    }

    #[test]
    fn the_menu_is_a_different_job_from_the_converter() {
        // One label for both would make loading the agent unload the daemon: the
        // second bootstrap of a label replaces the first, and the converter is the
        // one that would go.
        assert_ne!(LABEL, MENU_LABEL);
        assert_ne!(plist_path(), agent_plist_path());
    }

    #[test]
    fn the_agent_plist_is_valid_property_list_syntax() {
        let contents = agent_plist(Path::new("/a/favjit-menu"), Path::new("/a/control"));
        let path = std::env::temp_dir().join("favjit-agent-plist-test.plist");
        std::fs::write(&path, &contents).expect("write");
        let status = std::process::Command::new("/usr/bin/plutil")
            .args(["-lint", &path.display().to_string()])
            .output()
            .expect("plutil");
        let _ = std::fs::remove_file(&path);

        assert!(
            status.status.success(),
            "plutil rejected it: {}",
            String::from_utf8_lossy(&status.stdout)
        );
    }
}
