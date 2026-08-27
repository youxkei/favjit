//! Whether favjit is converting, as a file two processes can read (ADR-0012).
//!
//! Here rather than in the binary that installs it, because the menu bar item is a
//! second process that has to agree with it exactly: two implementations of "is it
//! off?" would be two answers, and the one on screen would be the one nobody
//! checked.
//!
//! A file rather than a socket or a signal, and in the console user's own directory,
//! so that turning favjit off needs no privilege — the whole point is to be usable
//! when the keyboard is not.

use std::path::{Path, PathBuf};

// Where the file is, is [`favjit_core::control`]'s: three processes have to agree on
// it and none of them needs a filesystem to work it out. What is here is the
// filesystem.
//
// Passed through rather than left to each caller to reach for, so that the menu bar
// item — which links this host and not `core` — cannot end up looking at a different
// path from the converter's.
pub use favjit_core::control::path;

/// The console user's home, from a process that may be running as root.
///
/// `SUDO_USER` first, because the daemon and the installer run as root and root's
/// home is not where a person's menu can reach; `HOME` otherwise, which is what the
/// menu itself has.
pub fn console_home() -> Option<PathBuf> {
    if let Ok(user) = std::env::var("SUDO_USER") {
        return Some(PathBuf::from("/Users").join(user));
    }
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Whether favjit is converting.
pub fn is_converting(control: &Path) -> bool {
    !control.exists()
}

/// Stop converting, and give the keyboards back.
pub fn disable(control: &Path) -> std::io::Result<()> {
    if let Some(parent) = control.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(control, b"")
}

/// Start again.
pub fn enable(control: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(control) {
        // Already on is not a failure: a menu will ask for this twice, and so will
        // a person who could not tell whether the first one landed.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converting_is_the_absence_of_the_file() {
        let control = std::env::temp_dir().join(format!("favjit-control-{}", std::process::id()));
        let _ = std::fs::remove_file(&control);

        assert!(is_converting(&control));
        disable(&control).expect("disable");
        assert!(!is_converting(&control));
        enable(&control).expect("enable");
        assert!(is_converting(&control));
        enable(&control).expect("enabling twice is not a failure");
    }
}
