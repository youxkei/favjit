//! The source half of favjit, as a program.
//!
//! A shell: the loop is in `engine::source`, the machine is in `host-windows`, and
//! nothing here decides anything beyond which mode was asked for.
//!
//! One flag decides the mode, and for the same reason as on the Mac. A bare command
//! is a dry run, which reads the keyboards, refuses nothing and sends nothing, so it
//! changes nothing outside this process; `--dry-run false` opens the link and
//! forwards. There is no flag for refusing, because relaying without it sends every
//! keystroke to the Mac *and* leaves it on this machine — everything typed twice,
//! once on each screen — and refusing without relaying is a keyboard that has
//! stopped.

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use favjit_engine::pairing::Paired;
use favjit_engine::supervision::{HEARTBEAT, PROBE};

const DISCOVERY_WAIT: Duration = Duration::from_secs(2);
use favjit_engine::source::{self, Driving, Ending, Request, SWITCH_BACK, SWITCH_TO_THE_SINK};
use favjit_host_windows::{
    link, supervisor_end, tray, what_is_attached, Chord, Supervisor, WindowsHost,
};
use log::{error, info, warn};

use crate::install;

pub fn main() {
    // Two kinds of output, kept apart on purpose. What a mode exists to produce —
    // the device list, this machine's key — goes to stdout, where a person or a
    // pipe can read it. Everything about how the run is going goes through the
    // log, so raising the level cannot silence the answer.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();

    // Before any of it, because the modes below are found by scanning for their
    // own flag: a run given something this binary does not read would otherwise
    // do whatever the rest of the arguments say and never mention the one it
    // dropped.
    let unknown = unknown_arguments(&args);
    if !unknown.is_empty() {
        error!("nothing here reads {}", unknown.join(" "));
        std::process::exit(1);
    }

    if args.iter().any(|a| a == "--identity") {
        std::process::exit(identity());
    }
    if let Some(digits) = arg_after(&args, "--pair") {
        std::process::exit(pair(digits, &args));
    }
    if args.iter().any(|a| a == "--devices") {
        std::process::exit(devices());
    }
    if args.iter().any(|a| a == "--install") {
        std::process::exit(install::install());
    }
    if args.iter().any(|a| a == "--uninstall") {
        std::process::exit(install::uninstall());
    }
    for (flag, driving) in [
        ("--to-the-mac", Driving::TheSink),
        ("--back-here", Driving::ThisMachine),
    ] {
        if args.iter().any(|a| a == flag) {
            std::process::exit(move_the_keyboard(driving));
        }
    }

    // One flag decides the mode, so there is no combination to get wrong.
    let Some(dry_run) = dry_run(&args) else {
        error!("--dry-run takes true or false, or nothing at all for true");
        std::process::exit(1);
    };
    let request = match dry_run {
        true => Request::DryRun,
        false => Request::Relaying,
    };
    let ansi = args.iter().any(|a| a == "--ansi");
    let chord = Chord {
        switch_to_the_sink: favjit_hid::scancode::as_a_hook_reports(SWITCH_TO_THE_SINK),
        switch_back: favjit_hid::scancode::as_a_hook_reports(SWITCH_BACK),
    };
    // Said here rather than left to the host, which is handed the positions and
    // not the keys: a key with no position matches nothing, so the run carries on
    // with a chord short of a key rather than refusing to start.
    if chord.switch_to_the_sink.is_none() || chord.switch_back.is_none() {
        warn!(
            "no position on this keyboard produces one of the chord's keys; the chord that \
             moves the keyboard is short of a key"
        );
    }

    // Split into the moment and whether there is one, which is how the host holds
    // it: an absent moment would have every reading of the bound ask whether
    // there is one first (ADR-0006).
    let (deadline, bounded) = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| (Instant::now() + Duration::from_secs_f64(s), true))
        .unwrap_or_else(|| (Instant::now(), false));

    // A run that is not relaying opens no link, because it needs no identity and
    // no pinned sink: this is the first thing anyone runs, and finding out which
    // keys arrive should not need a machine that has been set up.
    let (has_a_sink, link) = match request != Request::DryRun {
        true => match the_link(&args) {
            Some(link) => (true, link),
            None => std::process::exit(1),
        },
        false => {
            info!("dry run: reading the keyboards, refusing nothing, sending nothing");
            (false, link::Link::new(None))
        }
    };

    let mut host = WindowsHost::start(chord, has_a_sink, link, supervisor());
    host.until(deadline, bounded);
    // Read the same way `source::run` is about to, for the one thing this
    // process says about it that `Host::warn` is not the place for: which
    // machine is presenting, not a fact about how the run is going.
    if request != Request::DryRun {
        if let Ok(identified) = favjit_engine::source::identify(&mut host) {
            info!("this machine's key is {}", identified.fingerprint);
        }
    }
    let mut region = match favjit_host_windows::trace_handle(favjit_engine::supervision::TRACE) {
        None => None,
        Some(handle) => match favjit_host_windows::map_trace_region(
            handle,
            favjit_engine::supervision::TRACE_BYTES,
        ) {
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
                    favjit_host_windows::no_region(handle, error).0
                );
                None
            }
        },
    };
    let (ending, seen) = source::run(
        &request,
        ansi,
        &mut host,
        region.as_mut().map(|region| region.bytes()),
    );
    report(&host, &seen);
    std::process::exit(said_about_ending(ending));
}

/// What to say about how the run ended, and what to exit with.
///
/// Which ending happened is [`source::run`]'s; what it means to whatever started
/// favjit is here, because an exit code is this program's contract with that and
/// not `engine`'s.
fn said_about_ending(ending: Ending) -> i32 {
    match ending {
        Ending::Relayed => 0,
        Ending::NoInput => {
            error!("this machine's keyboards could not be read, so there was nothing to relay");
            1
        }
        Ending::InputGone => {
            error!("reading this machine's keyboards stopped; nothing is being relayed");
            1
        }
        Ending::NoLink => {
            // Not an error to report twice: whatever refused to make a link has
            // already said which of its reasons it was.
            info!("there is nothing to relay to; stopping");
            1
        }
    }
}

/// The link, ready to be opened.
///
/// Neither an identity nor a pinned sink is read here: whether either is
/// missing is `source::run`'s own question, asked of the host it is given
/// once it has one (ADR-0006) rather than checked ahead of it by this
/// function, which has no host yet to ask.
fn the_link(args: &[String]) -> Option<link::Link> {
    let fixed = match arg_after(args, "--sink-address") {
        Some(address) => match resolve(address) {
            Some(address) => {
                info!("connecting to {address} rather than looking for it");
                Some(address)
            }
            None => {
                error!("cannot make sense of --sink-address {address}");
                return None;
            }
        },
        None => None,
    };
    Some(link::Link::new(fixed))
}

fn resolve(address: &str) -> Option<SocketAddr> {
    address.to_socket_addrs().ok()?.next()
}

/// This machine's own key, and which sink it will send input to.
///
/// On stdout, because it is the answer this mode exists to produce: what this
/// machine is, and whether pairing has given it anywhere to send input. Built
/// with the same host a relaying run would use, so the mode reads exactly what
/// that run would (ADR-0006).
/// Whichever end of the watchdog link this process was given, one read each.
///
/// Read here rather than inside the host: each end is a call into the machine
/// and a host makes one of those per operation (ADR-0006), so the two of them
/// are asked for one at a time and handed over together.
fn supervisor() -> Supervisor {
    Supervisor::on(supervisor_end(PROBE), supervisor_end(HEARTBEAT))
}

fn identity() -> i32 {
    let chord = Chord {
        switch_to_the_sink: favjit_hid::scancode::as_a_hook_reports(SWITCH_TO_THE_SINK),
        switch_back: favjit_hid::scancode::as_a_hook_reports(SWITCH_BACK),
    };
    let mut host = WindowsHost::start(chord, false, link::Link::new(None), supervisor());
    let identified = match favjit_engine::source::identify(&mut host) {
        Ok(identified) => identified,
        Err(error) => {
            error!("cannot read or make an identity: {error}");
            return 1;
        }
    };

    println!("this machine: {}", identified.fingerprint);
    match identified.sink {
        Some(sink) => println!("sending input to: {sink}"),
        None => println!("no sink is pinned, so there is nowhere to send input"),
    }
    0
}

/// Pair with the machine showing a code, and pin it (ADR-0004).
///
/// The six digits the Mac put on its screen, entered here. What they buy is the
/// exchange ADR-0004 decides: the keys cross under the code, this end pins the Mac's
/// and the Mac pins this one, and nothing about the code is needed again.
fn pair(digits: &str, args: &[String]) -> i32 {
    let Some(code) = code_from(digits) else {
        error!(
            "that is not a pairing code: expected the {} digits the Mac is showing",
            favjit_engine::pairing::DIGITS
        );
        return 1;
    };
    let mut identity_store = link::IdentityFile::new(link::identity_path());
    let identity = match favjit_engine::pairing::identity(&mut identity_store) {
        Ok(identity) => identity,
        Err(error) => {
            error!("no identity, so nothing to pair: {error}");
            return 1;
        }
    };
    let Some(address) = where_the_sink_is(args) else {
        return 1;
    };

    let mut host = favjit_host_windows::pairing::Pairing::new(address);
    match favjit_engine::pairing::pair_with(code, &identity, &mut host) {
        Paired::Pinned(sink) => {
            info!("paired with {sink}; input will go to that machine and to no other");
            0
        }
        Paired::WrongCode => {
            error!(
                "the code does not match what that machine is showing. It has spent the one it \
                 showed, so ask it for another"
            );
            1
        }
        Paired::NoSink => 1,
        Paired::Interrupted => {
            error!("the exchange stopped part way; the code is still good for another attempt");
            1
        }
        Paired::CannotKeep(trouble) => {
            error!(
                "cannot write down the machine this paired with: {}",
                trouble.0
            );
            1
        }
        // The other end's endings, which this one cannot reach: this machine opens
        // no listener, shows no code, and waits for nobody.
        Paired::CannotListen | Paired::NoCode | Paired::NoSource => 1,
    }
}

/// The digits as a code, or nothing if they are not one.
///
/// Exactly the length, and digits only: a code with a letter in it is a misreading
/// rather than something to try, and trying it would spend the Mac's code on it.
fn code_from(text: &str) -> Option<favjit_engine::pairing::Code> {
    let digits = text.trim().as_bytes();
    digits
        .iter()
        .all(u8::is_ascii_digit)
        .then(|| digits.try_into().ok())
        .flatten()
}

/// Where to pair with: the address given, or whatever mDNS answers.
fn where_the_sink_is(args: &[String]) -> Option<SocketAddr> {
    if let Some(given) = arg_after(args, "--sink-address") {
        return match resolve(given) {
            Some(address) => Some(address),
            None => {
                error!("cannot make sense of --sink-address {given}");
                None
            }
        };
    }
    let mut network = favjit_host_windows::mdns::Network;
    match favjit_engine::discovery::find(favjit_engine::link::PAIRING, DISCOVERY_WAIT, &mut network)
    {
        Some(sink) => Some(sink),
        None => {
            error!(
                "nothing is advertising {} — the Mac has to be showing a code for this to find \
                 it, or pass --sink-address",
                favjit_discovery::service(favjit_engine::link::PAIRING)
            );
            None
        }
    }
}

/// Ask a run that is already going to move the keyboard.
///
/// The terminal's half of the tray item (`docs/platform/windows/tray-item-as-its-own-program.md`), the way `favjit --disable` is the
/// terminal's half of the Mac's menu bar item. What it is for is the case the item is
/// hardest to reach in: while the keyboard is the Mac's, the pointer here is refused
/// along with the keys, so a person who wants it back and cannot chord has this, over a
/// remote shell if it comes to that.
fn move_the_keyboard(driving: Driving) -> i32 {
    let Some(run) = tray::running() else {
        error!("no favjit is running here, so there is no keyboard to move");
        return 1;
    };
    if !tray::ask(run, driving) {
        error!("the ask could not be posted to the run");
        return 1;
    }
    // Not read back. What the ask moves is a state inside a loop that owns it, and the
    // run decides where it lands — a keyboard asked for the Mac with no link waits for
    // one rather than going over.
    info!("asked for the keyboard to be {driving:?}");
    0
}

/// Every keyboard and mouse attached, with what a rule would match it on.
///
/// The vendor and product in both bases, because the sink's configuration is
/// written in decimal and every Windows path is written in hex, and converting by
/// hand is how a rule ends up naming a keyboard that does not exist.
fn devices() -> i32 {
    let mut listing = what_is_attached();
    let attached = favjit_engine::source::what_is_attached(&mut listing);
    if attached.is_empty() {
        println!("nothing is attached, or the list could not be read");
        return 1;
    }
    for device in favjit_engine::source::describe_devices(&attached) {
        let identity = match device.identity {
            Some((vendor, product)) => {
                format!("vendor {vendor:#06x} ({vendor}) product {product:#06x} ({product})")
            }
            None => String::from("no vendor or product: no rule can single it out"),
        };
        println!(
            "{} {identity}\n  {}",
            match device.keyboard {
                true => "keyboard",
                false => "mouse   ",
            },
            device.path,
        );
    }
    0
}

/// What the run saw, and what it did with it.
fn report(host: &WindowsHost, seen: &source::Seen) {
    println!("\nkeys captured: {}", seen.keys);
    println!("pointer reports captured: {}", seen.pointers);
    // Not "what a dry run would have sent": the source converts nothing, so that
    // number is the two above added up and says nothing they do not.
    println!("messages sent to the sink: {}", seen.sent.went);
    // Only where the two differ, which is a link that took some of what was
    // handed to it and not the ordinary case of one that took all of it.
    if seen.sent.handed > seen.sent.went {
        println!(
            "  {} more were handed to the link and did not go",
            seen.sent.handed - seen.sent.went
        );
    }

    // Only the pointer's, because only the pointer is refused one event at a time. What
    // refuses the keys is the registration that also delivers them, so "refused but not
    // captured" is not a state the keyboard can be in any more — which it was, and it
    // was a keyboard that had stopped
    // (docs/platform/windows/hooks-and-raw-input.md).
    let refused = host.refused();
    if refused > 0 {
        println!("\nrefused by the mouse hook: {refused} pointer events");
        if seen.pointers == 0 {
            println!(
                "  nothing arrived as raw input while they were being refused, so those \
                 movements reached neither machine"
            );
        }
    }

    // A make code no key is named for is not counted here: the table that names one is
    // `favjit_hid::scancode`'s and the run that reads it warns as each arrives, so what
    // says which positions do nothing at all while input is refused is the log.

    if !seen.absolute.is_empty() {
        println!("\npointers that report where they are rather than how far they moved:");
        for device in &seen.absolute {
            println!("  device {}", device.0);
        }
        println!("  their movement is not relayed; their buttons and wheel are");
    }
}

/// The flags that are followed by their value, and the ones that stand alone.
///
/// Listed rather than derived, because the modes are found by scanning for their
/// own flag: nothing else in this file knows the whole set, so nothing else could
/// tell a misspelling from a flag it simply does not handle.
// `--dry-run`'s value is optional, which this list already allows: what follows a
// flag is only taken as a value when it is not another flag.
const WITH_A_VALUE: [&str; 4] = ["--pair", "--sink-address", "--seconds", "--dry-run"];

const ON_THEIR_OWN: [&str; 7] = [
    "--identity",
    "--devices",
    "--ansi",
    "--install",
    "--uninstall",
    "--to-the-mac",
    "--back-here",
];

/// What this run was given that this binary cannot act on.
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

/// Whether this run sends nothing.
///
/// True when nothing was said, so that a bare command changes nothing outside this
/// process: `--dry-run false` is how a person asks for the run that refuses this
/// machine's input and forwards it, and asking for it is the point.
///
/// `None` for a value that is neither, rather than a guess: a misspelling that read
/// as `true` would look like favjit reading keyboards and forwarding nothing, and
/// one that read as `false` would take them away.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_flag_this_binary_reads_is_in_one_of_the_two_lists() {
        // The lists are what tells a misspelling from a flag this file does not
        // handle, so a flag added to `main` and not to a list would be reported as
        // unknown and then acted on anyway.
        for flag in WITH_A_VALUE {
            assert!(unknown_arguments(&[String::from("favjit"), String::from(flag)]).is_empty());
        }
        for flag in ON_THEIR_OWN {
            assert!(unknown_arguments(&[String::from("favjit"), String::from(flag)]).is_empty());
        }
    }

    #[test]
    fn a_flag_nothing_reads_is_reported_rather_than_ignored() {
        let args = ["favjit", "--dry-run", "false", "--supress"].map(String::from);
        assert_eq!(unknown_arguments(&args), vec![String::from("--supress")]);
    }
}
