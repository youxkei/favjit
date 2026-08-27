//! Where the file that says favjit is off lives (ADR-0012).
//!
//! Here rather than beside the calls that read and write it, because three
//! processes have to agree on it exactly: the converter, the menu bar item that
//! turns it off, and the installer that puts the menu there. Two spellings of this
//! path are two answers to "is it off?", and the one on screen would be the one
//! nobody checked.
//!
//! Only the path. Whether the file is *there* is a question for a filesystem, and
//! that is a host's — this is the half a test can answer.

use std::path::{Path, PathBuf};

/// The file whose presence means "converting is off".
///
/// Under the person's own library and not `/Library`, so that turning favjit off
/// needs no privilege: the whole point is to be usable when the keyboard is not.
pub fn path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/favjit/disabled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_file_lives_under_the_persons_own_library() {
        assert_eq!(
            path(Path::new("/Users/someone")),
            PathBuf::from("/Users/someone/Library/Application Support/favjit/disabled")
        );
    }
}
