//! `favjit` on macOS.
//!
//! ADR-0005 names this binary after the platform rather than the role, so it
//! survives the day a machine runs both. It is a shell: it reads the arguments,
//! builds the machine out of `host-macos`, hands both to `engine::sink::run`, and
//! turns what comes back into log lines and an exit code. The run itself, and the
//! order it brings the machine up in, are `engine`'s (ADR-0006).
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
// the host's monotonic stamp that `engine` reads deadlines off.
use favjit_engine::pairing::Paired;
use favjit_engine::sink::{self, Ending, InputConfig, Replayed, Request, Settings};
use favjit_engine::supervision::{HEARTBEAT, PROBE};
use favjit_engine::{pointer::Tuning, trace::Trace, DeviceMatch, Key, Layout};
use favjit_hid::report::{modifier_bit, Report as KeyboardReport};
use favjit_hid::Wanted;
use favjit_host::link::LinkHost;
use favjit_host::sink::{SinkHost, SinkInputHost};
use favjit_host::{
    DeviceId, Entropy, Host, HostEvent, IdentityStore, Instant as HostInstant, OutputReport,
};
use favjit_host_macos::{
    ax_trusted, ax_trusted_asking, control, hid_access, hid_systems, link, request_hid_access,
    supervisor_end, DryRun, HidAccess, MacOsHost, Supervisor, VIRTUAL_KEYBOARD_PRODUCT,
    VIRTUAL_KEYBOARD_VENDOR,
};
use log::{error, info, warn};

/// Keyboards to leave alone.
///
/// **The first is the device favjit's own output goes to** (`docs/platform/macos/output-through-a-virtual-hid-device.md`), named by
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
    if let Some(path) = arg_after(&args, "--trace-out") {
        std::process::exit(ask_for_the_trace(Path::new(path)));
    }
    if let Some(path) = arg_after(&args, "--trace-report") {
        std::process::exit(trace_report(Path::new(path)));
    }
    if let Some(path) = arg_after(&args, "--replay") {
        std::process::exit(replay(Path::new(path), arg_after(&args, "--from")));
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
        //
        // A machine of its own rather than the run's, because there is no run:
        // nothing here takes a keyboard or opens an output, and the pointers are
        // their own boundary for exactly that (ADR-0011).
        let wanted = pointer_wanted(&args);
        let mut host = MacOsHost::new(
            (wanted.resolution(), wanted.acceleration()),
            // Nothing to watch: this mode reads and writes device properties and
            // converts nothing, so being switched off is not a state it has.
            PathBuf::new(),
            supervisor(),
        );
        tune_output_pointer(&mut host);
        std::process::exit(list_pointers(&mut host));
    }
    // Empty where there is neither a path on the command line nor a console
    // session to derive one from: what a run does about being switched off is
    // read off the file, and a path nothing sits at reads as switched on.
    let control = arg_after(&args, "--control")
        .map(PathBuf::from)
        .or_else(|| {
            favjit_engine::control::console_home(control::sudo_user(), control::home())
                .map(|home| favjit_engine::control::path(&home))
        })
        .unwrap_or_default();
    if args.iter().any(|a| a == "--pair") {
        std::process::exit(pair());
    }
    if args
        .iter()
        .any(|a| a == "--disable" || a == "--enable" || a == "--status")
    {
        // Named rather than derived, unlike a run's: these three are what a
        // person types to switch converting off, and one that guessed the wrong
        // session's file would report the wrong machine as switched on.
        if control.as_os_str().is_empty() {
            error!("cannot tell whose control file to look at; pass --control PATH");
            std::process::exit(1);
        }
        let control = control.as_path();
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
    let input = InputConfig {
        ignore: IGNORE.to_vec(),
        skip_built_in: args.iter().any(|a| a == "--skip-built-in"),
    };

    // `--usages` runs no conversion at all and prints no key, only the page and
    // usage the tables have no name for. That is the whole point: finding out
    // where a key reports must not require logging what was typed.
    //
    // It watches every element rather than the ones the tables already name,
    // because a key filtered out for being unnamed is a key this mode cannot
    // find — which is exactly what it exists for
    // (`favjit_host::sink::SinkInputHost::read_device`'s own contract).
    if let Some(i) = args.iter().position(|a| a == "--usages") {
        let seconds: f64 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(30.0);
        std::process::exit(scan_usages(seconds, control));
    }

    let unmapped = favjit_hid::usage::UNMAPPED;
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
        Some((initial, interval)) => {
            info!("key repeat comes from the OS: {initial:?} then every {interval:?}")
        }
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
    let mut region = match favjit_host_macos::trace_descriptor(favjit_engine::supervision::TRACE) {
        None => None,
        Some(fd) => {
            match favjit_host_macos::Region::from_fd(fd, favjit_engine::supervision::TRACE_BYTES) {
                Ok(region) => {
                    info!("recording a trace into the supervisor's memory");
                    Some(region)
                }
                // Carried on without one: a trace is what a failure is read off
                // afterwards, and refusing to run without it would take the
                // keyboards away for the sake of the record of them (ADR-0009).
                Err(error) => {
                    error!(
                        "no trace, so nothing to read a failure off: {}",
                        favjit_host_macos::no_region(fd, error).0
                    );
                    None
                }
            }
        }
    };

    // Before the run, and whichever way it runs: a run waiting for converting to be
    // switched on has to answer a `SIGTERM` too, and that wait happens before
    // anything else.
    let stop = stop_flag();
    stop_on_term(&stop);
    wait_until_on(&control, &stop);

    if !dry_run {
        let wanted = pointer_wanted(&args);
        let mut inner = MacOsHost::new(
            (wanted.resolution(), wanted.acceleration()),
            control.clone(),
            supervisor(),
        );
        inner.stop_on(stop);

        let mut host = Injecting {
            inner,
            wedge,
            until,
            seizures: Seizures::default(),
        };
        let (ending, latency) = sink::run(
            &request,
            Layout::dudrack(),
            settings,
            input,
            &mut host,
            region.as_mut().map(|region| region.bytes()),
        );
        report(&host.seizures.borrow(), &latency);
        std::process::exit(stopped(ending));
    } else {
        info!("dry run: converting for real, injecting nothing");
        let mut inner = DryRun::new(control.clone(), supervisor());
        inner.stop_on(stop);
        let mut host = Reporting {
            inner,
            described: Described::default(),
            wedge,
            until,
            seizures: Seizures::default(),
        };
        let (ending, latency) = sink::run(
            &request,
            Layout::dudrack(),
            settings,
            input,
            &mut host,
            region.as_mut().map(|region| region.bytes()),
        );
        report(&host.seizures.borrow(), &latency);
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

/// Which devices this run tried to open, exclusively or not, and what came
/// back.
///
/// The wrapper's own and not the host's: opening a device is a
/// `favjit_host::sink::Capturing::take_device` call, and which ones this run
/// made and what came back is this binary's to remember for the person reading
/// the log afterward, not something asking a data-conversion call to keep for
/// it. Shared, because the capture the run holds is what makes the calls and
/// this is read once the run is over.
type Seizures = std::rc::Rc<std::cell::RefCell<Vec<(DeviceId, bool, i32)>>>;

/// One capture with what each seizure answered noted beside it.
struct Noting {
    reading: Box<dyn favjit_host::sink::Capturing>,
    seizures: Seizures,
}

impl favjit_host::sink::Capturing for Noting {
    fn take_device(&mut self, device: DeviceId, exclusive: bool) -> i32 {
        let code = self.reading.take_device(device, exclusive);
        self.seizures.borrow_mut().push((device, exclusive, code));
        code
    }

    fn read_device(&mut self, device: DeviceId) -> bool {
        self.reading.read_device(device)
    }

    fn next_held_device(&mut self) -> Option<favjit_host::sink::Holding> {
        self.reading.next_held_device()
    }

    fn give_it_back(&mut self, device: favjit_host::Handed) -> i32 {
        self.reading.give_it_back(device)
    }

    fn forget_what_was_given_back(&mut self, device: favjit_host::Handed, code: i32) {
        self.reading.forget_what_was_given_back(device, code)
    }
}

/// A capture with the seizures noted, and nothing where none started.
fn noting(
    reading: Option<Box<dyn favjit_host::sink::Capturing>>,
    seizures: &Seizures,
) -> Option<Box<dyn favjit_host::sink::Capturing>> {
    reading.map(|reading| {
        Box::new(Noting {
            reading,
            seizures: std::rc::Rc::clone(seizures),
        }) as Box<dyn favjit_host::sink::Capturing>
    })
}

fn report(seizures: &[(DeviceId, bool, i32)], latency: &favjit_engine::sink::Latency) {
    for &(device, exclusive, code) in seizures {
        let outcome = match code {
            0 if exclusive => "held exclusively".to_string(),
            0 => "open, shared with everything else".to_string(),
            NOT_PRIVILEGED => "refused: not privileged".to_string(),
            EXCLUSIVE_ACCESS => "refused: something else holds it".to_string(),
            code => format!("refused: {code:#010x}"),
        };
        info!("device {}: {outcome}", device.0);
    }
    say_the_latency(latency);
}

/// What favjit's own path cost, per segment.
///
/// Printed at the end rather than as it happens: writing a line per keystroke
/// would put a stderr write in the interactive path, and a latency report that
/// adds latency is measuring itself.
fn say_the_latency(latency: &favjit_engine::sink::Latency) {
    let segments = [
        ("hid stamp -> capture", &latency.arrival),
        ("capture -> report ready", &latency.pipeline),
        ("writing the report", &latency.post),
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
        let mut sorted = samples.to_vec();
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
    let identity = match favjit_engine::pairing::identity(&mut link::IdentityFile::default()) {
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
/// looking for a code cannot reach the link by mistake (ADR-0012) — and the key it
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

    let identity = match favjit_engine::pairing::identity(&mut link::IdentityFile::default()) {
        Ok(identity) => identity,
        Err(error) => {
            error!("no identity, so nothing to pair: {error}");
            return 1;
        }
    };
    let mut host = favjit_host_macos::pairing::Pairing::new();

    match favjit_engine::pairing::pair(&identity, &mut host) {
        Paired::Pinned(source) => {
            info!("paired with {source}; the link will accept input from that machine");
            0
        }
        Paired::WrongCode => {
            error!("the code that machine used is not the one shown here; run this again for a fresh one");
            1
        }
        Paired::CannotListen => {
            error!("cannot listen for the other machine");
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
        Paired::CannotKeep(trouble) => {
            error!("cannot write down what this pairing agreed: {}", trouble.0);
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
fn list_pointers(host: &mut dyn favjit_engine::pointer::PointerHost) -> i32 {
    use favjit_engine::pointer;

    let found = host
        .open_event_system()
        .or_else(|| host.open_simple_event_system())
        .map(|mut view| view.look())
        .unwrap_or_default();
    if found.is_empty() {
        println!("no pointing devices, or the event system would not say");
        return 1;
    }
    for mut device in found {
        println!(
            "vendor={:?} product={:?} {:?}\n  resolution={:?} dpi  acceleration={:?}",
            device.integer(pointer::VENDOR),
            device.integer("ProductID"),
            device
                .text(pointer::PRODUCT_NAME)
                .unwrap_or_else(|| String::from("(unnamed)")),
            device.fixed(pointer::RESOLUTION),
            device
                .fixed(pointer::ACCELERATION)
                .or_else(|| device.fixed(pointer::MOUSE_ACCELERATION)),
        );
    }
    0
}

/// How far the output device's counts should carry the cursor, and how the OS should
/// curve that, when this run was told nothing.
///
/// Numbers rather than nothing: they belong to the pointer this relays — one
/// TrackPoint, whose reports are mostly single counts — and left to the machine's own
/// 400 dpi that pointer is unusably slow (ADR-0011). A value that had to be passed
/// again at every install is one that goes missing the once it is forgotten, and the
/// symptom is a cursor that feels wrong rather than anything that says so.
const POINTER_RESOLUTION: f64 = 80.0;
const POINTER_ACCELERATION: f64 = 0.8;

/// How far the virtual device's counts should carry the cursor, as this run was
/// asked for it.
///
/// Read here and applied by the host: the numbers come from the arguments, and what
/// they mean to a device belongs to the platform that has the device.
/// The initial delay and the interval, as the OS holds them.
///
/// Read once at start rather than per keystroke: a rate that changed mid-repeat
/// would be a value `engine` read that no event carries, which is what ADR-0010
/// keeps out of the loop. Changing it takes a restart, and that is the trade.
///
/// The lookup, the step through the registry and the property read are one call
/// into the machine each, so they are asked for one at a time and put in order
/// here rather than inside the host (ADR-0006). Every `IOHIDSystem` is stepped
/// over until one answers, because which of them holds the parameters is not
/// something this can know without asking.
fn system_repeat() -> Option<(Duration, Duration)> {
    let systems = hid_systems()?;
    let parameters =
        core::iter::from_fn(|| systems.next()).find_map(|system| system.parameters())?;
    parameters
        .nanos("HIDInitialKeyRepeat")
        .zip(parameters.nanos("HIDKeyRepeat"))
}

/// Whichever end of the watchdog link this process was given, one read each.
///
/// Read here rather than inside the host: each end is a call into the machine
/// and a host makes one of those per operation (ADR-0006), so the two of them
/// are asked for one at a time and handed over together.
fn supervisor() -> Supervisor {
    Supervisor::on(supervisor_end(PROBE), supervisor_end(HEARTBEAT))
}

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
/// one (`docs/platform/macos/output-through-a-virtual-hid-device.md`). Setting it on the TrackPoint would tune a device whose reports
/// nothing but favjit ever sees.
fn tune_output_pointer(host: &mut dyn favjit_engine::pointer::PointerHost) {
    let tuned = favjit_engine::pointer::tune(host);
    if tuned.is_empty() {
        warn!(
            "no pointing device from vendor {VIRTUAL_KEYBOARD_VENDOR} to tune; is the virtual \
             device up yet?"
        );
    }
    for one in tuned {
        info!(
            "output pointer {:?}: resolution {:?} -> {:?}, acceleration {:?} -> {:?} (in {}){}",
            one.name.as_deref().unwrap_or("(unnamed)"),
            one.resolution.0,
            one.resolution.1,
            one.acceleration.0,
            one.acceleration.1,
            one.key,
            if one.accepted { "" } else { " (refused)" }
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
    // (ADR-0011), so the machine's own setting cannot answer for a relayed one. A run
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
const WITH_A_VALUE: [&str; 11] = [
    "--trace-out",
    "--trace-report",
    "--replay",
    "--from",
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
/// whether a trace is worth replaying at all, and a report that showed the keys
/// would make looking at a trace and dumping a keylog the same command. Seeing
/// them takes `--replay`, which is a person asking for it by name
/// (`docs/adr/0009-trace-and-replay.md`).
/// Ask the running supervisor for its recording, and write it here (ADR-0009).
///
/// **Asked for rather than found on disk.** The recording lives in the
/// supervisor's memory and nothing writes it out on its own, because a trace
/// holds whatever was typed in the window it covers — a file that appeared
/// without anybody asking would be a keylog the machine keeps as a matter of
/// course.
///
/// The bytes come back over the socket and this process writes them, at this
/// process's own privilege: the supervisor runs as root, and a path handed to it
/// would be a way to have root write anywhere.
fn ask_for_the_trace(path: &Path) -> i32 {
    use std::io::Read;

    let asked =
        std::os::unix::net::UnixStream::connect(favjit_engine::supervision::TRACE_ASKED_FOR);
    let mut asked = match asked {
        Ok(asked) => asked,
        // Told apart, because they send a person somewhere different: nobody
        // listening is a machine with no favjit running, and a refusal is a
        // favjit that is running and this not being root. A trace is a keylog, so
        // only root may take one — the socket is the supervisor's and it is
        // root's (ADR-0009).
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            error!(
                "only root may take a trace, because a trace is a keylog: try sudo favjit \
                 --trace-out {}",
                path.display()
            );
            return 1;
        }
        // Nobody listening is the ordinary state of a machine with no favjit
        // running, so it is said rather than raised.
        Err(error) => {
            error!(
                "nothing is supervising a run to ask: {} ({error})",
                favjit_engine::supervision::TRACE_ASKED_FOR
            );
            return 1;
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = asked.read_to_end(&mut bytes) {
        error!("the supervisor stopped part way through the trace: {error}");
        return 1;
    }
    if bytes.is_empty() {
        error!("the supervisor had no trace to give: it was started without a region for one");
        return 1;
    }
    let mut wrote = 0;
    for (recording, at) in each_recording(&bytes, path) {
        match std::fs::write(&at, recording) {
            Ok(()) => {
                warn!(
                    "wrote {} KiB to {}. It contains the keystrokes of the window it covers — \
                     everything typed on the captured keyboards, passwords included",
                    recording.len() / 1024,
                    at.display()
                );
                println!("read it with: favjit --trace-report {}", at.display());
                wrote += 1;
            }
            Err(error) => error!("could not write the trace to {}: {error}", at.display()),
        }
    }
    // Anything written at all, because the two are read separately: a person
    // holding the recording of the run that failed is not helped by being told
    // the other one could not be saved.
    i32::from(wrote == 0)
}

/// The recordings an answer carries, each with the path to write it to.
///
/// The run going on now first and the run before it second, which is the order
/// the supervisor writes them in and the whole of the agreement
/// (`favjit_engine::supervision::TRACES_HANDED_OVER`). The second is named by
/// adding to the first rather than by a flag of its own: a person asks once, and
/// which of the two answered the question is read off the report.
///
/// **A recording of nothing is left out.** The region a run that recorded nothing
/// leaves behind is zeroes, and a file of those is one somebody would replay and
/// take the empty answer from — while a supervisor that has only just come up has
/// nothing to say about the run before it at all.
fn each_recording<'a>(bytes: &'a [u8], path: &Path) -> Vec<(&'a [u8], PathBuf)> {
    let each = favjit_engine::supervision::TRACE_BYTES;
    let previous = path.with_file_name(format!(
        "{}.previous",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    bytes
        .chunks(each)
        .take(favjit_engine::supervision::TRACES_HANDED_OVER)
        .zip([path.to_path_buf(), previous])
        .filter(|(recording, _)| recording.iter().any(|byte| *byte != 0))
        .collect()
}

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
    let mut first = None;
    let mut last = None;
    // Only this machine's, because the span is read off one clock: the other
    // machine's records carry its own, and the two have no shared origin
    // (ADR-0009).
    for (side, record) in trace.records() {
        if side != favjit_engine::trace::Side::Sink {
            continue;
        }
        match record {
            favjit_engine::trace::Record::Event(event) => {
                events += 1;
                first = first.or(Some(event.at));
                last = Some(event.at);
            }
            favjit_engine::trace::Record::Injected { .. } => injected += 1,
            _ => {}
        }
    }

    println!("checkpoints to replay from: {}", trace.checkpoints());
    println!("events: {events}");
    println!("injections: {injected}");
    println!("records dropped from the start: {}", trace.evicted());
    println!("begins at a checkpoint: {}", trace.begins_at_a_checkpoint());
    match (first, last) {
        (Some(first), Some(last)) => println!(
            "span: {:.3}s of the host's clock",
            (last.nanos.saturating_sub(first.nanos)) as f64 / 1e9
        ),
        _ => println!("span: nothing happened"),
    }
    the_link_read_from_both_ends(&trace);
    the_output_connection(&trace);
    println!(
        "\nthis says nothing about what was typed. Replaying it does:\n\
         `favjit --replay {} [--from <checkpoint>]`",
        path.display()
    );
    0
}

/// What held the output device's connection up, and which way it went.
///
/// Counted rather than listed one frame at a time: the beats are the same record
/// over and over and there can be one every few seconds, so a listing would bury
/// the two lines that matter. Which of them was sent is kept apart, because a run
/// that beat steadily and a run that only ever answered what it was asked are the
/// same total and different states — the second holds the connection up for no
/// longer than the service keeps asking
/// (`docs/platform/macos/virtual-hid-device.md`).
fn the_output_connection(trace: &favjit_engine::trace::Reader<'_>) {
    let mut sent = [0usize; 3];
    let mut refused = Vec::new();
    let mut ended = Vec::new();
    for (_, record) in trace.records() {
        match record {
            favjit_engine::trace::Record::Served { frame, code: 0 } => {
                sent[frame as usize] += 1;
            }
            favjit_engine::trace::Record::Served { frame, code } => {
                refused.push(format!("  {} would not go, error {code}", frame.said()))
            }
            favjit_engine::trace::Record::OutputEnded { why } => {
                ended.push(
                    favjit_engine::trace::Lost::from_number(why)
                        .map(|lost| lost.said().to_owned())
                        // A recording from a build that named a reason this one
                        // does not: better read as unnamed than as whichever
                        // reason sits at that number here.
                        .unwrap_or_else(|| format!("a reason this build does not name ({why})")),
                );
            }
            _ => {}
        }
    }
    if sent.iter().all(|count| *count == 0) && refused.is_empty() && ended.is_empty() {
        return;
    }
    println!("\nthe output device's connection:");
    for (frame, count) in sent.iter().enumerate() {
        if let Some(frame) = favjit_engine::trace::Served::from_number(frame as u32) {
            println!("  {}: {count}", frame.said());
        }
    }
    for line in refused {
        println!("{line}");
    }
    for reason in ended {
        println!("  the loop serving it came back: {reason}");
    }
}

/// Why each link ended, read off the one recording that holds both machines'
/// records (ADR-0009).
///
/// **One file and no second machine.** The forwarding machine's records cross the
/// link and land in this one, including the ones it made while there was no link
/// — it keeps those and sends them once a session is up. So the question neither
/// end can answer alone is answered here: a link that dropped either had a write
/// fail on the sending end or was let go by this one, and both are in the same
/// reading.
fn the_link_read_from_both_ends(trace: &favjit_engine::trace::Reader<'_>) {
    let mut crossed = 0usize;
    let mut ends = 0usize;
    let mut lines = Vec::new();
    for (side, record) in trace.records() {
        let machine = match side {
            favjit_engine::trace::Side::Source => "the forwarding machine",
            favjit_engine::trace::Side::Sink => "this machine",
        };
        match record {
            favjit_engine::trace::Record::Sent { at, code: 0 } => {
                crossed += 1;
                lines.push(format!("  {at:>6}  sent"));
            }
            // The number is the whole point of printing this at all: a write that
            // ran out of time and a connection the other end reset are the same
            // failure without it.
            favjit_engine::trace::Record::Sent { at, code } => {
                ends += 1;
                lines.push(format!(
                    "  {at:>6}  the write failed on {machine}, error {code}"
                ));
            }
            favjit_engine::trace::Record::Received { at } => {
                lines.push(format!("  {at:>6}  arrived"));
            }
            favjit_engine::trace::Record::LinkEnded { why } => {
                ends += 1;
                let said = favjit_engine::link::Refused::from_number(why)
                    .map(|reason| reason.said().to_owned())
                    // A recording from a build that named a reason this one does
                    // not: better read as unnamed than as whichever reason sits
                    // at that number here.
                    .unwrap_or_else(|| format!("a reason this build does not name ({why})"));
                lines.push(format!(
                    "  {:>6}  {machine} let the connection go: {said}",
                    ""
                ));
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        return;
    }
    println!("\nthe link, from both ends:");
    for line in lines {
        println!("{line}");
    }
    println!("\n{crossed} record(s) crossed, and {ends} way(s) a link ended.");
    if ends == 0 {
        println!("no link ended, so what is here is one that was still up when this was taken");
    }
}

/// Convert a saved trace's events again, injecting nothing (ADR-0009).
///
/// Everything a person holding a trace needs comes out of this one command, and
/// on the machine the fault happened on: what the trace holds, what favjit did
/// with it, and the bytes that would have crossed. Splitting the last of those
/// off into the suite would mean the machine that saw the fault cannot answer
/// for it, and a person moving a keylog between machines to find out.
///
/// Through the same host a dry run uses, so what it prints is what that run
/// prints — a second way of describing a report would be a second thing to read.
/// Nothing is opened and nothing is taken: the events come off the trace, so
/// this reproduces a keyboard on a machine that has none attached.
///
/// Without `--from` it replays every checkpoint the trace holds, in turn. The
/// one whose replay is shortest while still showing the fault is the reproduction
/// worth keeping, and comparing them is how a person finds it.
fn replay(path: &Path, from: Option<&str>) -> i32 {
    let from = match from.map(|from| from.parse::<usize>()) {
        None => None,
        Some(Ok(from)) => Some(from),
        Some(Err(_)) => {
            error!("--from takes a checkpoint number");
            return 2;
        }
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            error!("cannot read {}: {error}", path.display());
            return 1;
        }
    };
    let trace = Trace::read(&bytes);
    // Said rather than left as a run that printed nothing: a region the process
    // never wrote into and a run that converted nothing look the same from here,
    // and "no keystrokes" is the answer a person would otherwise take away.
    if trace.checkpoints() == 0 {
        error!("{} holds no checkpoint to replay from", path.display());
        return 1;
    }
    // The records themselves, which `--trace-report` will not print: they are the
    // keystrokes, and this is the mode a person asked for them in. What they are
    // for is writing the case — an event names the device, the page and the usage
    // it arrived on, which is what a test has to script to reproduce it.
    println!("what it holds:");
    for record in trace.records() {
        println!("  {record:?}");
    }

    let checkpoints = from.map_or_else(|| (0..trace.checkpoints()).collect(), |from| vec![from]);
    for from in checkpoints {
        let mut host = Reporting {
            inner: DryRun::new(PathBuf::new(), supervisor()),
            described: Described::default(),
            wedge: None,
            until: None,
            seizures: Seizures::default(),
        };
        println!("\nfrom checkpoint {from}:");
        let writing = host.instead_of_the_output();
        match sink::replay(Layout::dudrack(), None, &trace, from, &mut host, writing) {
            Replayed::Events(events) => info!("{events} event(s) replayed"),
            Replayed::NoSuchCheckpoint => {
                error!("the trace holds no checkpoint {from}");
                return 1;
            }
        }
        // And the bytes each report carried, beside the lines above rather than
        // instead of them: what a case asserts is what crossed to the device
        // (ADR-0006), which a line naming the key it stands for cannot be turned
        // back into.
        println!("the bytes that would have crossed:");
        for (report, bytes) in &host.described.borrow().would_send {
            println!("  {report:?} {bytes:02x?}");
        }
        if let Some((checkpoint, events)) = trace.checkpoint(from) {
            as_a_script(&checkpoint, &events);
        }
    }
    0
}

/// The same segment written out as a `host-sim` script, to be pasted into the
/// end-to-end suite.
///
/// Printed rather than left to whoever reads the records above: what a trace
/// records is `engine`'s own decoded stream, which is the same vocabulary a
/// script is written in (ADR-0009), so the two differ only in spelling — and a
/// spelling done by hand at the far end is one that can be done wrong.
///
/// The keys the checkpoint was holding are pressed first, which is where a
/// segment's script cannot be exact: what a checkpoint keeps is that they were
/// down, not when they went down, so a tap-hold's own window starts here rather
/// than where it really did. A timer is left out for the same reason the gaps are
/// spelled as waits — the simulator raises one when a wait passes a deadline, so
/// scripting it as well would be asking for it twice.
///
/// What the run *should* have written is left for the person to state: it is the
/// question they are holding the trace to answer, and the bytes above are what it
/// actually wrote.
fn as_a_script(checkpoint: &favjit_engine::trace::Checkpoint, events: &[favjit_engine::HostEvent]) {
    println!("as a script:");
    println!("    let mut mac = SimHost::new();");
    for info in &checkpoint.devices {
        println!("    mac.{};", attach(info));
    }
    if !checkpoint.held.is_empty() {
        println!("    // down when this checkpoint was taken:");
    }
    for &(device, key, _) in &checkpoint.held {
        println!("    mac.press({}, Key::{key:?});", device_id(device));
    }

    let mut at = events.first().map(|event| event.at.nanos);
    for event in events {
        if let Some(previous) = at {
            let gap = event.at.nanos.saturating_sub(previous);
            if gap > 0 {
                println!("    mac.advance(Duration::from_micros({}));", gap / 1_000);
            }
        }
        at = Some(event.at.nanos);
        if let Some(call) = scripted(&event.kind) {
            println!("    mac.{call};");
        }
    }
    println!(
        "    sink::run(&Request::Injecting {{ listen: false }}, Layout::dudrack(), None, \
         InputConfig::default(), &mut mac, None);"
    );
}

/// One event as the call a script makes for it, where there is one.
fn scripted(kind: &favjit_engine::EventKind) -> Option<String> {
    Some(match kind {
        favjit_engine::EventKind::DeviceAttached(info) => attach(info),
        favjit_engine::EventKind::DeviceDetached(device) => {
            format!("detach({})", device_id(*device))
        }
        favjit_engine::EventKind::KeyDown { device, key } => {
            format!("press({}, Key::{key:?})", device_id(*device))
        }
        favjit_engine::EventKind::KeyUp { device, key } => {
            format!("release({}, Key::{key:?})", device_id(*device))
        }
        favjit_engine::EventKind::Pointer { device, report } => format!(
            "pointer({}, PointerReport {{ dx: {}, dy: {}, vertical_wheel: {}, \
             horizontal_wheel: {}, buttons: {} }})",
            device_id(*device),
            report.dx,
            report.dy,
            report.vertical_wheel,
            report.horizontal_wheel,
            buttons(report.buttons),
        ),
        favjit_engine::EventKind::Probe => "probe()".to_string(),
        favjit_engine::EventKind::Timer => return None,
        // `EventKind` is `#[non_exhaustive]`, so a kind added to it reaches this
        // rather than the build: a script line invented for one nothing here
        // knows would be a case that says something the run never did.
        other => format!("script(/* {other:?} has no scripted form here */)"),
    })
}

/// A button set as the expression that builds it.
///
/// Asked about the buttons the set has room for and no further: it holds them in
/// a `u32`, and a number past that is a shift nothing answers.
fn buttons(held: favjit_engine::Buttons) -> String {
    (1..=32)
        .filter(|button| held.holds(*button))
        .fold("Buttons::NONE".to_string(), |out, button| {
            format!("{out}.with({button})")
        })
}

/// One attached keyboard as the call a script makes for it.
///
/// The machine's own words rather than what the run read off them: a script says
/// which keyboard a machine has, and the properties it presents are that
/// machine's to state (ADR-0006). So the three shapes a run can end up with map
/// onto the three attachments that produce them, and a shape none of them
/// covers — a keyboard reporting the Mac's own bus *and* a USB identity — is
/// written as the properties themselves.
fn attach(info: &favjit_engine::DeviceInfo) -> String {
    match (info.is_built_in, info.vendor_id, info.product_id) {
        (true, None, None) => format!("attach_built_in({})", device_id(info.id)),
        (false, Some(vendor), Some(product)) => format!(
            "attach_external({}, {vendor}, {product})",
            device_id(info.id)
        ),
        (false, None, None) => format!("attach_anonymous({})", device_id(info.id)),
        // Written as the properties themselves, through the machine's own
        // escape hatch: no attachment describes a keyboard reporting the Mac's
        // own bus *and* a USB identity, and a line that reached for the nearest
        // one would script a keyboard the trace never held.
        (built_in, vendor, product) => format!(
            "script(EventKind::HidDeviceFound {{ device: {}, primary_usage_page: Some(0x01), \
             primary_usage: Some(0x06), transport: Some(String::from({:?})), product: None, \
             vendor_id: {}, product_id: {} }})",
            device_id(info.id),
            match built_in {
                true => "FIFO",
                false => "USB",
            },
            widened(vendor),
            widened(product),
        ),
    }
}

/// A vendor or product id as the expression a script writes it with.
///
/// Widened, because IOKit holds these as numbers with more room than a USB id
/// has and that is the shape a machine reports them in.
fn widened(id: Option<u16>) -> String {
    match id {
        Some(id) => format!("Some({id})"),
        None => String::from("None"),
    }
}

fn device_id(device: DeviceId) -> String {
    format!("DeviceId({})", device.0)
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
/// belongs to a daemon that outlives this process (`docs/platform/macos/output-through-a-virtual-hid-device.md`). Default `SIGTERM`
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

/// Wait here, answering a `SIGTERM`, until converting is switched on — then
/// exit rather than proceed into the rest of `main`.
///
/// Nothing here has taken a keyboard to give back, so ending on a term is
/// exiting outright rather than the run's own path through `Ending`: the
/// message that a term arrived is [`stop_on_term`]'s own thread's to print,
/// on whichever side of this wait it lands on.
///
/// Waiting rather than ending outright while off saves launchd a restart it
/// would otherwise do for no reason (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`). But once switched back on,
/// this process still exits rather than going on to seize the keyboards
/// itself: capture was never started while it waited here, so continuing
/// would be starting it from a process that has already been running for
/// some time rather than from the fresh one launchd hands the job to next.
fn wait_until_on(control: &Path, stop: &Stop) {
    if control::is_converting(control) {
        return;
    }
    info!("converting is off; waiting for it to be switched on");
    while !control::is_converting(control) {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            std::process::exit(0);
        }
        std::thread::sleep(CONTROL_POLL);
    }
    info!("converting was switched on; this run is over and the next one starts afresh");
    std::process::exit(0);
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

/// The earlier of `engine`'s own deadline and `--seconds`' bound, so a run given
/// one stops at it whatever the wait `engine` asked for was — a bound this
/// process was handed on its command line is not something `engine`'s timer
/// could ever have known to ask about.
fn capped(deadline: HostInstant, until: Option<HostInstant>) -> HostInstant {
    until.map_or(deadline, |until| deadline.min(until))
}

/// `at`, on the clock this host itself reports.
///
/// Computed fresh from how much real time is left rather than converted once
/// and stored: [`HostInstant`]'s own origin is nobody's business but the
/// host's (ADR-0006), so the only honest way to place a wall-clock deadline
/// on it is to ask the host what time it is right as the real one is read.
fn on_this_clock(host: &mut impl Host, at: Instant) -> HostInstant {
    let remaining = at.saturating_duration_since(Instant::now());
    let nanos = u64::try_from(remaining.as_nanos()).unwrap_or(u64::MAX);
    HostInstant {
        nanos: host.now().nanos.saturating_add(nanos),
    }
}

/// Injects for real.
struct Injecting {
    inner: MacOsHost,
    wedge: Option<Instant>,
    until: Option<Instant>,
    /// The wrapper's own, not the host's: which devices this run tried to open
    /// and what came back is worth telling the person afterward, and a
    /// data-conversion call has nothing of its own to remember it in.
    seizures: Seizures,
}

impl SinkInputHost for Injecting {
    fn switched_on(&mut self) -> bool {
        self.inner.switched_on()
    }

    fn may_read_input(&mut self) -> bool {
        self.inner.may_read_input()
    }

    fn request_input_permission(&mut self) -> bool {
        self.inner.request_input_permission()
    }

    fn output_connected(&mut self) -> bool {
        self.inner.output_connected()
    }

    /// Also true past `until`: `next_event` alone would return `None` forever
    /// with nothing telling the loop that is the run ending on purpose rather
    /// than an ordinary empty wait.
    fn stop_requested(&mut self) -> bool {
        self.inner.stop_requested() || self.until.is_some_and(|at| Instant::now() >= at)
    }

    fn look_for_devices(
        &mut self,
        work: favjit_host::capture::SinkLoop,
    ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
        noting(self.inner.look_for_devices(work), &self.seizures)
    }
}

impl Host for Injecting {
    fn now(&mut self) -> HostInstant {
        self.inner.now()
    }

    fn next_event(&mut self, deadline: HostInstant) -> Option<HostEvent> {
        wedge_if_due(self.wedge);
        let mine = self.until.map(|at| on_this_clock(&mut self.inner, at));
        self.inner.next_event(capped(deadline, mine))
    }

    fn is_supervised(&mut self) -> bool {
        self.inner.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.inner.warn(message);
    }

    fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
        self.inner.heartbeat()
    }
}

impl IdentityStore for Injecting {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.inner.read()
    }

    fn make_directory(&mut self) -> Result<(), favjit_host::Trouble> {
        self.inner.make_directory()
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, favjit_host::Trouble> {
        self.inner.open()
    }
}

impl Entropy for Injecting {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.inner.fill(into)
    }
}

impl favjit_host::PointerHost for Injecting {
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
        self.inner.wanted_pointer_feel()
    }

    fn output_vendor(&mut self) -> i64 {
        self.inner.output_vendor()
    }

    fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.inner.open_event_system()
    }

    fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.inner.open_simple_event_system()
    }
}

impl SinkHost for Injecting {
    fn reach_the_output(
        &mut self,
    ) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
        self.inner.reach_the_output()
    }

    fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.inner.run_output_alongside(work)
    }

    fn bind_link(&mut self) -> Option<Box<dyn LinkHost + Send>> {
        self.inner.bind_link()
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.inner.run_alongside(work)
    }

    fn instead_of_the_output(&mut self) -> Box<dyn favjit_host::sink::Injecting> {
        self.inner.instead_of_the_output()
    }
}

/// Prints each conversion as it happens, and tallies them.
struct Reporting {
    inner: DryRun,
    /// Where the reports go, shared with the writer the run holds: the writer is
    /// what the run writes through, and what it wrote is read here once the run
    /// is over.
    described: Described,
    wedge: Option<Instant>,
    until: Option<Instant>,
    seizures: Seizures,
}

/// What a run that injects nothing wrote, and what has already been said about
/// it.
type Described = std::rc::Rc<std::cell::RefCell<Describing>>;

#[derive(Default)]
struct Describing {
    /// Each line and how many times it has been printed.
    seen: BTreeMap<String, usize>,
    /// The keyboard report before the last one written, so a live line can name
    /// what changed rather than only the report's whole state.
    last: KeyboardReport,
    /// Every report as it was written, for the bytes to be shown afterwards.
    would_send: Vec<(OutputReport, Vec<u8>)>,
}

/// Where a run that injects nothing writes, which is stdout and this list.
struct Describes(Described);

impl favjit_host::sink::Injecting for Describes {
    fn send_report(&mut self, report: OutputReport, bytes: &[u8]) -> i32 {
        let mut describing = self.0.borrow_mut();
        for line in describing.describe(report, bytes) {
            let n = describing.seen.entry(line.clone()).or_default();
            *n += 1;
            // Every keyboard line, and a pointer line only the first time its
            // shape appears: a pointer reports while the thumb is moving, which
            // is hundreds of times a second, and printing each one would bury
            // the keystrokes this mode exists to show — which is what the count
            // is kept for, since the line itself is the same one every time.
            if report != OutputReport::Pointing || *n == 1 {
                println!("would send {line}");
            }
        }
        describing.would_send.push((report, bytes.to_vec()));
        0
    }
}

impl SinkInputHost for Reporting {
    fn switched_on(&mut self) -> bool {
        self.inner.switched_on()
    }

    fn may_read_input(&mut self) -> bool {
        self.inner.may_read_input()
    }

    fn request_input_permission(&mut self) -> bool {
        self.inner.request_input_permission()
    }

    fn output_connected(&mut self) -> bool {
        self.inner.output_connected()
    }

    fn stop_requested(&mut self) -> bool {
        self.inner.stop_requested() || self.until.is_some_and(|at| Instant::now() >= at)
    }

    fn look_for_devices(
        &mut self,
        work: favjit_host::capture::SinkLoop,
    ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
        noting(self.inner.look_for_devices(work), &self.seizures)
    }
}

impl Host for Reporting {
    fn now(&mut self) -> HostInstant {
        self.inner.now()
    }

    fn next_event(&mut self, deadline: HostInstant) -> Option<HostEvent> {
        wedge_if_due(self.wedge);
        let mine = self.until.map(|at| on_this_clock(&mut self.inner, at));
        self.inner.next_event(capped(deadline, mine))
    }

    fn is_supervised(&mut self) -> bool {
        self.inner.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.inner.warn(message);
    }

    fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
        self.inner.heartbeat()
    }
}

impl IdentityStore for Reporting {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.inner.read()
    }

    fn make_directory(&mut self) -> Result<(), favjit_host::Trouble> {
        self.inner.make_directory()
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, favjit_host::Trouble> {
        self.inner.open()
    }
}

impl Entropy for Reporting {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.inner.fill(into)
    }
}

impl favjit_host::PointerHost for Reporting {
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
        self.inner.wanted_pointer_feel()
    }

    fn output_vendor(&mut self) -> i64 {
        self.inner.output_vendor()
    }

    fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.inner.open_event_system()
    }

    fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.inner.open_simple_event_system()
    }
}

impl SinkHost for Reporting {
    fn reach_the_output(
        &mut self,
    ) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
        self.inner.reach_the_output()
    }

    fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.inner.run_output_alongside(work)
    }

    fn bind_link(&mut self) -> Option<Box<dyn LinkHost + Send>> {
        self.inner.bind_link()
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.inner.run_alongside(work)
    }

    fn instead_of_the_output(&mut self) -> Box<dyn favjit_host::sink::Injecting> {
        Box::new(Describes(std::rc::Rc::clone(&self.described)))
    }
}

impl Describing {
    /// What changed in this report, one line per key or modifier — or, for a
    /// pointer, one line for the whole of it: nothing here can say "the thumb
    /// moved by this much" more usefully than the report itself does.
    fn describe(&mut self, report: OutputReport, bytes: &[u8]) -> Vec<String> {
        match report {
            OutputReport::Keyboard => {
                let report = KeyboardReport::from_bytes(bytes).expect("a keyboard report");
                let lines = keyboard_diff(&self.last, &report);
                self.last = report;
                lines
            }
            OutputReport::Pointing => {
                let report =
                    favjit_hid::report::pointing_from_bytes(bytes).expect("a pointing report");
                vec![format!(
                    "move buttons={:#b} wheel={}",
                    report.buttons.bits(),
                    report.vertical_wheel != 0 || report.horizontal_wheel != 0
                )]
            }
            // A control page, printed by its byte count rather than decoded:
            // the Dudrack layout's rules do not reach one, so naming the exact
            // field here would be a table kept for a case this mode never sees.
            other => vec![format!("{other:?}: {} byte(s)", bytes.len())],
        }
    }
}

/// The keys and modifiers this report added or let go of since the last one,
/// one line each — read off the usages and the modifier byte rather than
/// assumed, since a live report is exactly the case where a guess and the
/// device disagreeing would go unnoticed.
fn keyboard_diff(before: &KeyboardReport, after: &KeyboardReport) -> Vec<String> {
    const MODIFIERS: [Key; 8] = [
        Key::LeftControl,
        Key::LeftShift,
        Key::LeftOption,
        Key::LeftCommand,
        Key::RightControl,
        Key::RightShift,
        Key::RightOption,
        Key::RightCommand,
    ];

    let mut lines = Vec::new();
    for key in MODIFIERS {
        let bit = modifier_bit(key).expect("a modifier key");
        match (before.modifiers & bit != 0, after.modifiers & bit != 0) {
            (false, true) => lines.push(format!("down {key:?}")),
            (true, false) => lines.push(format!("up   {key:?}")),
            _ => {}
        }
    }
    for usage in after.keys.usages() {
        if !before.keys.holds(usage) {
            lines.push(format!("down {}", named_usage(usage)));
        }
    }
    for usage in before.keys.usages() {
        if !after.keys.holds(usage) {
            lines.push(format!("up   {}", named_usage(usage)));
        }
    }
    lines
}

/// The keyboard-page key this usage names, or the usage itself when the
/// layout has no name for it — a live report is read off the same table
/// `engine` decodes one with, not guessed at.
fn named_usage(usage: u16) -> String {
    match favjit_hid::usage::named(favjit_hid::page::KEYBOARD_OR_KEYPAD, u32::from(usage)) {
        Some(key) => format!("{key:?}"),
        None => format!("usage {usage:#04x}"),
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
    said.push_str(&format!("accessibility: {}\n", ax_trusted()));
    said.push_str(&format!("input monitoring: {:?}\n", hid_access()));

    if !ax_trusted() {
        said.push_str(&format!(
            "accessibility after asking: {}\n",
            ax_trusted_asking()
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
fn scan_usages(seconds: f64, control: PathBuf) -> i32 {
    println!("scanning for {seconds:.0}s. No conversion runs and no key is printed.\n");

    /// Reads every raw element [`sink::watch`]'s own resolving passes over
    /// without converting it — this wrapper's to keep, since a data-conversion
    /// call has nothing of its own to remember an unnamed usage in.
    struct ScanningUsages {
        inner: MacOsHost,
        until: Instant,
        unknown: Vec<(DeviceId, u32, u32)>,
    }

    impl SinkInputHost for ScanningUsages {
        fn switched_on(&mut self) -> bool {
            self.inner.switched_on()
        }

        fn may_read_input(&mut self) -> bool {
            self.inner.may_read_input()
        }

        fn request_input_permission(&mut self) -> bool {
            self.inner.request_input_permission()
        }

        fn output_connected(&mut self) -> bool {
            self.inner.output_connected()
        }

        fn stop_requested(&mut self) -> bool {
            self.inner.stop_requested() || Instant::now() >= self.until
        }

        fn look_for_devices(
            &mut self,
            work: favjit_host::capture::SinkLoop,
        ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
            self.inner.look_for_devices(work)
        }
    }

    impl Host for ScanningUsages {
        fn now(&mut self) -> HostInstant {
            self.inner.now()
        }

        fn next_event(&mut self, deadline: HostInstant) -> Option<HostEvent> {
            let mine = on_this_clock(&mut self.inner, self.until);
            let event = self.inner.next_event(capped(deadline, Some(mine)))?;
            if let favjit_host::EventKind::HidValue {
                device,
                page,
                usage,
                ..
            } = event.kind
            {
                if favjit_hid::usage::named(page, usage).is_none()
                    && !favjit_hid::usage::pointer(page, usage)
                {
                    let seen = (device, page, usage);
                    if !self.unknown.contains(&seen) {
                        self.unknown.push(seen);
                    }
                }
            }
            Some(event)
        }

        fn is_supervised(&mut self) -> bool {
            self.inner.is_supervised()
        }

        fn warn(&mut self, message: core::fmt::Arguments) {
            self.inner.warn(message);
        }

        fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
            self.inner.heartbeat()
        }
    }

    let mut host = ScanningUsages {
        inner: MacOsHost::new((None, None), control, supervisor()),
        until: Instant::now() + Duration::from_secs_f64(seconds),
        unknown: Vec::new(),
    };
    let ending = sink::watch(&mut host);

    // Read off the wrapper afterwards rather than printed as they arrive,
    // because a usage repeats every time the key is pressed and this list is
    // what the mode is for.
    println!("\n{} unnamed usage(s):", host.unknown.len());
    for (device, page, usage) in &host.unknown {
        println!(
            "  device {} page {page:#06x} usage {usage:#04x} ({usage})",
            device.0
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
