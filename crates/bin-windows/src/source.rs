//! The source half of favjit, as a program.
//!
//! A shell: the loop is in `core::source`, the machine is in `host-windows`, and
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

use favjit_core::pairing::Paired;
use favjit_core::source::{self, Ending, Request};
use favjit_host_windows::{announced_as, attached, link, Config, WindowsHost};
use log::{error, info};

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

    // One flag decides the mode, so there is no combination to get wrong.
    let Some(dry_run) = dry_run(&args) else {
        error!("--dry-run takes true or false, or nothing at all for true");
        std::process::exit(1);
    };
    let request = match dry_run {
        true => Request::DryRun,
        false => Request::Relaying,
    };
    let config = Config {
        ansi: args.iter().any(|a| a == "--ansi"),
    };

    let deadline = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| Instant::now() + Duration::from_secs_f64(s));

    // A run that is not relaying reads nothing off disk and writes nothing there,
    // because it needs no identity and no pinned sink: this is the first thing
    // anyone runs, and finding out which keys arrive should not need a machine that
    // has been set up.
    let link = match request != Request::DryRun {
        true => match the_link(&args) {
            Some(link) => Some(link),
            None => std::process::exit(1),
        },
        false => {
            info!("dry run: reading the keyboards, refusing nothing, sending nothing");
            None
        }
    };

    let mut host = WindowsHost::start(config, link);
    host.until(deadline);
    if let Some(fingerprint) = host.fingerprint() {
        info!("this machine's key is {fingerprint}");
    }
    let ending = source::run(&request, &mut host);
    report(&host);
    std::process::exit(said_about_ending(ending));
}

/// What to say about how the run ended, and what to exit with.
///
/// Which ending happened is [`source::run`]'s; what it means to whatever started
/// favjit is here, because an exit code is this program's contract with that and
/// not `core`'s.
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
/// Refuses to build one without a pinned sink: `IK` puts the responder's key into
/// the first handshake message, so there is nothing to say to a machine this one
/// has not pinned (ADR-0004). Saying so and stopping is the loud failure that
/// decision asks for.
fn the_link(args: &[String]) -> Option<link::Link> {
    let identity = match link::identity() {
        Ok(identity) => identity,
        Err(error) => {
            error!("no identity, so no link: {error}");
            return None;
        }
    };
    let path = link::sink_path();
    let Some(sink) = link::read_sink(&path) else {
        error!(
            "no sink is pinned in {}: run favjit --pair on the Mac to put a code on its screen, \
             then favjit --pair <those digits> here. The Mac has to be listening as well, which \
             is what sudo favjit --install sets up there",
            path.display()
        );
        return None;
    };

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
    Some(link::Link::new(identity, sink, fixed))
}

fn resolve(address: &str) -> Option<SocketAddr> {
    address.to_socket_addrs().ok()?.next()
}

/// This machine's own key, and which sink it will send input to.
///
/// On stdout, because it is the answer this mode exists to produce: what this
/// machine is, and whether pairing has given it anywhere to send input.
fn identity() -> i32 {
    let identity = match link::identity() {
        Ok(identity) => identity,
        Err(error) => {
            error!(
                "cannot read or make {}: {error}",
                link::identity_path().display()
            );
            return 1;
        }
    };

    println!("this machine: {}", identity.fingerprint());
    match link::read_sink(&link::sink_path()) {
        Some(sink) => println!("sending input to: {}", favjit_core::pairing::hex(&sink)),
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
            favjit_core::pairing::DIGITS
        );
        return 1;
    };
    let identity = match link::identity() {
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
    match favjit_core::pairing::pair_with(code, &identity, &mut host) {
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
        Paired::CannotKeep => {
            error!("cannot write {}", link::sink_path().display());
            1
        }
        // The other end's endings, which this one cannot reach: this machine shows
        // no code, so it has none to fail at making and nobody to wait for.
        Paired::NoCode | Paired::NoSource => 1,
    }
}

/// The digits as a code, or nothing if they are not one.
///
/// Exactly the length, and digits only: a code with a letter in it is a misreading
/// rather than something to try, and trying it would spend the Mac's code on it.
fn code_from(text: &str) -> Option<favjit_core::pairing::Code> {
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
    match link::find_pairing() {
        Ok(Some(address)) => Some(address),
        Ok(None) => {
            error!(
                "nothing is advertising {} — the Mac has to be showing a code for this to find \
                 it, or pass --sink-address",
                favjit_core::discovery::service(favjit_core::link::PAIRING)
            );
            None
        }
        Err(error) => {
            error!("cannot look for the sink: {error}");
            None
        }
    }
}

/// Every keyboard and mouse attached, with what a rule would match it on.
///
/// The vendor and product in both bases, because the sink's configuration is
/// written in decimal and every Windows path is written in hex, and converting by
/// hand is how a rule ends up naming a keyboard that does not exist.
fn devices() -> i32 {
    let attached = attached();
    if attached.is_empty() {
        println!("nothing is attached, or the list could not be read");
        return 1;
    }
    for (keyboard, path) in attached {
        let info = announced_as(&path);
        let identity = match (info.vendor_id, info.product_id) {
            (Some(vendor), Some(product)) => {
                format!("vendor {vendor:#06x} ({vendor}) product {product:#06x} ({product})")
            }
            _ => String::from("no vendor or product: no rule can single it out"),
        };
        println!(
            "{} {identity}\n  {path}",
            match keyboard {
                true => "keyboard",
                false => "mouse   ",
            }
        );
    }
    0
}

/// What the run saw, and what it did with it.
fn report(host: &WindowsHost) {
    println!("\nkeys captured: {}", host.keys);
    println!("pointer reports captured: {}", host.pointers);
    // Not "what a dry run would have sent": the source converts nothing, so that
    // number is the two above added up and says nothing they do not.
    println!("messages sent to the sink: {}", host.sent);

    // Only the pointer's, because only the pointer is refused one event at a time. What
    // refuses the keys is the registration that also delivers them, so "refused but not
    // captured" is not a state the keyboard can be in any more — which it was, and it
    // was a keyboard that had stopped
    // (docs/platform/windows/hooks-and-raw-input.md).
    let refused = host.refused();
    if refused > 0 {
        println!("\nrefused by the mouse hook: {refused} pointer events");
        if host.pointers == 0 {
            println!(
                "  nothing arrived as raw input while they were being refused, so those \
                 movements reached neither machine"
            );
        }
    }

    if !host.unknown.is_empty() {
        println!("\nmake codes seen that no key is named for:");
        for unknown in &host.unknown {
            println!(
                "  device {} {}{:#04x}",
                unknown.device.0,
                match unknown.extended {
                    true => "e0 ",
                    false => "",
                },
                unknown.code
            );
        }
        println!("  while suppressing, these are keys that do nothing at all");
    }

    if !host.absolute.is_empty() {
        println!("\npointers that report where they are rather than how far they moved:");
        for device in &host.absolute {
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

const ON_THEIR_OWN: [&str; 3] = ["--identity", "--devices", "--ansi"];

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
