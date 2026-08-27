//! Whether favjit is converting, as a file two processes can read (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`).
//!
//! Here rather than in the binary that installs it, because the menu bar item is a
//! second process that has to agree with it exactly: two implementations of "is it
//! off?" would be two answers, and the one on screen would be the one nobody
//! checked.
//!
//! A file rather than a socket or a signal, and in the console user's own directory,
//! so that turning favjit off needs no privilege — the whole point is to be usable
//! when the keyboard is not.
//!
//! **Where the file sits is not answered here.** Each read below answers one thing
//! this process was given, and which of them the file is looked for under is one
//! answer composed from two — `favjit_engine::control`'s, where every program that
//! reads the file gets the same one (ADR-0006).

use std::path::{Path, PathBuf};

/// The user a `sudo` put this process under, if one did.
pub fn sudo_user() -> Option<String> {
    std::env::var("SUDO_USER").ok()
}

/// This process's own home.
pub fn home() -> Option<PathBuf> {
    a_path(std::env::var("HOME"))
}

/// The path a variable holds, and none where it holds nothing.
///
/// Apart from the read rather than spelled onto the end of it, because the read
/// is the one call and what it answered is a string either way: a machine that
/// turned it into a path beside the asking would be one taking a turning there
/// (ADR-0006).
fn a_path(value: Result<String, std::env::VarError>) -> Option<PathBuf> {
    value.ok().map(PathBuf::from)
}

/// Whether favjit is converting.
pub fn is_converting(control: &Path) -> bool {
    !control.exists()
}

/// Where the file is written, for whoever is turning converting off.
///
/// The path in the open rather than behind a constructor, because there is
/// nothing to make: a borrow put in a field is the borrow, and a function doing
/// only that is a name in a host that reaches nothing (ADR-0006).
pub struct Store<'a> {
    pub control: &'a Path,
}

/// The directory a file sits in, and the file's own path where it names no
/// directory — which is a root, and a root is already there.
fn holding(file: &Path) -> &Path {
    file.parent().unwrap_or(file)
}

impl favjit_host::ControlStore for Store<'_> {
    fn make_directory(&mut self) -> std::io::Result<()> {
        std::fs::create_dir_all(holding(self.control))
    }

    fn write_disabled(&mut self) -> std::io::Result<()> {
        std::fs::write(self.control, b"")
    }
}

/// Start again.
pub fn enable(control: &Path) -> std::io::Result<()> {
    already_on(std::fs::remove_file(control))
}

/// A file that was not there read as converting already being on.
///
/// Not a failure: a menu will ask for this twice, and so will a person who could
/// not tell whether the first one landed.
fn already_on(removed: std::io::Result<()>) -> std::io::Result<()> {
    match removed {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converting_is_the_absence_of_the_file() {
        use favjit_host::ControlStore;

        let control = std::env::temp_dir().join(format!("favjit-control-{}", std::process::id()));
        let _ = std::fs::remove_file(&control);

        assert!(is_converting(&control));
        let mut store = Store { control: &control };
        store.make_directory().expect("control directory");
        store.write_disabled().expect("disable");
        assert!(!is_converting(&control));
        enable(&control).expect("enable");
        assert!(is_converting(&control));
        enable(&control).expect("enabling twice is not a failure");
    }
}
