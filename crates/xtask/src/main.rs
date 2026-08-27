//! The workspace's own tasks: `cargo xtask <task>`.
//!
//! A crate rather than a script, because a script is a thing to find and a task is
//! a thing to run: this one is in the workspace, so it builds with everything else
//! and a task that stopped working stops the build rather than the person using it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("coverage") => coverage(args.collect()),
        Some(other) => {
            eprintln!("no task called {other}");
            usage()
        }
        None => usage(),
    }
}

fn usage() -> std::process::ExitCode {
    eprintln!(
        "usage: cargo xtask coverage [file.rs ...]\n\n\
         coverage        what the end-to-end suite alone reaches in `engine`, per file\n\
         coverage <file> the lines in that file of `engine` nothing reached"
    );
    std::process::ExitCode::from(2)
}

/// What `crates/e2e` alone reaches in `crates/engine`.
///
/// The suite and not the workspace: what a unit test inside `engine` reaches says
/// nothing about whether the behaviour is drivable from outside it, which is the
/// question ADR-0006 and ADR-0007 leave this measuring (`docs/adr/`).
fn coverage(files: Vec<String>) -> std::process::ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two directories under the workspace root")
        .to_path_buf();
    let out = root.join("target/coverage");
    let profdata = out.join("e2e.profdata");

    let Some(tools) = llvm_tools() else {
        eprintln!(
            "no llvm-profdata in the toolchain; run `rustup component add llvm-tools-preview`"
        );
        return std::process::ExitCode::FAILURE;
    };

    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("cannot make {}: {error}", out.display());
        return std::process::ExitCode::FAILURE;
    }
    // Cleared rather than merged into: a `.profraw` left from an earlier build
    // belongs to binaries that no longer exist, and llvm-cov reads it as coverage
    // of code the run never had.
    for stale in profraws(&out) {
        let _ = std::fs::remove_file(stale);
    }

    // Its own target directory, because `-C instrument-coverage` is a different
    // build of every crate: sharing one with the ordinary build makes each of them
    // rebuild the other's dependencies from scratch.
    let target = out.join("target");
    let ran = Command::new("cargo")
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &target)
        .env("RUSTFLAGS", "-C instrument-coverage")
        .env("LLVM_PROFILE_FILE", out.join("%p-%m.profraw"))
        .args(["test", "-p", "favjit-e2e"])
        .status();
    match ran {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "the suite did not pass ({status}); the numbers below would be of a \
                       run that failed"
            );
            return std::process::ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("cannot run the suite: {error}");
            return std::process::ExitCode::FAILURE;
        }
    }

    // Every test binary, because one file's coverage is spread across all of them:
    // a report given one binary counts the other twenty-nine as unreached.
    let binaries = match test_binaries(&root, &target, &out) {
        Some(binaries) if !binaries.is_empty() => binaries,
        _ => {
            eprintln!("cannot tell which binaries the suite built");
            return std::process::ExitCode::FAILURE;
        }
    };

    let merged = Command::new(tools.join("llvm-profdata"))
        .arg("merge")
        .arg("-sparse")
        .args(profraws(&out))
        .arg("-o")
        .arg(&profdata)
        .status();
    if !matches!(merged, Ok(status) if status.success()) {
        eprintln!("cannot merge the profiles");
        return std::process::ExitCode::FAILURE;
    }

    // The first binary is the positional argument and the rest are `-object`,
    // which is what llvm-cov's own command line takes.
    let (first, rest) = binaries.split_first().expect("checked above");
    let objects = || {
        let mut args = vec![first.clone()];
        for binary in rest {
            args.push("-object".into());
            args.push(binary.clone());
        }
        args
    };

    let mut cov = Command::new(tools.join("llvm-cov"));
    match files.is_empty() {
        true => {
            cov.arg("report")
                .arg(format!("--instr-profile={}", profdata.display()))
                .args(objects())
                // Everything but `engine`: the other crates are the suite itself,
                // the simulator, and the crates a host reaches — none of which
                // this is asking about.
                .arg(
                    "--ignore-filename-regex=(^|/)(\\.cargo|rustc|library)/|\
                     crates/(e2e|host-sim|host-macos|host-windows|bin-|discovery|noise|\
                     pairing-exchange|link-wire|hid|host|xtask)/",
                );
        }
        false => {
            cov.arg("show")
                .arg(format!("--instr-profile={}", profdata.display()))
                .args(objects())
                .arg("--show-line-counts");
            for file in &files {
                cov.arg(root.join("crates/engine/src").join(file));
            }
        }
    }
    match cov.current_dir(&root).status() {
        Ok(status) if status.success() => std::process::ExitCode::SUCCESS,
        _ => {
            eprintln!("llvm-cov would not run");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Where the toolchain keeps `llvm-profdata` and `llvm-cov`.
///
/// Asked of `rustc` rather than assumed under `~/.rustup`: the path carries the
/// toolchain and the target triple, and a machine whose default toolchain is not
/// the one the workspace builds with would be measured with the other one's tools.
fn llvm_tools() -> Option<PathBuf> {
    let sysroot = Command::new("rustc").arg("--print=sysroot").output().ok()?;
    let sysroot = PathBuf::from(String::from_utf8(sysroot.stdout).ok()?.trim());
    let target = Command::new("rustc").arg("-vV").output().ok()?;
    let target = String::from_utf8(target.stdout).ok()?;
    let triple = target
        .lines()
        .find_map(|line| line.strip_prefix("host: "))?
        .trim();
    let bin = sysroot.join("lib/rustlib").join(triple).join("bin");
    bin.join("llvm-profdata").exists().then_some(bin)
}

fn profraws(out: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(out) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|kind| kind == "profraw"))
        .collect()
}

/// The test binaries the suite built, from cargo's own account of the build.
///
/// Asked for with `--no-run` after the tests have already run, so the build is a
/// cache hit and what comes back is the same set of binaries the profiles came
/// from.
fn test_binaries(root: &Path, target: &Path, out: &Path) -> Option<Vec<String>> {
    let built = Command::new("cargo")
        .current_dir(root)
        .env("CARGO_TARGET_DIR", target)
        .env("RUSTFLAGS", "-C instrument-coverage")
        .env("LLVM_PROFILE_FILE", out.join("%p-%m.profraw"))
        .args([
            "test",
            "-p",
            "favjit-e2e",
            "--no-run",
            "--message-format=json",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let json = String::from_utf8(built.stdout).ok()?;
    Some(
        json.lines()
            .filter_map(|line| line.split("\"executable\":\"").nth(1))
            .filter_map(|rest| rest.split('"').next())
            .filter(|path| *path != "null")
            .map(String::from)
            .collect(),
    )
}
