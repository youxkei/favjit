//! The control-file sequence shared by the command and menu entry points.

use std::path::PathBuf;

use favjit_host::ControlStore;

/// Where under a home the control file sits.
///
/// One string in one place because more than one program reads it: two spellings
/// would be two answers, and the one on screen would be the one nobody checked
/// (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`).
const UNDER_A_HOME: &str = "Library/Application Support/favjit/disabled";

/// The home the control file is looked for under, out of the two a process may
/// have been given.
///
/// The `sudo` user's first, because the daemon and the installer run as root and
/// root's home is not where a person's menu can reach; the process's own
/// otherwise, which is what the menu itself has. Neither read decides this —
/// each host answers one question and this is where the two answers are put
/// together, since a host that chose between them would be choosing for every
/// program that reads the file (ADR-0006).
pub fn console_home(sudo_user: Option<String>, home: Option<PathBuf>) -> Option<PathBuf> {
    sudo_user
        .map(|user| PathBuf::from("/Users").join(user))
        .or(home)
}

/// The control file under that home.
pub fn path(home: &std::path::Path) -> PathBuf {
    home.join(UNDER_A_HOME)
}

/// Write the file whose presence stops conversion.
pub fn disable(host: &mut dyn ControlStore) -> std::io::Result<()> {
    host.make_directory()?;
    host.write_disabled()
}
