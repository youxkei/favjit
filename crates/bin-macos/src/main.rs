//! `favjit` on macOS.
//!
//! ADR-0005 names this binary after the platform rather than the role, so it
//! survives the day a machine runs both. It is a shell: it reads the arguments,
//! builds the machine out of `host-macos`, hands both to `core::sink::run`, and
//! turns what comes back into log lines and an exit code. The run itself, and the
//! order it brings the machine up in, are `core`'s (ADR-0006).
//!
//! One flag decides the mode. A bare command is a dry run, which converts for real
//! and delivers nothing, so it changes nothing outside this process; `--dry-run
//! false` is the run that takes the keyboards exclusively and injects. There is no
//! flag for suppressing, because delivering without it types every keystroke twice
//! — once unconverted from the keyboard and once converted from here — which is not
//! a choice anybody would make.

mod install;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// Two clocks in one file: `Instant` bounds the run in real time, `HostInstant` is
// the host's monotonic stamp that `core` reads deadlines off.
use favjit_core::link::LinkHost;
use favjit_core::pairing::{Identity, IdentityStore, Paired};
use favjit_core::sink::{self, Ending, Request, Settings, SinkHost, SinkInputHost};
use favjit_core::{
    pointer::{Tuning, Wanted},
    trace::Trace,
    DeviceMatch, Ended, Host, HostEvent, Injected, Instant as HostInstant, Layout,
};
use favjit_host_macos::{
    acceleration, ax_trusted, control, hid_access, link, request_hid_access, system_repeat, Config,
    DryRun, HidAccess, MacOsHost, VIRTUAL_KEYBOARD_PRODUCT, VIRTUAL_KEYBOARD_VENDOR,
};
use log::{error, info, warn};

/// Keyboards to leave alone.
///
/// **The first is the device favjit's own output goes to** (ADR-0011), named by
/// the vendor and product favjit itself initialises it with. Reading it back
/// would be worse than a loop: a run that delivers seizes what it captures, so
/// favjit would take its own output device exclusively and every converted
/// keystroke would come back to itself instead of reaching an application.
///
/// The second is the same device as Karabiner-Elements initialises it — whoever
/// initialises the virtual keyboard sets its identity, so a device left over from
/// its client carries different numbers.
///
/// Named individually because nothing observed distinguishes a virtual keyboard
/// from a real one in general: this one has no `Transport` property while both
/// real keyboards do, which is a difference but not an established rule.
const IGNORE: &[DeviceMatch] = &[
    DeviceMatch::new(VIRTUAL_KEYBOARD_VENDOR, VIRTUAL_KEYBOARD_PRODUCT),
    DeviceMatch::new(1452, 591),
];

fn main() {
    // Two kinds of output, kept apart on purpose. What a mode exists to produce
    // — the usage report, the converted keys — goes to stdout, where a person or
    // a pipe can read it. Everything about how the run is going goes through the
    // log, so raising the level cannot silence the answer.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();

    // Before any of it, because the modes below are found by scanning for their own
    // flag: a run given something this binary does not read would otherwise do
    // whatever the rest of the arguments say and never mention the one it dropped.
    let unknown = unknown_arguments(&args);
    if !unknown.is_empty() {
        error!("nothing here reads {}", unknown.join(" "));
        std::process::exit(1);
    }

    // The modes that only touch the machine's configuration, before anything opens
    // a device.
    if args.iter().any(|a| a == "--install") {
        std::process::exit(install::install());
    }
    if args.iter().any(|a| a == "--uninstall") {
        std::process::exit(install::uninstall());
    }
    if let Some(path) = arg_after(&args, "--trace-report") {
        std::process::exit(trace_report(Path::new(path)));
    }
    if args.iter().any(|a| a == "--permission-check") {
        std::process::exit(permission_check(arg_after(&args, "--permission-check")));
    }
    if args.iter().any(|a| a == "--identity") {
        std::process::exit(identity());
    }
    if args.iter().any(|a| a == "--pointers") {
        // Tuning from here as well as from a run, because these properties belong to
        // the device and outlive the process that set them: trying a number this way
        // costs nothing, where trying it through the daemon costs an install.
        tune_output_pointer(&args);
        std::process::exit(list_pointers());
    }
    let control = arg_after(&args, "--control")
        .map(PathBuf::from)
        .or_else(|| control::console_home().map(|home| control::path(&home)));
    if args.iter().any(|a| a == "--pair") {
        std::process::exit(pair());
    }
    if args
        .iter()
        .any(|a| a == "--disable" || a == "--enable" || a == "--status")
    {
        let Some(control) = control.as_deref() else {
            error!("cannot tell whose control file to look at; pass --control PATH");
            std::process::exit(1);
        };
        std::process::exit(
            match args
                .iter()
                .find(|a| *a == "--disable" || *a == "--enable" || *a == "--status")
            {
                Some(flag) if flag == "--disable" => install::disable(control),
                Some(flag) if flag == "--enable" => install::enable(control),
                _ => install::status(control),
            },
        );
    }

    let Some(dry_run) = dry_run(&args) else {
        error!("--dry-run takes true or false, or nothing at all for true");
        std::process::exit(1);
    };
    let mut config = Config {
        ignore: IGNORE.to_vec(),
        // A run that delivers takes them exclusively, always: the physical
        // keystroke arrives alongside the converted one otherwise, and that is not
        // a mode (`favjit_core::sink::Request`).
        suppress: !dry_run,
        skip_built_in: args.iter().any(|a| a == "--skip-built-in"),
        watch_everything: false,
    };

    // `--usages` runs no conversion at all and prints no key, only the page and
    // usage the tables have no name for. That is the whole point: finding out
    // where a key reports must not require logging what was typed.
    //
    // It watches every element rather than the ones the tables already name,
    // because a key filtered out for being unnamed is a key this mode cannot
    // find — which is exactly what it exists for.
    if let Some(i) = args.iter().position(|a| a == "--usages") {
        let seconds: f64 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(30.0);
        config.watch_everything = true;
        std::process::exit(scan_usages(seconds, config, control));
    }

    let unmapped = MacOsHost::unmapped_keys();
    if !unmapped.is_empty() {
        warn!("keys the layout names but this host cannot recognise yet: {unmapped:?}");
    }
    if !dry_run {
        warn!(
            "delivering takes the captured keyboards exclusively; without \
             --skip-built-in the Mac's own keyboard is among them, so a wedge \
             leaves nothing to type on"
        );
    }

    // Nothing here produces repeats, because the OS already does: output is a
    // device that holds key state, and a key it says is down repeats at the
    // machine's own rate whatever favjit sends alongside it — measured three ways
    // in `docs/platform/macos/key-repeat.md`. A second source would be at best
    // invisible and at worst a doubled rate.
    //
    // The rates are still read and logged, because they are what the repeats a
    // person sees should match: a machine whose sliders moved and whose repeat did
    // not is the kind of thing this line answers.
    let repeat = None;
    match system_repeat() {
        Some(rates) => info!(
            "key repeat comes from the OS: {:?} then every {:?}",
            rates.initial, rates.interval
        ),
        None => warn!("could not read the machine's key repeat rates"),
    }

    let settings = Settings {
        repeat,
        pointer: pointer_tuning(&args),
    };

    let deadline = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| Instant::now() + Duration::from_secs_f64(s));
    // The deadline goes to one of the two and never both: `until` ends the run at
    // it and the wedge hangs at it, so a deadline given to both would be a race
    // whose loser never happened.
    let (until, wedge) = if asked_to_wedge(&args) {
        if deadline.is_none() {
            warn!(
                "--wedge does nothing without --seconds: it is what should happen at that deadline"
            );
        }
        (None, deadline)
    } else {
        (deadline, None)
    };

    // One flag decides the mode, so there is no combination to get wrong. A
    // delivering run listens unless it is told not to: what refuses a source is the
    // pairing and not the absence of a socket (ADR-0004), so a shut port protects
    // nothing the empty list does not already protect — while a run that quietly has
    // no link is a Windows keyboard that does nothing and says nothing about why.
    // What `--no-listen` is for is the network you are only visiting, since serving
    // the link is also what announces this machine on it.
    //
    // A dry run opens no socket whichever way that flag went, because a run that
    // exists to change nothing outside this process should not announce itself on a
    // network either.
    let request = match dry_run {
        true => Request::DryRun,
        false => Request::Injecting {
            listen: !args.iter().any(|a| a == "--no-listen"),
        },
    };
    let mut region = favjit_host_macos::inherited_trace_region();
    if region.is_some() {
        info!("recording a trace into the supervisor's memory");
    }

    // Before the run, and whichever way it runs: a run waiting for converting to be
    // switched on has to answer a `SIGTERM` too, and that wait happens before
    // anything else.
    let stop = stop_flag();
    stop_on_term(&stop);

    if !dry_run {
        let mut inner = MacOsHost::new(config, pointer_wanted(&args), control.clone());
        inner.until(until);
        inner.stop_on(stop);

        let mut host = Injecting { inner, wedge };
        let ending = sink::run(
            &request,
            Layout::dudrack(),
            settings,
            &mut host,
            region.as_mut().map(|region| region.bytes()),
        );
        report(&host.inner);
        std::process::exit(stopped(ending));
    } else {
        info!("dry run: converting for real, injecting nothing");
        let mut inner = DryRun::new(config, control.clone());
        inner.until(until);
        inner.stop_on(stop);
        let mut host = Reporting {
            inner,
            seen: BTreeMap::new(),
            wedge,
        };
        let ending = sink::run(
            &request,
            Layout::dudrack(),
            settings,
            &mut host,
            region.as_mut().map(|region| region.bytes()),
        );
        report(host.inner.host());
        std::process::exit(stopped(ending));
    }
}

/// What the process exits with.
///
/// A run that could not start says so through the exit code, because launchd is what
/// reads it: a converter that returned success having converted nothing would be
/// restarted forever with nothing in the log to say why.
fn stopped(ending: Ending) -> i32 {
    match ending {
        Ending::Converted => 0,
        // Zero, because being switched off is not a fault and whatever supervises
        // this is meant to start it again: the next run is the one that takes the
        // keyboards, from nothing.
        Ending::SwitchedOff => {
            info!("converting was switched off; this run is over and the next one starts afresh");
            0
        }
        // Also zero, and also for the restart: the device belongs to a daemon that
        // can come back, and a run that ended for this reason gave the keyboards up
        // rather than converting into a closed socket.
        Ending::OutputGone => {
            error!("the virtual HID device went away; the keyboards are back and this run is over");
            0
        }
        // Zero for the restart as well: nothing rebinds the socket inside a run, so
        // the next run is what the other machine can reach again — and the keyboards
        // in front of the person were given back on the way out.
        Ending::LinkGone => {
            error!("the link stopped being served; the keyboards are back and this run is over");
            0
        }
        Ending::NoPermission => {
            error!("cannot read the keyboards without input monitoring; ending the run so the next one asks again");
            error!("say yes to the dialog, or turn favjit on under System Settings, Privacy & Security, Accessibility");
            1
        }
        Ending::NoOutput | Ending::NoInput => 1,
    }
}

/// The `IOReturn` values a seize actually comes back with here, read off
/// `<IOKit/IOReturn.h>` by compiling against it.
const NOT_PRIVILEGED: i32 = 0xE00002C1u32 as i32;
const EXCLUSIVE_ACCESS: i32 = 0xE00002C5u32 as i32;

fn report(host: &MacOsHost) {
    for seizure in &host.seizures {
        let outcome = match seizure.code {
            0 if seizure.exclusive => "held exclusively".to_string(),
            0 => "open, shared with everything else".to_string(),
            NOT_PRIVILEGED => "refused: not privileged".to_string(),
            EXCLUSIVE_ACCESS => "refused: something else holds it".to_string(),
            code => format!("refused: {code:#010x}"),
        };
        info!("device {}: {outcome}", seizure.device.0);
    }
    if !host.unknown.is_empty() {
        println!("\nHID usages seen that no rule can name:");
        for u in &host.unknown {
            println!(
                "  device {} page {:#06x} usage {:#04x} ({})",
                u.device.0, u.page, u.usage, u.usage
            );
        }
    }
    if !host.unsendable.is_empty() {
        println!("\nkeys the layout wanted to emit that no HID usage can send:");
        println!("  {:?}", host.unsendable);
    }
    latency(host);
}

/// What favjit's own path cost, per segment.
///
/// Printed at the end rather than as it happens: writing a line per keystroke
/// would put a stderr write in the interactive path, and a latency report that
/// adds latency is measuring itself.
fn latency(host: &MacOsHost) {
    let segments = [
        ("hid stamp -> capture", &host.latency.arrival),
        ("capture -> report ready", &host.latency.pipeline),
        ("writing the report", &host.latency.post),
    ];
    if segments.iter().all(|(_, s)| s.is_empty()) {
        return;
    }
    println!("\nfavjit's own latency, us:");
    for (name, samples) in segments {
        if samples.is_empty() {
            continue;
        }
        // The first sample is reported beside the quantiles because a path walked
        // once is not the same path warm: a single outlier in a run says nothing
        // about typing if it is the one that faulted the code in.
        let first = samples[0] as f64 / 1000.0;
        let mut sorted = samples.clone();
        sorted.sort_unstable();
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize] as f64 / 1000.0;
        println!(
            "  {name:<22} n={:<5} first {:8.1}  p50 {:8.1}  p90 {:8.1}  p99 {:8.1}  max {:8.1}",
            sorted.len(),
            first,
            at(0.5),
            at(0.9),
            at(0.99),
            at(1.0)
        );
    }
}

/// Whether the run should hang at its deadline instead of ending at it.
///
/// A safety device that has never met the failure it exists for is a guess. This is
/// how the watchdog gets tested: hung at the deadline, the loop stops heartbeating
/// while still holding the keyboards, which is exactly the state ADR-0008 says must
/// not outlive the ability to process input.
///
/// It shares `--seconds` rather than carrying a time of its own, because the two
/// would say the same thing — after this long, act — and differ only in the act.
/// Two deadlines would also have to be ordered against each other, and the losing
/// one would silently never happen.
///
/// It hangs inside the loop rather than killing the thread, because a dead thread
/// is a case the run loop's absence would give away; a live loop that has stopped
/// delivering is the one that needs catching.
fn asked_to_wedge(args: &[String]) -> bool {
    args.iter().any(|a| a == "--wedge")
}

/// This machine's own key, and who it will take input from.
///
/// On stdout, because the point of it is to be read: which machine this is, and
/// which sources it has pinned. Pairing itself carries the key over a code
/// (ADR-0004), so nothing here is meant to be transcribed.
fn identity() -> i32 {
    let identity = match favjit_core::pairing::identity(&mut link::IdentityFile::default()) {
        Ok(identity) => identity,
        Err(error) => {
            error!(
                "cannot read or make {}: {error}",
                link::identity_path().display()
            );
            return 1;
        }
    };
    let authorized = link::authorized();

    println!("this machine: {}", identity.fingerprint());
    println!("paired sources: {}", authorized.len());
    if authorized.is_empty() {
        println!("nothing is paired, so the link accepts nobody");
    }
    0
}

/// Authorise a source to send input to this machine (ADR-0004).
///
/// A command of its own, run by a person, because that is the explicit action on
/// the sink the whole authorisation model rests on: nothing the running converter
/// does can add a key, and a source cannot ask to be added.
///
/// It runs alongside a converting favjit rather than asking for one to be switched
/// off. What it offers is a port of its own under a name of its own, so a source
/// looking for a code cannot reach the link by mistake (ADR-0017) — and the key it
/// writes down is in force for the next session, since the list is read at every
/// handshake rather than held from startup.
fn pair() -> i32 {
    if !install::is_root() {
        error!(
            "--pair needs root: it adds a key to {}",
            link::authorized_path().display()
        );
        return 1;
    }

    let identity = match favjit_core::pairing::identity(&mut link::IdentityFile::default()) {
        Ok(identity) => identity,
        Err(error) => {
            error!("no identity, so nothing to pair: {error}");
            return 1;
        }
    };
    let mut host = match favjit_host_macos::pairing::Pairing::open() {
        Ok(host) => host,
        Err(error) => {
            error!("cannot listen for the other machine: {error}");
            return 1;
        }
    };

    match favjit_core::pairing::pair(&identity, &mut host) {
        Paired::Pinned(source) => {
            info!("paired with {source}; the link will accept input from that machine");
            0
        }
        Paired::WrongCode => {
            error!("the code that machine used is not the one shown here; run this again for a fresh one");
            1
        }
        Paired::NoCode => {
            error!("cannot produce a code on this machine");
            1
        }
        Paired::NoSource => {
            error!("nothing connected; run this again when the other machine is ready");
            1
        }
        Paired::Interrupted => {
            error!("the exchange stopped part way; run this again for a fresh code");
            1
        }
        Paired::CannotKeep => {
            error!("cannot write {}", link::authorized_path().display());
            1
        }
        // The other end's ending, which this one cannot reach: this machine is the
        // one being paired to, so it has no sink to fail to find.
        Paired::NoSink => 1,
    }
}

/// What macOS currently believes about every pointing device.
///
/// On stdout, because it is what this mode exists to produce. It names no key and
/// reads no input: the numbers are the machine's settings, not anything typed.
fn list_pointers() -> i32 {
    let pointers = acceleration::pointers();
    if pointers.is_empty() {
        println!("no pointing devices, or the event system would not say");
        return 1;
    }
    for pointer in &pointers {
        println!(
            "vendor={:?} product={:?} {:?}\n  resolution={:?} dpi  acceleration={:?} (in {})",
            pointer.vendor,
            pointer.product,
            pointer.name.as_deref().unwrap_or("(unnamed)"),
            pointer.resolution(),
            pointer.acceleration(),
            pointer.acceleration_key(),
        );
    }
    0
}

/// How far the output device's counts should carry the cursor, and how the OS should
/// curve that, when this run was told nothing.
///
/// Numbers rather than nothing: they belong to the pointer this relays — one
/// TrackPoint, whose reports are mostly single counts — and left to the machine's own
/// 400 dpi that pointer is unusably slow (ADR-0016). A value that had to be passed
/// again at every install is one that goes missing the once it is forgotten, and the
/// symptom is a cursor that feels wrong rather than anything that says so.
const POINTER_RESOLUTION: f64 = 80.0;
const POINTER_ACCELERATION: f64 = 0.8;

/// How far the virtual device's counts should carry the cursor, as this run was
/// asked for it.
///
/// Read here and applied by the host: the numbers come from the arguments, and what
/// they mean to a device belongs to the platform that has the device.
fn pointer_wanted(args: &[String]) -> Wanted {
    Wanted::new(
        arg_after(args, "--pointer-resolution")
            .and_then(|s| s.parse().ok())
            .or(Some(POINTER_RESOLUTION)),
        arg_after(args, "--pointer-acceleration")
            .and_then(|s| s.parse().ok())
            .or(Some(POINTER_ACCELERATION)),
    )
}

/// Tell macOS how far the virtual device's counts should carry the cursor.
///
/// Applied to favjit's own output device, not to the keyboard it relays from: what
/// the OS accelerates is the device the reports arrive on, and that is the virtual
/// one (ADR-0011). Setting it on the TrackPoint would tune a device whose reports
/// nothing but favjit ever sees.
fn tune_output_pointer(args: &[String]) {
    let wanted = pointer_wanted(args);
    let (dpi, factor) = (wanted.resolution(), wanted.acceleration());
    if wanted.is_empty() {
        return;
    }

    let mut tuned = false;
    for pointer in acceleration::pointers() {
        if pointer.vendor != Some(VIRTUAL_KEYBOARD_VENDOR as i64) {
            continue;
        }
        let before = (pointer.resolution(), pointer.acceleration());
        let done = pointer.tune(dpi, factor);
        info!(
            "output pointer {:?}: resolution {:?} -> {:?}, acceleration {:?} -> {:?}{}",
            pointer.name.as_deref().unwrap_or("(unnamed)"),
            before.0,
            pointer.resolution(),
            before.1,
            pointer.acceleration(),
            if done { "" } else { " (refused)" }
        );
        tuned = true;
    }
    if !tuned {
        warn!(
            "no pointing device from vendor {VIRTUAL_KEYBOARD_VENDOR} to tune; is the virtual \
             device up yet?"
        );
    }
}

/// How the pointer should feel, from the flags.
///
/// Flags rather than something read from a file: the values are a property of the
/// hardware on this machine, they change once and then never, and the job that runs
/// favjit records them where anyone can read what is in force.
fn pointer_tuning(args: &[String]) -> Tuning {
    // One flag for both axes, because a wheel that scrolls the wrong way scrolls
    // the wrong way in both — the axes are separate in the tuning itself, for the
    // device that turns out to disagree.
    //
    // Turned over unless told otherwise: which way is right is a property of the
    // wheel this relays, and macOS has one scroll direction switch for every device
    // (ADR-0016), so the machine's own setting cannot answer for a relayed one. A run
    // that had to be told each time gets it wrong the once it is forgotten.
    let invert = !args.iter().any(|a| a == "--no-invert-scroll");
    if invert {
        info!("pointer: scroll turned over");
    }
    Tuning {
        invert_vertical_wheel: invert,
        invert_horizontal_wheel: invert,
    }
}

/// The flags that are followed by their value, and the ones that stand alone.
///
/// Listed rather than derived, because the modes are found by scanning for their
/// own flag: nothing else in this file knows the whole set, so nothing else could
/// tell a misspelling from a flag it simply does not handle.
/// `--usages` and `--permission-check` are here even though their value is
/// optional: a value is taken only when the next argument is not itself a flag, so
/// one list covers both.
const WITH_A_VALUE: [&str; 8] = [
    "--trace-report",
    "--permission-check",
    "--control",
    "--pointer-resolution",
    "--pointer-acceleration",
    "--seconds",
    "--usages",
    // Its value is optional, which this list already allows: what follows a flag is
    // only taken as a value when it is not another flag.
    "--dry-run",
];

const ON_THEIR_OWN: [&str; 12] = [
    "--install",
    "--uninstall",
    "--identity",
    // The digits go on the other machine, so a run given them here has been told to
    // do something this cannot: refusing says which end reads them, where taking the
    // value and ignoring it would leave a person waiting for a code that is already
    // on screen.
    "--pair",
    "--pointers",
    "--disable",
    "--enable",
    "--status",
    "--skip-built-in",
    // The negative forms only: what they turn off is what a run does when told
    // nothing, so the positive form would name the default and do nothing — and an
    // argument that does nothing is one this refuses rather than accepts.
    "--no-invert-scroll",
    "--no-listen",
    "--wedge",
];

/// What this run was given that this binary cannot act on.
///
/// A value is only a value when it follows a flag that takes one; anything else on
/// its own is reported too, since an argument nothing reads is an instruction that
/// silently did not happen.
fn unknown_arguments(args: &[String]) -> Vec<String> {
    let mut unknown = Vec::new();
    let mut rest = args.iter().skip(1).peekable();
    while let Some(argument) = rest.next() {
        if WITH_A_VALUE.contains(&argument.as_str()) {
            if rest.peek().is_some_and(|next| !next.starts_with("--")) {
                rest.next();
            }
            continue;
        }
        if ON_THEIR_OWN.contains(&argument.as_str()) {
            continue;
        }
        unknown.push(argument.clone());
    }
    unknown
}

/// Whether this run delivers nothing.
///
/// True when nothing was said, so that a bare command changes nothing outside this
/// process: `--dry-run false` is how a person asks for the run that takes the
/// keyboards and injects, and asking for it is the point.
///
/// `None` for a value that is neither, rather than a guess: a misspelling that read
/// as `true` would look like favjit converting and doing nothing, and one that read
/// as `false` would take the keyboards.
fn dry_run(args: &[String]) -> Option<bool> {
    let Some(at) = args.iter().position(|a| a == "--dry-run") else {
        return Some(true);
    };
    match args.get(at + 1).map(String::as_str) {
        // The flag on its own, or with the next flag behind it.
        None | Some("true") => Some(true),
        Some(next) if next.starts_with("--") => Some(true),
        Some("false") => Some(false),
        Some(_) => None,
    }
}

fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// What a saved trace holds, without saying what was typed.
///
/// Counts and a time span rather than the keystrokes: the numbers are what says
/// whether a trace is worth replaying at all, and printing the keys would make
/// reading a trace and dumping a keylog the same command. The replay that does
/// show them lives in the end-to-end suite, where a person debugging asks for it
/// by name (`docs/adr/0009-trace-and-replay.md`).
fn trace_report(path: &Path) -> i32 {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            error!("cannot read {}: {error}", path.display());
            return 1;
        }
    };
    let trace = Trace::read(&bytes);

    let mut events = 0usize;
    let mut injected = 0usize;
    let mut timers = 0usize;
    let mut checkpoints = 0usize;
    let mut first = None;
    let mut last = None;
    for record in trace.records() {
        match record {
            favjit_core::trace::Record::CheckpointBegin { .. } => checkpoints += 1,
            favjit_core::trace::Record::Event(event) => {
                events += 1;
                first = first.or(Some(event.at));
                last = Some(event.at);
            }
            favjit_core::trace::Record::Injected { .. } => injected += 1,
            favjit_core::trace::Record::SetTimer(_) => timers += 1,
            _ => {}
        }
    }

    println!("checkpoints: {checkpoints}");
    println!("events: {events}");
    println!("injections: {injected}");
    println!("timer calls: {timers}");
    println!("records dropped from the start: {}", trace.evicted());
    println!("begins at a checkpoint: {}", trace.begins_at_a_checkpoint());
    match (first, last) {
        (Some(first), Some(last)) => println!(
            "span: {:.3}s of the host's clock",
            (last.as_nanos().saturating_sub(first.as_nanos())) as f64 / 1e9
        ),
        _ => println!("span: nothing happened"),
    }
    println!(
        "\nthis says nothing about what was typed. Replaying it does, and that is\n\
         `FAVJIT_TRACE={} cargo test -p favjit-e2e --test replay_a_file -- --nocapture`",
        path.display()
    );
    0
}

/// How often the control file is looked at.
///
/// Polled rather than watched through the file system's own notifications: the
/// answer is wanted within a moment of a menu item being chosen, a `stat` costs
/// nothing at this rate, and a watch would be a second thing that can fail while
/// holding the keyboards.
const CONTROL_POLL: Duration = Duration::from_millis(250);

/// Set when a `SIGTERM` arrives.
///
/// A flag and not the work itself: a handler runs on whatever thread the signal
/// lands on, and letting go of the virtual keyboard means writing to a socket
/// behind a mutex the interrupted thread may be holding.
static ASKED_TO_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn note_the_term(_signal: i32) {
    ASKED_TO_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

extern "C" {
    fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
}

/// Answer a `SIGTERM` by ending the run rather than by stopping where we stand.
///
/// The supervisor asks with `SIGTERM` and insists with `SIGKILL` two hundred
/// milliseconds later, and the ask is the only chance to put the virtual keyboard
/// back: a key that is down when the kill lands stays down, because the device
/// belongs to a daemon that outlives this process (ADR-0011). Default `SIGTERM`
/// handling ends the process without running a destructor, which is the same
/// outcome as the kill.
///
/// A stuck key is a second shape of what ADR-0008 rules out, and the worse one: a
/// dead keyboard is obvious and recoverable, a held-down modifier is neither.
fn stop_on_term(flag: &Stop) {
    // Handled rather than blocked, and noted rather than acted on: see the flag.
    unsafe { signal(15, note_the_term) };

    let raised = std::sync::Arc::clone(flag);
    std::thread::spawn(move || {
        while !ASKED_TO_STOP.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(CONTROL_POLL);
        }
        info!("asked to stop; letting the keys go and giving the keyboards back");
        raised.store(true, std::sync::atomic::Ordering::SeqCst);
    });
}

/// The one thing that ends a run early.
///
/// One flag for both reasons rather than one each, because the host's wait can only
/// come back for one: whether the run is ending because a person turned favjit off
/// or because the supervisor asked, what has to happen is the same and the log says
/// which it was.
type Stop = std::sync::Arc<std::sync::atomic::AtomicBool>;

fn stop_flag() -> Stop {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

/// Wedge on this event if the count is up, and never come back.
///
/// Stop the loop for good, once the deadline has passed.
///
/// Checked before an event is taken rather than after one arrives, so nothing is
/// pulled out of the stream and dropped: a run wedged this way leaves a trace whose
/// last event is the last one favjit actually handled, which is what makes the
/// trace worth having about a wedge at all.
///
/// A deadline rather than a count of keystrokes, because the interesting case is a
/// wedge while a key is held and a person can hold one across a deadline. Counting
/// would need the count to skip the supervisor's four probes a second, a flag on
/// each host wrapper to defer the stop past the event, and tests for both — a lot
/// of machinery inside the thing whose only job is to break on purpose.
fn wedge_if_due(wedge: Option<Instant>) {
    if wedge.is_none_or(|at| Instant::now() < at) {
        return;
    }
    warn!("wedged on purpose; the watchdog should kill this shortly");
    loop {
        std::thread::park();
    }
}

/// Injects for real.
struct Injecting {
    inner: MacOsHost,
    wedge: Option<Instant>,
}

impl SinkInputHost for Injecting {
    fn switched_on(&mut self) -> bool {
        self.inner.switched_on()
    }

    fn wait_until_on(&mut self) {
        self.inner.wait_until_on();
    }

    fn may_read_input(&mut self) -> bool {
        self.inner.may_read_input()
    }

    fn take_input(&mut self, suppress: bool) -> bool {
        self.inner.take_input(suppress)
    }

    fn ended(&mut self) -> Ended {
        self.inner.ended()
    }
}

impl Host for Injecting {
    fn next_event(&mut self) -> Option<HostEvent> {
        wedge_if_due(self.wedge);
        self.inner.next_event()
    }

    fn is_supervised(&mut self) -> bool {
        self.inner.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.inner.warn(message);
    }

    fn heartbeat(&mut self) {
        self.inner.heartbeat();
    }
}

impl IdentityStore for Injecting {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.inner.read()
    }

    fn make(&mut self) -> Option<Identity> {
        self.inner.make()
    }

    fn keep(&mut self, bytes: &[u8]) -> bool {
        self.inner.keep(bytes)
    }
}

impl SinkHost for Injecting {
    fn set_timer(&mut self, at: Option<HostInstant>) {
        self.inner.set_timer(at);
    }

    fn open_output(&mut self) -> bool {
        self.inner.open_output()
    }

    fn tune_output(&mut self) {
        self.inner.tune_output();
    }

    fn bind_link(&mut self, identity: &Identity) -> Option<Box<dyn LinkHost + Send>> {
        self.inner.bind_link(identity)
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.inner.run_alongside(work)
    }

    fn inject(&mut self, injected: Injected) {
        self.inner.inject(injected);
    }
}

/// Prints each conversion as it happens, and tallies them.
struct Reporting {
    inner: DryRun,
    seen: BTreeMap<String, usize>,
    wedge: Option<Instant>,
}

impl SinkInputHost for Reporting {
    fn switched_on(&mut self) -> bool {
        self.inner.switched_on()
    }

    fn wait_until_on(&mut self) {
        self.inner.wait_until_on();
    }

    fn may_read_input(&mut self) -> bool {
        self.inner.may_read_input()
    }

    fn take_input(&mut self, suppress: bool) -> bool {
        self.inner.take_input(suppress)
    }

    fn ended(&mut self) -> Ended {
        self.inner.ended()
    }
}

impl Host for Reporting {
    fn next_event(&mut self) -> Option<HostEvent> {
        wedge_if_due(self.wedge);
        self.inner.next_event()
    }

    fn is_supervised(&mut self) -> bool {
        self.inner.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.inner.warn(message);
    }

    fn heartbeat(&mut self) {
        self.inner.heartbeat();
    }
}

impl IdentityStore for Reporting {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.inner.read()
    }

    fn make(&mut self) -> Option<Identity> {
        self.inner.make()
    }

    fn keep(&mut self, bytes: &[u8]) -> bool {
        self.inner.keep(bytes)
    }
}

impl SinkHost for Reporting {
    fn set_timer(&mut self, at: Option<HostInstant>) {
        self.inner.set_timer(at);
    }

    fn open_output(&mut self) -> bool {
        self.inner.open_output()
    }

    fn tune_output(&mut self) {
        self.inner.tune_output();
    }

    fn bind_link(&mut self, identity: &Identity) -> Option<Box<dyn LinkHost + Send>> {
        self.inner.bind_link(identity)
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.inner.run_alongside(work)
    }

    fn inject(&mut self, injected: Injected) {
        // Both directions, because a stream shown one-way is misleading in the
        // one case that matters: a key that never reports a release looks
        // identical to one that does, and the modifier it converts to would be
        // left down at the OS.
        // Every key line is printed, and a pointer line only the first time its
        // shape appears: a pointer reports while the thumb is moving, which is
        // hundreds of times a second, and printing each one would bury the
        // keystrokes this mode exists to show. The tally at the end has the counts.
        let (line, every_time) = match injected {
            Injected::KeyDown { key, modifiers } => (format!("down {key:?} + {modifiers:?}"), true),
            Injected::KeyUp { key, modifiers } => (format!("up   {key:?} + {modifiers:?}"), true),
            Injected::Pointer(report) => (
                format!(
                    "move buttons={:#b} wheel={}",
                    report.buttons.bits(),
                    report.vertical_wheel != 0 || report.horizontal_wheel != 0
                ),
                false,
            ),
            // Every time, like a key: the modifiers changing on their own is what
            // lets go of a shift a keystroke borrowed, and a stream that showed
            // the borrowing without the letting go would look like a stuck one.
            Injected::Modifiers(keys) => {
                (format!("mods {:?}", keys.keys().collect::<Vec<_>>()), true)
            }
        };
        let n = self.seen.entry(line.clone()).or_default();
        *n += 1;
        if every_time || *n == 1 {
            println!("would send {line}");
        }
        self.inner.inject(injected);
    }
}

/// Ask for the permissions favjit needs, from a process that can be prompted.
///
/// The converter itself cannot do this. It is a daemon, and a request from a process
/// with no login session neither prompts nor leaves anything to switch on
/// (`docs/platform/macos/input-permissions.md`) — so this mode exists to be launched
/// as an application in somebody's session, where the dialogs can appear.
///
/// Accessibility first, because it is the request that can put a dialog on screen at
/// all, and on macOS 26 that grant can cover input monitoring too.
fn permission_check(out: Option<&str>) -> i32 {
    let mut said = String::new();
    said.push_str(&format!("accessibility: {}\n", ax_trusted(false)));
    said.push_str(&format!("input monitoring: {:?}\n", hid_access()));

    if !ax_trusted(false) {
        said.push_str(&format!(
            "accessibility after asking: {}\n",
            ax_trusted(true)
        ));
    }
    if hid_access() != HidAccess::Granted {
        said.push_str(&format!(
            "input monitoring after asking: {}\n",
            request_hid_access()
        ));
        said.push_str(&format!("input monitoring now: {:?}\n", hid_access()));
    }

    print!("{said}");
    // Also to a file when one is named, because the way this mode gets a session is
    // being launched with `open`, which keeps neither stdout nor the exit status.
    if let Some(path) = out {
        if let Err(error) = std::fs::write(path, &said) {
            error!("cannot write {path}: {error}");
            return 1;
        }
    }
    0
}

/// Watch the capture stream and report only what could not be named.
fn scan_usages(seconds: f64, config: Config, control: Option<PathBuf>) -> i32 {
    println!("scanning for {seconds:.0}s. No conversion runs and no key is printed.\n");

    let mut host = MacOsHost::new(config, Wanted::default(), control);
    host.until(Some(Instant::now() + Duration::from_secs_f64(seconds)));
    let ending = sink::watch(&mut host);

    // Read off the host afterwards rather than printed as they arrive, because a
    // usage repeats every time the key is pressed and this list is what the mode is
    // for: the device and the page come with it, since a usage number with no page
    // cannot be looked up anywhere.
    println!("\n{} unnamed usage(s):", host.unknown.len());
    for usage in &host.unknown {
        println!(
            "  device {} page {:#06x} usage {:#04x} ({})",
            usage.device.0, usage.page, usage.usage, usage.usage
        );
    }
    stopped(ending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_this_binary_does_not_know_is_refused() {
        // Refused rather than ignored, because every mode here is found by scanning
        // for its own flag: a misspelt one changes nothing and says nothing, and the
        // person who passed it goes on believing it took effect.
        assert_eq!(
            unknown_arguments(&[
                "favjit".to_string(),
                "--install".to_string(),
                "--port".to_string(),
                "9000".to_string(),
            ]),
            vec!["--port".to_string(), "9000".to_string()]
        );
        assert_eq!(
            unknown_arguments(&["favjit".to_string(), "--supress".to_string()]),
            vec!["--supress".to_string()]
        );
    }

    #[test]
    fn the_flags_this_binary_does_know_are_taken_with_their_values() {
        for args in [
            vec!["favjit", "--dry-run", "false", "--skip-built-in"],
            vec!["favjit", "--control", "/a/path", "--status"],
            vec!["favjit", "--pair", "--identity"],
            vec!["favjit", "--pointer-resolution", "80"],
            vec![
                "favjit",
                "--pointer-acceleration",
                "0.8",
                "--no-invert-scroll",
            ],
            vec!["favjit", "--trace-report", "/a/trace"],
            vec!["favjit", "--no-listen", "--seconds", "5", "--wedge"],
            // The two whose value is optional: a number when there is one, and the
            // next flag when there is not.
            vec!["favjit", "--usages", "5"],
            vec!["favjit", "--usages"],
            vec!["favjit", "--permission-check", "/a/file"],
            vec!["favjit", "--permission-check"],
        ] {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            assert_eq!(unknown_arguments(&args), Vec::<String>::new(), "{args:?}");
        }
    }

    #[test]
    fn naming_a_default_is_refused_rather_than_accepted_as_a_no_op() {
        // A run given one of these would do exactly what it does without it, so
        // taking it silently would tell the person their flag was read and acted on.
        for flag in ["--listen", "--invert-scroll"] {
            let args = vec!["favjit".to_string(), flag.to_string()];
            assert_eq!(unknown_arguments(&args), vec![flag.to_string()]);
        }
    }

    #[test]
    fn a_run_told_nothing_gets_the_pointer_this_relay_needs() {
        // The numbers are the ones the machine is used at. Left to macOS's own 400
        // dpi the relayed TrackPoint is unusably slow, and nothing about a slow
        // cursor says which of the two ends is responsible for it.
        let wanted = pointer_wanted(&["favjit".to_string()]);
        assert_eq!(wanted.resolution(), Some(POINTER_RESOLUTION));
        assert_eq!(wanted.acceleration(), Some(POINTER_ACCELERATION));

        let asked = pointer_wanted(&[
            "favjit".to_string(),
            "--pointer-resolution".to_string(),
            "120".to_string(),
        ]);
        assert_eq!(asked.resolution(), Some(120.0), "an asked-for value wins");
        assert_eq!(asked.acceleration(), Some(POINTER_ACCELERATION));
    }

    #[test]
    fn a_run_told_nothing_turns_the_wheel_over() {
        let tuning = pointer_tuning(&["favjit".to_string()]);
        assert!(tuning.invert_vertical_wheel);
        assert!(tuning.invert_horizontal_wheel);

        let asked = pointer_tuning(&["favjit".to_string(), "--no-invert-scroll".to_string()]);
        assert!(!asked.invert_vertical_wheel);
        assert!(!asked.invert_horizontal_wheel);
    }
}
