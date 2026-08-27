//! `favjit-watchdog` — the process that gives the keyboard back (ADR-0008).
//!
//! It starts favjit, sends a probe down one pipe, and waits for a heartbeat on the
//! other. When the heartbeats stop within their bound it ends the child, because
//! suppression must never outlive the ability to process input: a failure has to
//! degrade to "favjit stopped working", never "the keyboard stopped working".
//!
//! **The judgement is not here.** When a probe is due, how long a silence may last,
//! what a silence that long means, and the order of asking a process to stop before
//! insisting are `favjit_engine::watchdog`'s, where the end-to-end suite drives them.
//! Every role that suppresses needs supervising, and one copy of that judgement under
//! test is worth more than one per platform that nothing reaches. What is here is the
//! arguments, and a platform half: pipes, a child, and a way to end it.
//!
//! It does nothing else. Being small enough to trust is the whole of its value, so
//! every responsibility that could be added here is one more way for the component
//! that must not fail to fail. It cannot inject input and holds no conversion state —
//! releasing the keys is a constraint on how favjit injects, not a job handed to this.
//!
//! Usage:
//!
//! ```text
//! favjit-watchdog [--timeout SECONDS] [--probe SECONDS] [--trace-out PATH] [--log PATH]
//!                 -- <favjit> [args...]
//! ```
//!
//! **On Windows nothing is printed and `--log` is where it goes instead.** A console
//! program started by a logon task is given no console to inherit, so Windows makes one
//! for it — and with Windows Terminal as the default that is a terminal window on the
//! desktop for the whole session. This is a window-subsystem program so that no console
//! is ever made, and it starts the process it supervises without one for the same reason.
//! What that costs is a run by hand printing nothing unless it is given `--log`.

// No console, ever. See the note above: what a console-subsystem supervisor started by a
// logon task gets is a terminal window on the desktop for the whole session. Ignored on
// every other platform, where a subsystem is not a thing a binary has.
#![windows_subsystem = "windows"]

use core::time::Duration;

use favjit_engine::supervision::BEAT_WITHIN;
#[cfg(any(unix, windows))]
use favjit_engine::watchdog::{run, Bound, Exit, Supervised};
#[cfg(any(unix, windows))]
use log::{error, info, warn};

#[cfg(any(unix, windows))]
mod beats;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Twice what a run promises to beat within, probed four times a second, and a fifth of
/// a second to stop in.
///
/// **Derived from the promise rather than picked beside it.** How often a run says its
/// loop came round is `favjit_engine::supervision::BEAT_WITHIN`, which is the same
/// number seen from the other side — and a supervisor allowing less than it ends a run
/// that is working, which is what a bound picked on its own for a person's patience did:
/// two seconds allowed against a two-second lookup, and every attempt to reach the other
/// machine was ended partway through.
///
/// Twice, and not exactly it, because the margin is the judgement left: an ordinary
/// stall — a slow injection, a burst of keys — should not read as a wedge, and a person
/// should still get their keyboard back before reaching for the power button.
const SILENCE: Duration = BEAT_WITHIN.saturating_mul(2);
const PROBE_EVERY: Duration = Duration::from_millis(250);
const GRACE: Duration = Duration::from_millis(200);

#[cfg(any(unix, windows))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(split) = args.iter().position(|a| a == "--") else {
        eprintln!(
            "usage: favjit-watchdog [--timeout SECONDS] [--probe SECONDS] [--trace-out PATH] \
             [--log PATH] -- <favjit> [args...]"
        );
        std::process::exit(2);
    };
    // Before anything is logged, because the point of it is where the logging goes.
    let log = arg_after(&args[..split], "--log");
    logging(log.as_deref());

    let child: Vec<String> = args[split + 1..].to_vec();
    if child.is_empty() {
        error!("nothing to supervise");
        std::process::exit(2);
    }

    let flags = &args[..split];
    let bound = Bound {
        silence: seconds(flags, "--timeout").unwrap_or(SILENCE),
        probe_every: seconds(flags, "--probe").unwrap_or(PROBE_EVERY),
        grace: GRACE,
    };
    // Where a trace may be written, and nowhere by default: it holds whatever was
    // typed in the window it covers, so writing one out is an explicit action and this
    // flag is it (ADR-0009).
    let trace_out = arg_after(flags, "--trace-out");

    let mut machine = machine(child, trace_out, log);
    info!(
        "supervising: {:?} of silence, probing every {:?}",
        bound.silence, bound.probe_every
    );
    std::process::exit(said_about(run(&bound, &mut machine)));
}

/// What the supervision's ending means to whatever started this.
///
/// Here and not in `engine`: an exit code is this program's contract with a service
/// manager, and deciding it is not something the judgement should carry (ADR-0006).
#[cfg(any(unix, windows))]
fn said_about(supervised: Supervised) -> i32 {
    match supervised {
        // Passed through, because a child that exited on its own is not a failure: a
        // bounded run is how favjit gets measured.
        Supervised::Ended(Exit::Code(code)) => {
            info!("the supervised process exited with {code}");
            code
        }
        // A child something else ended named no status of its own, and it did not
        // finish what it was doing.
        Supervised::Ended(Exit::Signal(signal)) => {
            warn!("the supervised process was ended by signal {signal}");
            1
        }
        // `Exit` is `#[non_exhaustive]`: a way this crate does not yet name a
        // process as having ended is still one that did not finish what it was
        // doing, the same as `Signal`.
        Supervised::Ended(_) => {
            warn!("the supervised process was ended by something other than itself");
            1
        }
        Supervised::Killed => 1,
        Supervised::NotStarted => {
            warn!("nothing was supervised");
            1
        }
    }
}

/// Where this and the process it supervises write what they have to say.
///
/// Warn and above by default, so an unattended supervisor is silent until it has
/// something to say. `RUST_LOG=debug` shows the silence it is measuring.
///
/// A file rather than the inherited stderr wherever one is asked for, because there are
/// two ways a supervisor gets started and only one of them has a terminal behind it: run
/// by hand, stderr is where a person is looking, and started at logon or at boot it is
/// wherever the thing that started it decided — which for a Windows logon task is nowhere
/// at all. Appended to, so what is kept is the last few runs rather than the last one.
#[cfg(any(unix, windows))]
fn logging(log: Option<&str>) {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"));
    if let Some(path) = log {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(file) => {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
            // Said on stderr, which is the only place left to say it.
            Err(error) => eprintln!("cannot write {path}: {error}; logging to stderr instead"),
        }
    }
    builder.init();
}

/// The machine this runs on, as the judgement's boundary.
#[cfg(unix)]
fn machine(child: Vec<String>, trace_out: Option<String>, log: Option<String>) -> unix::Unix {
    unix::Unix::new(
        child,
        trace_out,
        log,
        String::from(favjit_engine::supervision::TRACE_ASKED_FOR),
    )
}

#[cfg(windows)]
fn machine(child: Vec<String>, trace_out: Option<String>, log: Option<String>) -> windows::Windows {
    windows::Windows::new(child, trace_out, log)
}

#[cfg(any(unix, windows))]
fn arg_after(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

#[cfg(any(unix, windows))]
fn seconds(args: &[String], name: &str) -> Option<Duration> {
    arg_after(args, name)
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs_f64)
}

/// Nothing here can supervise a process.
///
/// Saying so and exiting is the honest thing for a build that cannot do the job — the
/// same answer `favjit` gives on the platform whose keyboards it cannot read.
#[cfg(not(any(unix, windows)))]
fn main() {
    eprintln!(
        "favjit-watchdog supervises through pipes and a process it can end; neither is written \
         for this platform."
    );
    std::process::exit(1);
}
