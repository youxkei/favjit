//! `favjit` on Windows.
//!
//! ADR-0005 names this binary after the platform rather than the role, so it
//! survives the day a machine runs both — and so that the thing a person runs is
//! called the same on both machines.
//!
//! The whole of it is in [`source`], behind a platform gate, because the crates
//! this binary is built out of are per platform: on a Mac there is no capture and
//! no hook to reach, and a `main` that referred to them would stop
//! `cargo test --workspace` from building there. Saying so and exiting is the
//! honest thing for a build that cannot do the job.

#[cfg(windows)]
mod source;

#[cfg(windows)]
fn main() {
    source::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "favjit's source half reads the Windows keyboards and mice; there are none here. \
         Build this on the Windows machine."
    );
    std::process::exit(1);
}
