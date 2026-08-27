//! ADR-0006's prohibitions, held against both hosts' own source text.
//!
//! Source text and not behaviour, because there is nothing else left to read: a
//! decision inside a host is one nothing can drive, which is the whole reason the
//! boundary exists, so a test that ran the code could not reach the thing being
//! asserted about. It sits in the suite rather than beside it so that a passing
//! `cargo test` is a claim about the boundary and not only about what a run does.
//!
//! It reads `host-windows` as well, which is the half this machine has no C
//! toolchain to compile — so these rules are held over code no compiler here ever
//! sees.
//!
//! What it does not read is `bin-watchdog`'s two platform halves, which are host
//! halves living beside a `main` rather than in a host crate (ADR-0006). They are
//! the one place these rules apply and nothing applies them.
//!
//! Each rule carries its exceptions, keyed `<crate>/<file>::<name>` rather than by
//! line, since a line moves whenever anything above it does. **An entry is a
//! violation on record and not a permission**: what is on these lists is what
//! remains of ADR-0006's migration, written to be counted and shrunk. An
//! exception matching nothing fails as loudly as a violation matching no
//! exception, because a list carrying entries for code that has moved on is one
//! nobody can read the remaining work off.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use syn::visit::Visit;

/// The crates whose whole content is calls into a platform.
const HOSTS: [&str; 2] = ["host-macos", "host-windows"];

/// Declarations rather than operations, so the rules do not apply to it.
///
/// Reading the platform's own names out of it, rather than from a list written
/// beside it: a list would go stale silently, and a stale one makes the
/// call-counting rule pass by failing to recognise the calls.
const DECLARATIONS: &str = "ffi.rs";

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("this crate sits under the workspace's crates directory")
        .to_path_buf()
}

/// Every `.rs` file under a crate's `src`, however deep, as `("<file>", parsed)`.
///
/// Named by the path below `src` rather than the file alone, since two modules in
/// different directories can share a basename and an exception has to name one.
fn source_of(crate_name: &str) -> Vec<(String, syn::File)> {
    fn walk(dir: &std::path::Path, prefix: &str, found: &mut Vec<(String, syn::File)>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path
                .file_name()
                .expect("read_dir yields named entries")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                walk(&path, &format!("{prefix}{name}/"), found);
                continue;
            }
            if path.extension().is_some_and(|kind| kind == "rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
                let parsed = syn::parse_file(&text)
                    .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
                found.push((format!("{prefix}{name}"), parsed));
            }
        }
    }
    let mut found = Vec::new();
    walk(&crates_dir().join(crate_name).join("src"), "", &mut found);
    assert!(!found.is_empty(), "no source was found in {crate_name}");
    found
}

#[test]
fn the_host_crate_holds_no_code() {
    // ADR-0006: the traits and the types their signatures are written in are the
    // whole content of this crate — definitions, and no code at all. A body here
    // would be a decision in the one crate both `engine` and every host can see,
    // which is the one place a host could reach one without depending on
    // `engine`.
    //
    // A trait method's default body counts. It is the shape that reads most like
    // a convenience — every host would inherit it and none would have written it
    // — and inheriting a decision is how a host comes to hold one it did not.
    let mut found = Vec::new();
    for (file, parsed) in source_of("host") {
        walk_items(&parsed.items, &mut |item| match item {
            syn::Item::Fn(function) => found.push(format!("host/{file}::{}", function.sig.ident)),
            syn::Item::Impl(block) => {
                for member in &block.items {
                    if let syn::ImplItem::Fn(function) = member {
                        found.push(format!("host/{file}::{}", function.sig.ident));
                    }
                }
            }
            syn::Item::Trait(declared) => {
                for member in &declared.items {
                    if let syn::TraitItem::Fn(function) = member {
                        if function.default.is_some() {
                            found.push(format!("host/{file}::{}", function.sig.ident));
                        }
                    }
                }
            }
            _ => {}
        });
    }
    hold(
        "the `host` crate holds no code (ADR-0006: definitions, and no code at all)",
        found,
        HOST_CRATE_CODE,
    );
}

/// Every body in the `host` crate, and why it is there.
///
/// Empty, and meant to stay so: what a `derive` generates is not written here and
/// does not appear.
const HOST_CRATE_CODE: &[(&str, &str)] = &[];

#[test]
fn engines_public_functions_are_the_ways_in() {
    // What `engine` offers outward is the modes a run can be started in, one per
    // thing a binary or the suite does. Anything else public is a step of a run
    // that something outside is performing for itself — which puts the order over
    // those steps in a binary, where the suite cannot drive it, and that is
    // ADR-0006's harm arrived at from the other side of the boundary.
    //
    // Free functions and not methods: a constructor or an accessor on a public
    // type is the type's surface, and the vocabulary is `engine`'s to publish
    // (ADR-0005). What this counts is the verbs.
    let mut found = Vec::new();
    for (file, parsed) in source_of("engine") {
        walk_items(&parsed.items, &mut |item| {
            if let syn::Item::Fn(function) = item {
                if matches!(function.vis, syn::Visibility::Public(_)) {
                    found.push(format!("engine/{file}::{}", function.sig.ident));
                }
            }
        });
    }
    hold(
        "`engine`'s public functions are the ways into a run (ADR-0006)",
        found,
        WAYS_IN,
    );
}

/// Every public function in `engine`, and what starts it.
const WAYS_IN: &[(&str, &str)] = &[
    (
        "engine/sink.rs::run",
        "the converting run (`favjit --dry-run false`)",
    ),
    (
        "engine/sink.rs::watch",
        "the reading run (`favjit --usages`)",
    ),
    (
        "engine/sink.rs::replay",
        "a recorded run, played back (ADR-0009)",
    ),
    (
        "engine/source.rs::replay",
        "a recorded relaying run, read back as the messages it sent (ADR-0009)",
    ),
    (
        "engine/source.rs::answer_a_procedure",
        "the platform calling a procedure a run installed, which is a way in the OS takes \
         rather than a person",
    ),
    (
        "engine/source.rs::run",
        "the relaying run on the forwarding machine",
    ),
    (
        "engine/watchdog.rs::run",
        "the supervisor's own run (ADR-0008)",
    ),
    (
        "engine/pairing.rs::pair",
        "`favjit --pair` on the machine showing the code",
    ),
    (
        "engine/pairing.rs::pair_with",
        "`favjit --pair <digits>` on the machine given it",
    ),
    (
        "engine/pairing.rs::identity",
        "`favjit --identity`, and what every run opens with",
    ),
    (
        "engine/pointer.rs::tune",
        "`favjit --pointers`, and what a converting run does once",
    ),
    (
        "engine/control.rs::disable",
        "the off switch (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`)",
    ),
    (
        "engine/capture.rs::convert",
        "the loop that reads the converting machine's keyboards, which its host starts a thread \
         for and turns",
    ),
    (
        "engine/capture.rs::relay",
        "the loop that reads the forwarding machine's, likewise",
    ),
    (
        "engine/control.rs::console_home",
        "which of the two homes a process was given the off switch is looked for under, which \
         every program that reads it has to get the same answer to",
    ),
    (
        "engine/control.rs::path",
        "where under that home it sits, likewise",
    ),
    (
        "engine/discovery.rs::find",
        "looking for the other machine (ADR-0012)",
    ),
    (
        "engine/source.rs::identify",
        "`favjit --identity`, on the forwarding machine",
    ),
    (
        "engine/source.rs::describe_devices",
        "`favjit --devices`, on the forwarding machine",
    ),
    (
        "engine/source.rs::what_is_attached",
        "the enumeration `favjit --devices` reads, which is the count before the list and the \
         room before each path",
    ),
];

/// Every `.rs` file in a host, as `("<crate>/<file>", parsed)`.
///
/// Flat rather than recursive, and it refuses a directory instead of walking one:
/// a module tree that grew a subdirectory would otherwise be checked in part,
/// which is a rule quietly covering less rather than a rule failing.
fn host_files() -> Vec<(String, syn::File)> {
    let mut found = Vec::new();
    for host in HOSTS {
        let dir = crates_dir().join(host).join("src");
        let entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
            .flatten()
            .map(|entry| entry.path())
            .collect();
        let nested: Vec<&PathBuf> = entries.iter().filter(|path| path.is_dir()).collect();
        assert!(
            nested.is_empty(),
            "{} holds a directory ({nested:?}); these rules read one level and would cover \
             everything under it",
            dir.display()
        );
        let mut paths: Vec<PathBuf> = entries
            .into_iter()
            .filter(|path| path.extension().is_some_and(|kind| kind == "rs"))
            .collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let parsed = syn::parse_file(&text)
                .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
            let file = path
                .file_name()
                .expect("read_dir yields named files")
                .to_string_lossy()
                .into_owned();
            found.push((format!("{host}/{file}"), parsed));
        }
    }
    assert!(!found.is_empty(), "no host source was found to check");
    found
}

/// Report every finding no exception covers, and every exception nothing matches.
fn hold(rule: &str, found: Vec<String>, exceptions: &[(&str, &str)]) {
    let allowed: BTreeSet<&str> = exceptions.iter().map(|(what, _)| *what).collect();
    assert_eq!(
        allowed.len(),
        exceptions.len(),
        "{rule}: an exception is listed twice, so one of the two reasons is not being read"
    );
    let found: BTreeSet<String> = found.into_iter().collect();

    let unlisted: Vec<&str> = found
        .iter()
        .map(String::as_str)
        .filter(|what| !allowed.contains(what))
        .collect();
    let stale: Vec<&str> = allowed
        .iter()
        .copied()
        .filter(|what| !found.contains(*what))
        .collect();

    let listing = |what: &[&str]| {
        what.iter()
            .map(|one| format!("  {one}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        unlisted.is_empty(),
        "{rule}\n\nnot on the list ({}):\n{}\n\nADR-0006: a host decides nothing. Move it to \
         `engine`, or add it to this rule's exceptions with the reason it is one.",
        unlisted.len(),
        listing(&unlisted),
    );
    assert!(
        stale.is_empty(),
        "{rule}\n\nlisted and no longer there ({}):\n{}\n\nDelete the entries: what is left on \
         this list is the work that remains, so one matching nothing hides how much that is.",
        stale.len(),
        listing(&stale),
    );
}

/// The name of the function a finding is in, innermost first.
///
/// Tracked while walking rather than asked of a span, because `syn` spans carry
/// no parent — the visitor is what knows where it is.
#[derive(Default)]
struct Where(Vec<String>);

impl Where {
    fn here(&self, file: &str) -> String {
        match self.0.last() {
            Some(name) => format!("{file}::{name}"),
            None => format!("{file}::<top level>"),
        }
    }

    /// The whole of where it is, `impl` type included.
    ///
    /// Kept apart from [`Where::here`] because the rules that count calls key on
    /// the name alone, and moving them all onto this would rewrite every entry
    /// on their lists for nothing they are about.
    fn all_of_here(&self, file: &str) -> String {
        match self.0.is_empty() {
            true => format!("{file}::<top level>"),
            false => format!("{file}::{}", self.0.join("::")),
        }
    }
}

/// The name of the type an `impl` block is over.
///
/// The last segment of its path, which is what a reader of one of these files
/// would call it; a type this cannot read a name off is `<impl>`, since leaving
/// it out would put the method under the file alone.
fn named_type(of: &syn::Type) -> String {
    match of {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(|| String::from("<impl>"), |last| last.ident.to_string()),
        _ => String::from("<impl>"),
    }
}

/// The macros that put words somewhere, which is a choice of words.
const SAYS_SOMETHING: [&str; 10] = [
    "log", "trace", "debug", "info", "warn", "error", "print", "println", "eprint", "eprintln",
];

/// Names standing for one call into this machine, beyond the `extern` ones.
///
/// Matched by name rather than by resolved type, so this errs towards reporting:
/// a `read` that turns out to be over a slice becomes an exception carrying that
/// as its reason, and resolving types without a compiler is not available here.
const REACHES_THE_MACHINE: [&str; 32] = [
    "read_to_string",
    "read_exact",
    "write_all",
    "create_dir_all",
    "remove_file",
    "set_len",
    "sync_all",
    "spawn",
    "sleep",
    "getrandom",
    "connect",
    "bind",
    "accept",
    "local_addr",
    "peer_addr",
    "shutdown",
    "try_clone",
    "set_read_timeout",
    "set_write_timeout",
    "set_nonblocking",
    "set_nodelay",
    "recv_from",
    "send_to",
    "join_multicast_v4",
    "set_multicast_loop_v4",
    "as_raw_fd",
    "into_raw_fd",
    "from_raw_fd",
    "kill",
    "wait",
    "status",
    "output",
];

/// Where words are written.
struct Says<'a> {
    file: &'a str,
    at: Where,
    found: Vec<String>,
}

impl<'ast> Visit<'ast> for Says<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.at.0.push(node.sig.ident.to_string());
        syn::visit::visit_item_fn(self, node);
        self.at.0.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.at.0.push(node.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, node);
        self.at.0.pop();
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let segment = |at: usize| {
            node.path
                .segments
                .iter()
                .nth(at)
                .map(|segment| segment.ident.to_string())
                .unwrap_or_default()
        };
        // Either half, because both `log::warn!` and a `use log::warn` reach the
        // same macro and only one of them is written as a path.
        let first = segment(0);
        let last = node
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        if SAYS_SOMETHING.contains(&first.as_str()) || SAYS_SOMETHING.contains(&last.as_str()) {
            self.found.push(self.at.here(self.file));
        }
        syn::visit::visit_macro(self, node);
    }
}

#[test]
fn a_host_writes_no_words_of_its_own() {
    // `Host::warn` is the operation for saying something: `engine` composes the
    // line and the host writes it where this machine's log goes. A host that
    // formats its own message has decided what a fact means and handed over only
    // the typing — and a message is also where a policy hides, since "once and
    // not per event" is a decision with no platform call in it at all.
    //
    // What holds this in the end is that neither host depends on `log`, which is
    // cargo refusing rather than a check reporting. This still earns its place
    // once that lands: `std`'s own `println!` compiles with the dependency gone.
    let mut found = Vec::new();
    for (file, parsed) in host_files() {
        let mut says = Says {
            file: &file,
            at: Where::default(),
            found: Vec::new(),
        };
        says.visit_file(&parsed);
        found.extend(says.found);
    }
    hold(
        "a host writes no words of its own (ADR-0006: what is said)",
        found,
        MESSAGES,
    );
}

/// Every function in a host that says something, and why it still does.
///
/// One entry per function however many lines it writes: what has to move is that
/// function's judgement about what its fact means, and it moves once.
const MESSAGES: &[(&str, &str)] = &[
    // The operation itself. `engine` composed the line; this writes it where this
    // machine's log goes, which is the one call it makes.
    ("host-macos/lib.rs::warn", KEEPS_THE_LOG),
    ("host-windows/lib.rs::warn", KEEPS_THE_LOG),
    // The same operation on the boundary the link is served through, which is
    // turned on a thread with no `Host` of its own to say anything through.
    ("host-macos/link.rs::warn", KEEPS_THE_LOG),
    // The same shape, on stdout instead of the log: `PairingHost::show` takes
    // the whole line, so what is said and the order of the lines are the
    // pairing's, and this is the call that puts one where a person reads it.
    ("host-macos/pairing.rs::show", KEEPS_THE_LOG),
];

const KEEPS_THE_LOG: &str =
    "`Host::warn` itself: the line arrives composed and this is the call that writes it";

#[test]
fn a_host_states_no_bound_of_its_own() {
    // A poll interval, a retry limit, a heartbeat cadence: each is a bound on a
    // machine's behaviour that the machine did not state, so a run given a
    // different one behaves differently in a way no test can show. The platform's
    // own vocabulary is not this — a struct's size, a protocol version, the byte
    // naming a request — because the machine would reject any other value, which
    // is the test ADR-0006 states.
    let mut found = Vec::new();
    for (file, parsed) in host_files() {
        if file.ends_with(DECLARATIONS) {
            continue;
        }
        found.extend(constants_in(&file, &parsed));
    }
    hold(
        "a host states no bound of its own (ADR-0006: a constant that is favjit's own judgement)",
        found,
        CONSTANTS,
    );
}

/// Every constant a host states, and what makes it the platform's own.
const CONSTANTS: &[(&str, &str)] = &[
    // The machine's own values, held outside `ffi.rs` only because they are
    // `libc`'s rather than declared here.
    (
        "host-windows/region.rs::FILE_MAP_ALL_ACCESS",
        THE_MACHINES_VALUE,
    ),
    ("host-macos/region.rs::MAP_FAILED", THE_MACHINES_VALUE),
    ("host-macos/region.rs::MAP_SHARED", THE_MACHINES_VALUE),
    ("host-macos/region.rs::PROT_READ", THE_MACHINES_VALUE),
    ("host-macos/region.rs::PROT_WRITE", THE_MACHINES_VALUE),
    ("host-macos/supervisor.rs::F_SETFL", THE_MACHINES_VALUE),
    ("host-macos/supervisor.rs::O_NONBLOCK", THE_MACHINES_VALUE),
    ("host-windows/mdns.rs::GROUP", THE_MACHINES_VALUE),
    // The virtual HID service's protocol: every number it would reject another
    // value of, from its own headers (`docs/platform/macos/virtual-hid-device.md`).
    (
        "host-macos/vhid.rs::CLIENT_PROTOCOL_VERSION",
        THE_SERVICES_PROTOCOL,
    ),
    ("host-macos/vhid.rs::COUNTRY_CODE_US", THE_SERVICES_PROTOCOL),
    (
        "host-macos/vhid.rs::HEALTH_CHECK_RESPONSE",
        THE_SERVICES_PROTOCOL,
    ),
    ("host-macos/vhid.rs::HEARTBEAT", THE_SERVICES_PROTOCOL),
    (
        "host-macos/vhid.rs::KEYBOARD_REPORT_ID",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::KEYBOARD_REPORT_LEN",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::POINTING_REPORT_LEN",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::POST_APPLE_VENDOR_KEYBOARD_INPUT_REPORT",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::POST_APPLE_VENDOR_TOP_CASE_INPUT_REPORT",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::POST_CONSUMER_INPUT_REPORT",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::POST_GENERIC_DESKTOP_INPUT_REPORT",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::POST_KEYBOARD_INPUT_REPORT",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::POST_POINTING_INPUT_REPORT",
        THE_SERVICES_PROTOCOL,
    ),
    ("host-macos/vhid.rs::REQUEST", THE_SERVICES_PROTOCOL),
    ("host-macos/vhid.rs::RESPONSE", THE_SERVICES_PROTOCOL),
    ("host-macos/vhid.rs::SOCKET", THE_SERVICES_PROTOCOL),
    (
        "host-macos/vhid.rs::VIRTUAL_HID_KEYBOARD_INITIALIZE",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::VIRTUAL_HID_POINTING_INITIALIZE",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::VIRTUAL_KEYBOARD_PRODUCT",
        THE_SERVICES_PROTOCOL,
    ),
    (
        "host-macos/vhid.rs::VIRTUAL_KEYBOARD_VENDOR",
        THE_SERVICES_PROTOCOL,
    ),
    // How deep a queue over one keyboard is: bounded memory and nothing else,
    // sized past any burst a person can type rather than to a number that has
    // to be argued for. Karabiner uses the same figure.
    (
        "host-macos/capture.rs::QUEUE_DEPTH",
        "how many values a queue holds before it drops them, which is memory and not a bound \
         on any behaviour",
    ),
    // State a callback or a hook procedure reads, which Windows and Core
    // Foundation both call with the event and nothing else — so there is nowhere
    // to hang a context and the static is the context. Not a bound on anything.
    ("host-macos/capture.rs::CAPTURE", NOWHERE_TO_HANG_IT),
    ("host-windows/suppress.rs::BACK_HERE", NOWHERE_TO_HANG_IT),
    ("host-windows/suppress.rs::HELD_HERE", NOWHERE_TO_HANG_IT),
    ("host-windows/suppress.rs::KEYS_GO_TO", NOWHERE_TO_HANG_IT),
    ("host-windows/suppress.rs::POINTERS", NOWHERE_TO_HANG_IT),
    ("host-windows/suppress.rs::REFUSING", NOWHERE_TO_HANG_IT),
    ("host-windows/suppress.rs::THE_RUN", NOWHERE_TO_HANG_IT),
    ("host-windows/suppress.rs::TO_THE_SINK", NOWHERE_TO_HANG_IT),
    // How a value is spelled into what an atomic holds, which is the conversion
    // in rather than a judgement about the machine.
    ("host-windows/suppress.rs::EVERYTHING", PACKED_FOR_AN_ATOMIC),
    ("host-windows/suppress.rs::NOTHING", PACKED_FOR_AN_ATOMIC),
    (
        "host-windows/suppress.rs::NO_POSITION",
        PACKED_FOR_AN_ATOMIC,
    ),
    ("host-windows/suppress.rs::THE_SWITCH", PACKED_FOR_AN_ATOMIC),
];

const THE_MACHINES_VALUE: &str = "the platform's own value, which `libc` names and the machine \
                                  would reject any other of";
const THE_SERVICES_PROTOCOL: &str =
    "the virtual HID service's own protocol number, which it would reject any other of";
const NOWHERE_TO_HANG_IT: &str = "the context a bare callback has nowhere else to keep, not a \
                                  bound the machine would have taken differently";
const PACKED_FOR_AN_ATOMIC: &str =
    "how a value is spelled into what an atomic holds, which is the conversion in";

#[test]
fn a_hosts_own_tests_are_the_inventory_of_what_is_in_it() {
    // ADR-0006: a unit test inside a host says the line has moved, because
    // something a unit test can drive is something that decides. The one
    // exception is a test exercising the IO itself, so each entry below has to be
    // that or it is logic to move.
    let mut found = Vec::new();
    for (file, parsed) in host_files() {
        found.extend(tests_in(&file, &parsed));
    }
    hold(
        "a host's own tests are the inventory of what is in it (ADR-0006)",
        found,
        TESTS,
    );
}

/// Every test inside a host, and the IO it drives.
const TESTS: &[(&str, &str)] = &[
    (
        "host-macos/control.rs::converting_is_the_absence_of_the_file",
        "a file whose absence is the answer, which is the exception ADR-0006 names by name",
    ),
    (
        "host-macos/link.rs::a_source_gets_in_over_a_socket_and_what_it_wrote_arrives",
        "a real connection carrying bytes through the accept, the answer and the record, in \
         the order a run makes those calls",
    ),
    (
        "host-macos/vhid.rs::a_bound_that_comes_round_with_nothing_is_told_from_bytes",
        "a quiet socket, which is what a cadence is built on",
    ),
    (
        "host-macos/vhid.rs::a_read_answers_with_what_is_there_rather_than_with_what_it_asked_for",
        "a stream socket answering with less than was wanted, which is its own right and what \
         the framing above it is built on",
    ),
    (
        "host-macos/vhid.rs::what_arrived_is_what_lands_in_the_buffer",
        "a real read of a real socket into the buffer a run handed it",
    ),
    (
        "host-macos/vhid.rs::a_far_end_that_has_closed_answers_with_no_bytes_at_all",
        "a socket whose peer is gone, which nothing but a socket does",
    ),
    (
        "host-windows/link.rs::a_sink_that_is_not_listening_is_reported_rather_than_raised",
        "a connection refused by a port nothing is on",
    ),
    (
        "host-windows/link.rs::this_end_and_a_listening_sink_agree_on_what_was_written",
        "a real connection carrying bytes through the first message, the answer and the record, \
         and the record going out on the socket the handshake was made on",
    ),
    (
        "host-windows/ffi.rs::the_structures_are_the_size_the_headers_say",
        "the declarations against the headers, which is the one thing `ffi.rs` can be wrong about",
    ),
    (
        "host-windows/ffi.rs::a_wide_string_is_terminated",
        "the conversion a Win32 call's argument takes, which no other machine's suite reaches",
    ),
];

/// What each host's `ffi.rs` declares, which is the platform's own vocabulary.
fn platform_names() -> BTreeSet<String> {
    // Every host file and not only the one named for declarations: a file that
    // declares what it calls beside its own calls is the ordinary case for a
    // small surface, and reading one file would leave those calls invisible to
    // every rule below — which is a host being measured on the part of itself it
    // happens to have put in one place.
    let mut names = BTreeSet::new();
    for (_, parsed) in host_files() {
        walk_items(&parsed.items, &mut |item| {
            if let syn::Item::ForeignMod(block) = item {
                for declared in &block.items {
                    if let syn::ForeignItem::Fn(function) = declared {
                        names.insert(function.sig.ident.to_string());
                    }
                }
            }
        });
    }
    assert!(
        names.contains("IOHIDDeviceOpen")
            && names.contains("SetWindowsHookExW")
            && names.contains("mmap")
            && names.contains("MapViewOfFile"),
        "every host file's declarations have to be read for the rules below to recognise a \
         call, the ones declared beside their own calls included; found {} names",
        names.len()
    );
    names
}

/// Whether a name a call site spells is the platform's own.
fn declared_by_the_platform(platform: &BTreeSet<String>, name: &str) -> bool {
    (platform.contains(name) || REACHES_THE_MACHINE.contains(&name))
        && !ASKS_THE_MACHINE_NOTHING.contains(&name)
}

/// Only the keys belonging to the same host as this file.
fn only_this_host(keys: Vec<String>, file: &str) -> Vec<String> {
    let host = file.split('/').next().unwrap_or_default();
    keys.into_iter()
        .filter(|key| key.starts_with(&format!("{host}/")))
        .collect()
}

/// Which of a host's own functions a call site could mean.
///
/// Nearest first and the first answer taken, since nothing here reads a
/// receiver's type: a name spelled with its type means that one, a bare name
/// inside an `impl` means that type's own, and only a name matching nothing
/// nearer is looked for in the other files — where several answer, all of them
/// do, which counts a call into the machine that only one of them makes.
///
/// Keyed the way a finding is keyed, so that what this resolves to is a name a
/// reader can look up on the lists rather than a bare name several functions
/// share.
#[allow(clippy::too_many_arguments)]
fn meant(
    functions: &BTreeSet<String>,
    file: &str,
    within: Option<&str>,
    typed: Option<&str>,
    name: &str,
    how: How,
    on: Option<&str>,
    // The function being walked, which a bare method name inside it must not
    // resolve to: a `Turning::next_device` whose body reaches a `Capture`'s of
    // the same name would otherwise resolve to itself, and a function that only
    // reaches the machine through itself never reaches it at all.
    not: &str,
) -> Vec<String> {
    // A receiver whose type the source states is the answer, and an empty answer
    // where that type is not a host's: nothing else can tell a `Supervisor`'s
    // `beat` from a channel's `send`, and a bare name would find whichever file
    // happens to declare one.
    if let (How::Method, Some(on)) = (how, on) {
        return meant_on(functions, file, on, name, not);
    }
    let one = |key: String| (functions.contains(&key) && key != not).then_some(key);
    let every = |tail: String, shaped: fn(&str) -> bool| -> Vec<String> {
        functions
            .iter()
            .filter(|key| key.ends_with(&tail) && shaped(key) && *key != not)
            .cloned()
            .collect()
    };
    let free = |key: &str| key.split("::").count() == 2;
    let of_a_type = |key: &str| key.split("::").count() > 2;
    // This file first and then the rest of this host, and never the other one:
    // the two are separate crates, so a name in one cannot mean a function in the
    // other — and reading a `next` here as the other machine's would put a call
    // into a body that makes none.
    let here = |keys: Vec<String>| -> Vec<String> {
        let mine: Vec<String> = keys
            .iter()
            .filter(|key| key.starts_with(&format!("{file}::")))
            .cloned()
            .collect();
        match mine.is_empty() {
            true => only_this_host(keys, file),
            false => mine,
        }
    };
    // A name spelled with what holds it is that one or there is none: `CString::new`
    // is `std`'s, and letting it fall through to every `new` a host declares would
    // read the machine into a call that never reaches it. Which of the two it is
    // spelled with is the case of the segment — `crate::vhid::open` names the file
    // and `CfString::new` names the type, and reading a module as a type would
    // leave every call across a host's own files unresolved.
    if let Some(typed) = typed {
        let a_module = typed.starts_with(|first: char| first.is_lowercase());
        if a_module {
            let host = file.split('/').next().unwrap_or_default();
            return one(format!("{host}/{typed}.rs::{name}"))
                .map(|key| vec![key])
                .unwrap_or_default();
        }
        return one(format!("{file}::{typed}::{name}"))
            .map(|key| vec![key])
            .unwrap_or_else(|| every(format!("::{typed}::{name}"), of_a_type));
    }
    match how {
        // A receiver's own, whatever type it is: no free function can be called
        // through one, and letting a method fall through to them makes every
        // `.map()` in a host the mapping call that some file happens to declare.
        // This type's own, then this file's, and no further: a receiver whose
        // type the source does not state is one nothing can resolve, and reading
        // `OpenOptions::open` as the injector's would put a call to the output
        // device in a body that opens a file.
        How::Method => within
            .and_then(|within| one(format!("{file}::{within}::{name}")))
            .map(|key| vec![key])
            .unwrap_or_else(|| {
                every(format!("::{name}"), of_a_type)
                    .into_iter()
                    .filter(|key| key.starts_with(&format!("{file}::")))
                    .collect()
            }),
        How::Plain => one(format!("{file}::{name}"))
            .or_else(|| within.and_then(|within| one(format!("{file}::{within}::{name}"))))
            .map(|key| vec![key])
            .unwrap_or_else(|| here(every(format!("::{name}"), free))),
    }
}

/// How a call site spells the name it calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    /// `name(…)`, which is a free function or one this type declares.
    Plain,
    /// `receiver.name(…)`, which is a method and cannot be a free function.
    Method,
}

/// The methods that hand back what they were called on, for reading a receiver
/// through them.
///
/// A field declared `Option<Injector>` is reached as `self.injector.as_ref()`,
/// and what the method after that runs is `Injector`'s: stopping at the wrapper
/// would leave every field a host holds behind an `Option` unresolved.
const THE_SAME_VALUE: [&str; 8] = [
    "as_ref",
    "as_mut",
    "as_deref",
    "unwrap",
    "expect",
    "clone",
    "borrow",
    "borrow_mut",
];

/// The name of the type a receiver has, where the source says what it is.
fn typed_receiver(
    fields: &BTreeMap<String, BTreeMap<String, String>>,
    locals: &BTreeMap<String, String>,
    file: &str,
    within: Option<&str>,
    receiver: &syn::Expr,
) -> Option<String> {
    match receiver {
        syn::Expr::Field(field) => {
            let named = match &field.member {
                syn::Member::Named(named) => named.to_string(),
                syn::Member::Unnamed(_) => return None,
            };
            let of_self =
                matches!(&*field.base, syn::Expr::Path(path) if path.path.is_ident("self"));
            let within = within?;
            of_self
                .then(|| fields.get(&format!("{file}::{within}")))
                .flatten()
                .and_then(|fields| fields.get(&named))
                .cloned()
        }
        syn::Expr::Path(path) => path
            .path
            .get_ident()
            .and_then(|named| locals.get(&named.to_string()))
            .cloned(),
        syn::Expr::MethodCall(call) => THE_SAME_VALUE
            .contains(&call.method.to_string().as_str())
            .then(|| typed_receiver(fields, locals, file, within, &call.receiver))
            .flatten(),
        syn::Expr::Reference(reference) => {
            typed_receiver(fields, locals, file, within, &reference.expr)
        }
        syn::Expr::Paren(paren) => typed_receiver(fields, locals, file, within, &paren.expr),
        syn::Expr::Group(group) => typed_receiver(fields, locals, file, within, &group.expr),
        syn::Expr::Unary(unary) => typed_receiver(fields, locals, file, within, &unary.expr),
        syn::Expr::Try(attempt) => typed_receiver(fields, locals, file, within, &attempt.expr),
        _ => None,
    }
}

/// Which function a method on a receiver of this type is, where it is a host's.
///
/// An empty answer for a type this host did not declare, which is what makes a
/// `Receiver<Captured>` different from a `Supervisor`: the first reaches nothing
/// in a host, and reading its `recv` as some other file's would be a call into
/// the machine that is not there.
fn meant_on(
    functions: &BTreeSet<String>,
    file: &str,
    typed: &str,
    name: &str,
    not: &str,
) -> Vec<String> {
    // This host's own and nothing else: the two are separate crates, so a method
    // named here cannot be one the other machine's host declares.
    let tail = format!("::{typed}::{name}");
    let found: Vec<String> = functions
        .iter()
        .filter(|key| key.ends_with(&tail) && *key != not)
        .cloned()
        .collect();
    only_this_host(found, file)
}

/// Every name a function binds, which is a name that is not a function's there.
///
/// Whole functions and not blocks: a binding shadowing a function of the same
/// name is what has to be recognised, and where in the body it does so changes
/// nothing about which of the two a bare name means. A `read` bound by a `let` is
/// the reason this exists at all — without it the name resolves to a function
/// that reaches the machine, and reading a local counts as calling it.
#[derive(Default)]
struct Bound(BTreeSet<String>);

impl<'ast> syn::visit::Visit<'ast> for Bound {
    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        self.0.insert(node.ident.to_string());
        syn::visit::visit_pat_ident(self, node);
    }
}

/// The last two segments of a path, as the type a name is spelled with.
fn typed_by(path: &syn::Path) -> Option<&syn::Ident> {
    let mut segments = path.segments.iter().rev();
    segments.next();
    segments.next().map(|segment| &segment.ident)
}

/// What each host struct's fields are typed as, by the type's own name.
///
/// The last segment of the type and its first type argument's, so that a field
/// declared `Option<Injector>` answers `Injector`: what a method on it reaches is
/// the same either way, and a receiver read through `as_ref` or `unwrap` spells
/// neither wrapper.
fn field_types() -> BTreeMap<String, BTreeMap<String, String>> {
    let mut found: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (file, parsed) in host_files() {
        if file.ends_with(DECLARATIONS) {
            continue;
        }
        walk_items(&parsed.items, &mut |item| {
            if let syn::Item::Struct(declared) = item {
                let named = declared.ident.to_string();
                let fields = found.entry(format!("{file}::{named}")).or_default();
                for field in &declared.fields {
                    if let Some(name) = &field.ident {
                        fields.insert(name.to_string(), innermost(&field.ty));
                    }
                }
            }
        });
    }
    found
}

/// The types that stand in front of the one a method is called on.
///
/// Only these are looked past. Reading through every type argument would take
/// `Option<Sender<Request>>` for a `Request`, and what a `send` on it runs is the
/// sender's.
const HOLDS_ANOTHER: [&str; 8] = [
    "Option", "Box", "Arc", "Rc", "RefCell", "Cell", "Mutex", "RwLock",
];

/// The name a type is reached through, past whatever holds it.
fn innermost(of: &syn::Type) -> String {
    match of {
        syn::Type::Path(path) => path.path.segments.last().map_or_else(String::new, |last| {
            let named = last.ident.to_string();
            let holds = HOLDS_ANOTHER.contains(&named.as_str());
            let inner = match &last.arguments {
                syn::PathArguments::AngleBracketed(arguments) => {
                    arguments.args.iter().find_map(|argument| match argument {
                        syn::GenericArgument::Type(inner) => Some(innermost(inner)),
                        _ => None,
                    })
                }
                _ => None,
            };
            holds.then_some(inner).flatten().unwrap_or(named)
        }),
        syn::Type::Reference(reference) => innermost(&reference.elem),
        syn::Type::Paren(paren) => innermost(&paren.elem),
        _ => String::new(),
    }
}

/// Every function in a host, keyed the way a finding is.
fn host_functions() -> BTreeSet<String> {
    struct Named<'a> {
        file: &'a str,
        at: Where,
        found: BTreeSet<String>,
    }

    impl<'ast> syn::visit::Visit<'ast> for Named<'_> {
        fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
            self.at.0.push(named_type(&node.self_ty));
            syn::visit::visit_item_impl(self, node);
            self.at.0.pop();
        }

        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            self.at.0.push(node.sig.ident.to_string());
            self.found.insert(self.at.all_of_here(self.file));
            syn::visit::visit_item_fn(self, node);
            self.at.0.pop();
        }

        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            self.at.0.push(node.sig.ident.to_string());
            self.found.insert(self.at.all_of_here(self.file));
            syn::visit::visit_impl_item_fn(self, node);
            self.at.0.pop();
        }
    }

    let mut found = BTreeSet::new();
    for (file, parsed) in host_files() {
        if file.ends_with(DECLARATIONS) {
            continue;
        }
        let mut named = Named {
            file: &file,
            at: Where::default(),
            found: BTreeSet::new(),
        };
        named.visit_file(&parsed);
        found.extend(named.found);
    }
    found
}

/// Every host function whose call reaches the machine, through however many of
/// the host's own functions.
///
/// A call to a host's own wrapper is a call into the machine. Counting only the
/// declared names would make "one call per body" say nothing: giving each call a
/// one-line function of its own leaves a body that sequences eight of them
/// looking like a body that calls nothing, which is the whole of what this rule
/// is for.
fn reaches_the_machine(
    platform: &BTreeSet<String>,
    functions: &BTreeSet<String>,
    fields: &BTreeMap<String, BTreeMap<String, String>>,
) -> BTreeSet<String> {
    let mut calls: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut directly: BTreeSet<String> = BTreeSet::new();
    for (file, parsed) in host_files() {
        if file.ends_with(DECLARATIONS) {
            continue;
        }
        let mut walking = Calls {
            file: &file,
            functions,
            platform,
            fields,
            at: Where::default(),
            within: Vec::new(),
            bound: Vec::new(),
            locals: Vec::new(),
            handed_to_the_platform: 0,
            calls: &mut calls,
            directly: &mut directly,
        };
        walking.visit_file(&parsed);
    }

    let mut found = directly;
    loop {
        let before = found.len();
        for (key, called) in &calls {
            if called.iter().any(|one| found.contains(one)) {
                found.insert(key.clone());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Which of a host's own functions each of them calls, and which call out.
struct Calls<'a> {
    file: &'a str,
    functions: &'a BTreeSet<String>,
    platform: &'a BTreeSet<String>,
    fields: &'a BTreeMap<String, BTreeMap<String, String>>,
    at: Where,
    within: Vec<String>,
    bound: Vec<BTreeSet<String>>,
    locals: Vec<BTreeMap<String, String>>,
    /// How deep inside the arguments of a call into the platform this is.
    handed_to_the_platform: usize,
    calls: &'a mut BTreeMap<String, BTreeSet<String>>,
    directly: &'a mut BTreeSet<String>,
}

impl Calls<'_> {
    /// A name a body spells, whether it is called there or handed on to be.
    ///
    /// Handed on counts as called: a function's name given to `map` is a
    /// function the body runs, and a body that reached the machine that way
    /// would be one this could not see reaching it at all.
    fn spelled(&mut self, typed: Option<&str>, name: &str, how: How, on: Option<&str>) {
        let here = self.at.all_of_here(self.file);
        if declared_by_the_platform(self.platform, name) {
            self.directly.insert(here);
            return;
        }
        let within = self.within.last().map(String::as_str);
        let meant = meant(
            self.functions,
            self.file,
            within,
            typed,
            name,
            how,
            on,
            &here,
        );
        self.calls.entry(here).or_default().extend(meant);
    }

    /// Whether a bare name is one this function bound rather than a function's.
    fn is_local(&self, typed: Option<&str>, name: &str) -> bool {
        typed.is_none() && self.bound.last().is_some_and(|bound| bound.contains(name))
    }

    /// The type of a receiver, read off this function's own bindings.
    fn typed_receiver(&self, receiver: &syn::Expr) -> Option<String> {
        typed_receiver(
            self.fields,
            self.locals.last()?,
            self.file,
            self.within.last().map(String::as_str),
            receiver,
        )
    }

    /// Note what a closure's one parameter is handed, so a method on it resolves.
    fn handed_on(&mut self, node: &syn::ExprMethodCall) {
        if !HANDS_IT_ON.contains(&node.method.to_string().as_str()) {
            return;
        }
        let Some(typed) = self.typed_receiver(&node.receiver) else {
            return;
        };
        for named in node.args.iter().filter_map(one_parameter) {
            if let Some(locals) = self.locals.last_mut() {
                locals.insert(named, typed.clone());
            }
        }
    }
}

/// The methods that hand what they were called on to a closure.
const HANDS_IT_ON: [&str; 12] = [
    "map",
    "map_or",
    "map_or_else",
    "and_then",
    "is_some_and",
    "is_none_or",
    "filter",
    "inspect",
    "for_each",
    "unwrap_or_else",
    "ok_or_else",
    "take_if",
];

/// The name a closure's one parameter binds, where it binds exactly one.
fn one_parameter(argument: &syn::Expr) -> Option<String> {
    let syn::Expr::Closure(closure) = argument else {
        return None;
    };
    match closure.inputs.iter().collect::<Vec<_>>().as_slice() {
        [syn::Pat::Ident(named)] => Some(named.ident.to_string()),
        _ => None,
    }
}

impl<'ast> syn::visit::Visit<'ast> for Calls<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let tested = node
            .attrs
            .iter()
            .any(|attribute| attribute.path().is_ident("cfg") && cfg_is_test(attribute));
        if tested {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let named = named_type(&node.self_ty);
        self.at.0.push(named.clone());
        self.within.push(named);
        syn::visit::visit_item_impl(self, node);
        self.within.pop();
        self.at.0.pop();
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let mut bound = Bound::default();
        bound.visit_item_fn(node);
        self.at.0.push(node.sig.ident.to_string());
        self.bound.push(bound.0);
        self.locals.push(BTreeMap::new());
        syn::visit::visit_item_fn(self, node);
        self.locals.pop();
        self.bound.pop();
        self.at.0.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let mut bound = Bound::default();
        bound.visit_impl_item_fn(node);
        self.at.0.push(node.sig.ident.to_string());
        self.bound.push(bound.0);
        self.locals.push(BTreeMap::new());
        syn::visit::visit_impl_item_fn(self, node);
        self.locals.pop();
        self.bound.pop();
        self.at.0.pop();
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let (syn::Pat::Ident(named), Some(init)) = (&node.pat, &node.init) {
            if let Some(typed) = self.typed_receiver(&init.expr) {
                if let Some(locals) = self.locals.last_mut() {
                    locals.insert(named.ident.to_string(), typed);
                }
            }
        }
        syn::visit::visit_local(self, node);
    }

    /// The arguments and not the name being called, which [`Calls::spelled`]
    /// already has: walking the whole of it would reach the name again as a
    /// value handed on and count one call as two.
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        let mut the_platforms = false;
        if let syn::Expr::Path(path) = &*node.func {
            if let Some(last) = path.path.segments.last() {
                let name = last.ident.to_string();
                let typed = typed_by(&path.path).map(|typed| typed.to_string());
                the_platforms = declared_by_the_platform(self.platform, &name);
                self.spelled(typed.as_deref(), &name, How::Plain, None);
            }
        }
        self.handed_to_the_platform += usize::from(the_platforms);
        for argument in &node.args {
            self.visit_expr(argument);
        }
        self.handed_to_the_platform -= usize::from(the_platforms);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let name = node.method.to_string();
        let on = self.typed_receiver(&node.receiver);
        self.spelled(None, &name, How::Method, on.as_deref());
        self.handed_on(node);
        let the_platforms = declared_by_the_platform(self.platform, &name);
        self.handed_to_the_platform += usize::from(the_platforms);
        syn::visit::visit_expr_method_call(self, node);
        self.handed_to_the_platform -= usize::from(the_platforms);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(last) = node.path.segments.last() {
            let typed = typed_by(&node.path).map(|typed| typed.to_string());
            let name = last.ident.to_string();
            let mine = !self.is_local(typed.as_deref(), &name);
            // Not one of the platform's own used as a value, for
            // [`Shaped::visit_expr_path`]'s reason: a procedure it is given is
            // one it calls later rather than a call this body makes.
            let the_platforms = declared_by_the_platform(self.platform, &name);
            if mine && !the_platforms && self.handed_to_the_platform == 0 {
                self.spelled(typed.as_deref(), &name, How::Plain, None);
            }
        }
        syn::visit::visit_expr_path(self, node);
    }
}

/// The leaf an expression hangs off, `None` where it is not one name.
fn root_of(subject: &syn::Expr) -> Option<String> {
    match subject {
        syn::Expr::Path(path) => match path.path.segments.len() {
            // A single segment is a binding; `Suppressing::Everything` and
            // anything else with a `::` in it is a name from elsewhere.
            1 => Some(path.path.segments[0].ident.to_string()),
            _ => None,
        },
        syn::Expr::Unary(unary) => root_of(&unary.expr),
        syn::Expr::Reference(reference) => root_of(&reference.expr),
        syn::Expr::Paren(paren) => root_of(&paren.expr),
        syn::Expr::Group(group) => root_of(&group.expr),
        syn::Expr::Field(field) => root_of(&field.base),
        syn::Expr::MethodCall(call) => root_of(&call.receiver),
        syn::Expr::Try(attempt) => root_of(&attempt.expr),
        // A comparison branches on both sides, and either being handed in is the
        // same finding, so the left one standing for the pair is enough.
        syn::Expr::Binary(binary) => root_of(&binary.left).or_else(|| root_of(&binary.right)),
        _ => None,
    }
}

/// The methods that mean a value is being added to something rather than read.
///
/// By name, because there is no type to resolve against here: what this is after
/// is a collection on `self` growing, and every one of these is how that is
/// spelled. A host reaching for a container method not on this list keeps
/// something the rule below does not catch.
const KEEPS_IT: [&str; 6] = ["push", "insert", "extend", "push_back", "append", "replace"];

/// Where a host makes what a call answered into a fact of its own.
///
/// The three shapes ADR-0006 names, each written into something hanging off
/// `self`: a flag set to a literal, a counter stepped on, a list added to. What
/// is deliberately not here is a handle being held — `self.socket = connect()`
/// and its kind — because holding what a call returned, and reaching back
/// through it for the next call, is the shape of a host and not a decision. The
/// two are told apart by the value written and not by the field, since there is
/// no type to resolve against here: a literal and an increment carry no handle,
/// and a list that grows was not answered in one piece.
///
/// So a host that keeps a fact by assigning it out of a call — a code, a length —
/// is not caught. That shape is what the call-counting rule sees, since the fact
/// has to be produced by a call in the same body to be assigned there.
///
/// A closure counts into the function that writes it, as in [`Calls`]: a `move`
/// closure handed to a thread writes into what it captured, and that is the same
/// keeping.
struct Keeps<'a> {
    file: &'a str,
    at: Where,
    declared: &'a BTreeSet<String>,
    fields: &'a BTreeMap<String, BTreeMap<String, String>>,
    found: Vec<String>,
}

impl Keeps<'_> {
    fn kept(&mut self, target: &syn::Expr) {
        if root_of(target).as_deref() == Some("self") {
            self.found.push(self.at.here(self.file));
        }
    }

    /// Whether what is being kept is a handle the platform answered with.
    ///
    /// ADR-0006 keeps platform handles out of `engine`, so something in a host
    /// holds them and the next call reaches back through them: what this rule is
    /// after is a *fact* turned into a counter or a flag, which is a fact the run
    /// was not handed. A type the declarations file names is neither, and a
    /// struct one of whose fields is such a type is the handle with the run's own
    /// number for it beside it.
    ///
    /// Read off the type rather than excused by name, so that what a host keeps
    /// is a handle because its declarations say so — and a fact smuggled into one
    /// of those structs would have to be declared beside the platform's own
    /// names to get past this.
    fn a_handle(&self, value: &syn::Expr) -> bool {
        let of_the_platform = |named: &str| self.declared.contains(named);
        match value {
            syn::Expr::Struct(built) => built
                .path
                .segments
                .last()
                .and_then(|last| self.fields.get(&format!("{}::{}", self.file, last.ident)))
                .is_some_and(|fields| fields.values().any(|typed| of_the_platform(typed))),
            syn::Expr::Path(path) => path
                .path
                .segments
                .last()
                .is_some_and(|last| of_the_platform(&last.ident.to_string())),
            syn::Expr::Reference(reference) => self.a_handle(&reference.expr),
            syn::Expr::Paren(paren) => self.a_handle(&paren.expr),
            _ => false,
        }
    }
}

/// What each host's declarations file names, which is the platform's own
/// vocabulary of types.
fn platform_types() -> BTreeMap<String, BTreeSet<String>> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (file, parsed) in host_files() {
        if !file.ends_with(DECLARATIONS) {
            continue;
        }
        let host = file.split('/').next().unwrap_or_default().to_string();
        let named = found.entry(host).or_default();
        walk_items(&parsed.items, &mut |item| match item {
            syn::Item::Type(declared) => {
                named.insert(declared.ident.to_string());
            }
            syn::Item::Struct(declared) => {
                named.insert(declared.ident.to_string());
            }
            syn::Item::Enum(declared) => {
                named.insert(declared.ident.to_string());
            }
            _ => {}
        });
    }
    found
}

/// Whether an expression carries no answer from a call — a flag's own value.
fn a_bare_flag(value: &syn::Expr) -> bool {
    match value {
        syn::Expr::Lit(literal) => matches!(literal.lit, syn::Lit::Bool(_) | syn::Lit::Int(_)),
        syn::Expr::Unary(unary) => a_bare_flag(&unary.expr),
        syn::Expr::Paren(paren) => a_bare_flag(&paren.expr),
        syn::Expr::Group(group) => a_bare_flag(&group.expr),
        _ => false,
    }
}

impl<'ast> Visit<'ast> for Keeps<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.at.0.push(node.sig.ident.to_string());
        syn::visit::visit_item_fn(self, node);
        self.at.0.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.at.0.push(node.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, node);
        self.at.0.pop();
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if a_bare_flag(&node.right) {
            self.kept(&node.left);
        }
        syn::visit::visit_expr_assign(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        // The compound assignments, which `syn` gives as a binary operator
        // rather than as an assignment of its own.
        if matches!(
            node.op,
            syn::BinOp::AddAssign(_)
                | syn::BinOp::SubAssign(_)
                | syn::BinOp::MulAssign(_)
                | syn::BinOp::DivAssign(_)
                | syn::BinOp::RemAssign(_)
                | syn::BinOp::BitXorAssign(_)
                | syn::BinOp::BitAndAssign(_)
                | syn::BinOp::BitOrAssign(_)
                | syn::BinOp::ShlAssign(_)
                | syn::BinOp::ShrAssign(_)
        ) {
            self.kept(&node.left);
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if KEEPS_IT.contains(&node.method.to_string().as_str()) {
            let a_handle = node.args.iter().any(|value| self.a_handle(value));
            if !a_handle {
                self.kept(&node.receiver);
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

#[test]
fn a_host_keeps_nothing_a_call_answered() {
    // ADR-0006's three-part body has no room for a fourth thing, and state is
    // the fourth thing a count cannot see: a fact turned into a counter or a
    // flag is a fact the run was not handed, and asking for it again is the run
    // reading what the host decided to remember rather than what happened.
    let types = platform_types();
    let fields = field_types();
    let mut found = Vec::new();
    for (file, parsed) in host_files() {
        if file.ends_with(DECLARATIONS) {
            continue;
        }
        let host = file.split('/').next().unwrap_or_default().to_string();
        let nothing = BTreeSet::new();
        let mut keeps = Keeps {
            file: &file,
            at: Where::default(),
            declared: types.get(&host).unwrap_or(&nothing),
            fields: &fields,
            found: Vec::new(),
        };
        keeps.visit_file(&parsed);
        found.extend(keeps.found);
    }
    hold(
        "a host keeps nothing a call answered (ADR-0006)",
        found,
        KEPT,
    );
}

/// Every function writing to its own state, and why it still does.
const KEPT: &[(&str, &str)] = &[];

/// Every `const` and `static` in the file, wherever it is nested.
fn constants_in(file: &str, parsed: &syn::File) -> Vec<String> {
    let mut found = Vec::new();
    walk_items(&parsed.items, &mut |item| match item {
        syn::Item::Const(declared) => found.push(format!("{file}::{}", declared.ident)),
        syn::Item::Static(declared) => found.push(format!("{file}::{}", declared.ident)),
        _ => {}
    });
    found
}

/// Every `#[test]` in the file, wherever it is nested.
fn tests_in(file: &str, parsed: &syn::File) -> Vec<String> {
    let mut found = Vec::new();
    walk_items(&parsed.items, &mut |item| {
        if let syn::Item::Fn(function) = item {
            let tested = function
                .attrs
                .iter()
                .any(|attribute| attribute.path().is_ident("test"));
            if tested {
                found.push(format!("{file}::{}", function.sig.ident));
            }
        }
    });
    found
}

/// The two shapes a function in a host is allowed to have.
///
/// One reaches the machine: it makes one call and takes no turnings at all, so
/// the whole of it is the argument going out and the answer coming back. The
/// other turns a value into another value and reaches nothing, which is where
/// every turning a host needs is allowed to be — and it is registered, so what a
/// person checks is a list of things that claim to be conversions rather than a
/// list of functions being let off.
///
/// That is the difference from an exception. An entry here says what a function
/// *is*, and a wrong entry is a description a reader can see is wrong: a
/// conversion into the platform's own spelling reads as one, and a choice favjit
/// made does not. An entry excusing a function that reaches the machine cannot
/// be written at all, because the two shapes are checked against each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// More than one call into the machine from one body.
    TwoCallsIn,
    /// A turning taken in a body that reaches the machine.
    ATurningBesideACall,
    /// A turning taken by a function that is not registered as a conversion.
    ATurningNobodyRegistered,
    /// A registered conversion that reaches the machine.
    AConversionThatCalls,
}

impl Shape {
    fn said(self) -> &'static str {
        match self {
            Self::TwoCallsIn => "two calls into the machine in one body",
            Self::ATurningBesideACall => {
                "a turning taken beside the call: lift it into a conversion and register that"
            }
            Self::ATurningNobodyRegistered => {
                "a turning taken by a function no conversion list names"
            }
            Self::AConversionThatCalls => "registered as a conversion, but it reaches the machine",
        }
    }
}

/// The methods whose call is itself a choice, whatever they are handed.
///
/// Named rather than left to the closures most of them carry, because a closure
/// is not what makes them one: `a.zip(b)` and `a.unwrap_or(b)` decide which value
/// comes out with both arms already evaluated, and `iterators.flatten().find_map(
/// next_service)` decides which of several a value comes from with a function's
/// name where a closure would be. What is common to all of them is the receiver
/// being asked whether it has anything, so that is what is counted.
const CHOOSES_BETWEEN_ARMS: [&str; 28] = [
    "zip",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "then",
    "then_some",
    "ok_or",
    "ok_or_else",
    "or",
    "or_else",
    "and",
    "and_then",
    "xor",
    "get_or_insert",
    "get_or_insert_with",
    "map",
    "map_or",
    "map_or_else",
    "filter",
    "filter_map",
    "flatten",
    "find",
    "find_map",
    "position",
    "any",
    "all",
    "is_some_and",
    "is_none_or",
];

/// Whether a `cfg` attribute is the one naming a test build.
fn cfg_is_test(attribute: &syn::Attribute) -> bool {
    attribute
        .parse_args::<syn::Path>()
        .is_ok_and(|path| path.is_ident("test"))
}

/// What a host's functions do, by the two shapes above.
struct Shaped<'a> {
    file: &'a str,
    at: Where,
    within: Vec<String>,
    bound: Vec<BTreeSet<String>>,
    locals: Vec<BTreeMap<String, String>>,
    /// How deep inside the arguments of a call into the platform this is.
    handed_to_the_platform: usize,
    platform: &'a BTreeSet<String>,
    functions: &'a BTreeSet<String>,
    fields: &'a BTreeMap<String, BTreeMap<String, String>>,
    reaching: &'a BTreeSet<String>,
    /// One frame per function walked: calls into the machine, and turnings.
    counted: Vec<(usize, usize)>,
    found: Vec<String>,
}

impl Shaped<'_> {
    fn enter(&mut self, name: String, bound: BTreeSet<String>) {
        self.at.0.push(name);
        self.bound.push(bound);
        self.locals.push(BTreeMap::new());
        self.counted.push((0, 0));
    }

    /// The type of a receiver, read off this function's own bindings.
    fn typed_receiver(&self, receiver: &syn::Expr) -> Option<String> {
        typed_receiver(
            self.fields,
            self.locals.last()?,
            self.file,
            self.within.last().map(String::as_str),
            receiver,
        )
    }

    /// Note what a closure's one parameter is handed, so a method on it resolves.
    fn handed_on(&mut self, node: &syn::ExprMethodCall) {
        if !HANDS_IT_ON.contains(&node.method.to_string().as_str()) {
            return;
        }
        let Some(typed) = self.typed_receiver(&node.receiver) else {
            return;
        };
        for named in node.args.iter().filter_map(one_parameter) {
            if let Some(locals) = self.locals.last_mut() {
                locals.insert(named, typed.clone());
            }
        }
    }

    fn leave(&mut self) {
        let here = self.at.all_of_here(self.file);
        self.bound.pop();
        self.locals.pop();
        let Some((calls, turnings)) = self.counted.pop() else {
            return;
        };
        let registered = CONVERSIONS.iter().any(|(what, _)| *what == here);
        let shape = match (calls, turnings, registered) {
            (2.., _, _) => Some(Shape::TwoCallsIn),
            (1.., 1.., _) => Some(Shape::ATurningBesideACall),
            (1.., _, true) => Some(Shape::AConversionThatCalls),
            (0, 1.., false) => Some(Shape::ATurningNobodyRegistered),
            _ => None,
        };
        if let Some(shape) = shape {
            // The name alone is what the lists are keyed on, so that an entry
            // names a function rather than a function and the way it currently
            // fails. Which way that is goes to the run's output instead, since a
            // list entry that had to be rewritten whenever the shape changed
            // would be one nobody keeps.
            println!("{here} — {}", shape.said());
            self.found.push(here);
        }
        self.at.0.pop();
    }

    /// A name this body spells, counted where what it names reaches the machine.
    ///
    /// The host's own functions count with the platform's: a wrapper is not a
    /// boundary, so a body sequencing several of them is sequencing the calls
    /// inside them. Not the conversions among the platform's own, since a
    /// `CFTypeRef` cannot be read without asking Core Foundation what it is and
    /// none of those asks the machine anything.
    fn called(&mut self, typed: Option<&str>, name: &str, how: How, on: Option<&str>) {
        let here = self.at.all_of_here(self.file);
        let reaches = declared_by_the_platform(self.platform, name)
            || meant(
                self.functions,
                self.file,
                self.within.last().map(String::as_str),
                typed,
                name,
                how,
                on,
                &here,
            )
            .iter()
            .any(|key| self.reaching.contains(key));
        if reaches {
            if let Some((calls, _)) = self.counted.last_mut() {
                *calls += 1;
            }
        }
    }

    fn turned(&mut self) {
        if let Some((_, turnings)) = self.counted.last_mut() {
            *turnings += 1;
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for Shaped<'_> {
    /// A host's own tests are not walked here.
    ///
    /// What may be under test in a host is the one thing this rule cannot help
    /// with, since a test that stands up both ends of some IO is a sequence by
    /// nature — and [`TESTS`] already names every test in a host with the IO it
    /// drives, which is the rule that governs them. Walking them here would put
    /// the same functions on two lists.
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let tested = node
            .attrs
            .iter()
            .any(|attribute| attribute.path().is_ident("cfg") && cfg_is_test(attribute));
        if tested {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let mut bound = Bound::default();
        bound.visit_item_fn(node);
        self.enter(node.sig.ident.to_string(), bound.0);
        syn::visit::visit_item_fn(self, node);
        self.leave();
    }

    /// The type an `impl` block is over is part of where a function is.
    ///
    /// Without it a file holding two functions of one name — a free helper and a
    /// method that wraps it — puts both under the same key, and a list entry
    /// naming that key says nothing about which of them it means.
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let named = named_type(&node.self_ty);
        self.at.0.push(named.clone());
        self.within.push(named);
        syn::visit::visit_item_impl(self, node);
        self.within.pop();
        self.at.0.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let mut bound = Bound::default();
        bound.visit_impl_item_fn(node);
        self.enter(node.sig.ident.to_string(), bound.0);
        syn::visit::visit_impl_item_fn(self, node);
        self.leave();
    }

    /// The arguments and not the name being called, which [`Shaped::called`]
    /// already has: walking the whole of it would reach the name again as a
    /// value handed on and count one call as two.
    ///
    /// A name handed to the platform is not a call either: a procedure given to
    /// `IOHIDDeviceRegisterRemovalCallback` is one the platform calls later, on
    /// a thread of its own, and counting it here would make registering a
    /// callback the same as making the calls inside it.
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        let mut the_platforms = false;
        if let syn::Expr::Path(path) = &*node.func {
            if let Some(name) = path.path.segments.last() {
                let name = name.ident.to_string();
                let typed = typed_by(&path.path).map(|typed| typed.to_string());
                the_platforms = declared_by_the_platform(self.platform, &name);
                self.called(typed.as_deref(), &name, How::Plain, None);
            }
        }
        self.handed_to_the_platform += usize::from(the_platforms);
        for argument in &node.args {
            self.visit_expr(argument);
        }
        self.handed_to_the_platform -= usize::from(the_platforms);
    }

    /// A function's name handed on to be called, which is a call the body makes.
    ///
    /// Not one of the platform's own, wherever it appears as a value: a
    /// procedure the platform is given — as a window class's, as a callback —
    /// is one it calls later, on a thread of its own, and counting it here would
    /// make handing one over the same as making the calls inside it.
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(name) = node.path.segments.last() {
            let typed = typed_by(&node.path).map(|typed| typed.to_string());
            let name = name.ident.to_string();
            let local =
                typed.is_none() && self.bound.last().is_some_and(|bound| bound.contains(&name));
            let the_platforms = declared_by_the_platform(self.platform, &name);
            if !local && !the_platforms && self.handed_to_the_platform == 0 {
                self.called(typed.as_deref(), &name, How::Plain, None);
            }
        }
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let name = node.method.to_string();
        let on = self.typed_receiver(&node.receiver);
        self.called(None, &name, How::Method, on.as_deref());
        self.handed_on(node);
        if CHOOSES_BETWEEN_ARMS.contains(&name.as_str()) {
            self.turned();
        }
        // What a thread is started on is spelled as a method, so the arguments
        // of one that reaches the platform are handed over the same way a plain
        // call's are.
        let the_platforms = declared_by_the_platform(self.platform, &name);
        self.handed_to_the_platform += usize::from(the_platforms);
        syn::visit::visit_expr_method_call(self, node);
        self.handed_to_the_platform -= usize::from(the_platforms);
    }

    /// A closure is a turning: the arm is its body and the condition is the
    /// method it is handed to.
    ///
    /// Without this the rule reads a style rather than a boundary — `if x { a }`
    /// is a turning and `x.then(|| a)` is the same turning spelled to get past a
    /// check, and it is the second spelling a body reaches for once the first is
    /// the one being counted.
    ///
    /// Not one handed to the platform, which is what that call runs rather than a
    /// choice between arms: the body a thread is started on has no condition in
    /// front of it, and counting it would make starting a thread a turning.
    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        if self.handed_to_the_platform == 0 {
            self.turned();
        }
        syn::visit::visit_expr_closure(self, node);
    }

    /// `matches!`, which is a `match` the visitor is handed as a macro.
    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        if node.mac.path.is_ident("matches") {
            self.turned();
        }
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.turned();
        syn::visit::visit_expr_if(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        self.turned();
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.turned();
        syn::visit::visit_expr_while(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.turned();
        syn::visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.turned();
        syn::visit::visit_expr_loop(self, node);
    }

    /// `?` is a turning: it is the arm that returns instead of carrying on.
    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        self.turned();
        syn::visit::visit_expr_try(self, node);
    }

    /// `let … else`, which `syn` gives as a binding carrying the arm that
    /// diverges rather than as an expression of its own.
    fn visit_local(&mut self, node: &'ast syn::Local) {
        if node
            .init
            .as_ref()
            .is_some_and(|init| init.diverge.is_some())
        {
            self.turned();
        }
        if let (syn::Pat::Ident(named), Some(init)) = (&node.pat, &node.init) {
            if let Some(typed) = self.typed_receiver(&init.expr) {
                if let Some(locals) = self.locals.last_mut() {
                    locals.insert(named.ident.to_string(), typed);
                }
            }
        }
        syn::visit::visit_local(self, node);
    }
}

#[test]
fn a_host_is_calls_and_registered_conversions_and_nothing_else() {
    // ADR-0006's three-part body, checked as the shape of every function rather
    // than as a count of what it happens to do: one call with no turnings beside
    // it, and every turning a host needs lifted into something that turns a value
    // into another value and reaches nothing. What that buys over the counts is
    // where the reading goes — a list of conversions is a list a person can read
    // and disagree with, and a list of functions being let off is not.
    let platform = platform_names();
    let functions = host_functions();
    let fields = field_types();
    let reaching = reaches_the_machine(&platform, &functions, &fields);
    let mut found = Vec::new();
    for (file, parsed) in host_files() {
        if file.ends_with(DECLARATIONS) {
            continue;
        }
        let mut shaped = Shaped {
            file: &file,
            at: Where::default(),
            within: Vec::new(),
            bound: Vec::new(),
            locals: Vec::new(),
            handed_to_the_platform: 0,
            platform: &platform,
            functions: &functions,
            fields: &fields,
            reaching: &reaching,
            counted: Vec::new(),
            found: Vec::new(),
        };
        shaped.visit_file(&parsed);
        found.extend(shaped.found);
    }
    // Every registered conversion names a function that is there, held the same
    // way an exception is: a conversion whose function has moved on is a claim
    // nobody can check, and it is the list a person reads instead of reading the
    // hosts.
    let gone: Vec<&str> = CONVERSIONS
        .iter()
        .map(|(what, _)| *what)
        .filter(|what| !functions.contains(*what))
        .collect();
    assert!(
        gone.is_empty(),
        "registered as conversions and no longer there ({}):\n{}",
        gone.len(),
        gone.iter()
            .map(|one| format!("  {one}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    hold(
        "a host is calls and registered conversions and nothing else (ADR-0006)",
        found,
        EXCEPTIONS,
    );
}

/// The calls that ask the machine nothing.
///
/// Core Foundation's own value operations, which is what a `CFTypeRef` has to be
/// read through, and IOKit's accessors over a value it has already handed over.
/// Asking a value what type it is, taking the number out of it, or reading the
/// page a HID value carries touches no device: what those answer was in the value
/// before the call was made.
///
/// Named here rather than left to the rules to infer, because there is nothing in
/// the shape of a call that says whether it asks the machine anything — and a
/// body doing several of these reads as several questions of the machine when it
/// is one question and the conversions around it.
const ASKS_THE_MACHINE_NOTHING: [&str; 24] = [
    // This process's own module, which a window class belongs to: it answers a
    // property of the calling process and asks nothing about any device, the
    // way `CFRunLoopGetCurrent` below answers a property of the calling thread.
    "GetModuleHandleW",
    // A property off the device object IOKit has already handed over. Observed
    // to answer for every device in a set copied from a manager whose open had
    // failed with `0xe00002c5`, which is what says the properties travel with
    // the object rather than being fetched
    // (`docs/platform/macos/hid-device-enumeration.md`).
    "IOHIDDeviceGetProperty",
    // The scale a mach tick is measured in, which is what turns the clock's own
    // count into nanoseconds: it is fixed for as long as the machine is up and
    // asks nothing about any device, so it is the argument conversion beside the
    // clock read rather than a second question.
    "mach_timebase_info",
    // The run loop of the thread this is called on, which is what every
    // scheduling call takes as its second argument: it answers with a property
    // of the calling thread and asks nothing about any device, the way
    // `IOServiceMatching` below answers with the dictionary a lookup takes.
    "CFRunLoopGetCurrent",
    "CFRelease",
    "CFGetTypeID",
    "CFNumberGetTypeID",
    "CFStringGetTypeID",
    "CFDictionaryGetTypeID",
    "CFNumberGetValue",
    "CFStringGetCString",
    "CFStringCreateWithCString",
    "CFDictionaryGetValue",
    "CFArrayGetCount",
    "CFArrayGetValueAtIndex",
    "CFNumberCreate",
    "IOServiceMatching",
    "IOHIDValueGetElement",
    "IOHIDValueGetIntegerValue",
    "IOHIDValueGetTimeStamp",
    "IOHIDElementGetUsagePage",
    "IOHIDElementGetUsage",
    "IONotificationPortGetRunLoopSource",
    "CFDictionaryCreate",
];

/// Every function that turns a value into another value, and what it turns.
///
/// Reaching the machine is what an entry here cannot do, so this is a list of
/// conversions and not a list of permissions.
const CONVERSIONS: &[(&str, &str)] = &[
    (
        "host-macos/region.rs::landed",
        "the region a mapping landed in, or the machine's own error where it answered \
         `MAP_FAILED`",
    ),
    (
        "host-windows/region.rs::landed",
        "the region a mapping landed in, or the machine's own error where it answered none",
    ),
    (
        "host-macos/region.rs::passed_down",
        "the descriptor a name in the environment holds, where it holds one",
    ),
    (
        "host-windows/region.rs::passed_down",
        "the handle a name in the environment holds, where it holds one",
    ),
    (
        "host-macos/supervisor.rs::named_fd",
        "the descriptor a name in the environment holds, and `-1` where it holds nothing a \
         descriptor could be",
    ),
    (
        "host-windows/supervisor.rs::named_handle",
        "the handle a name in the environment holds, and zero where it holds nothing a handle \
         could be",
    ),
    (
        "host-macos/control.rs::home",
        "this process's own home, out of the environment",
    ),
    (
        "host-macos/control.rs::holding",
        "the directory a file sits in, and the file's own path where it names no directory",
    ),
    (
        "host-macos/cf.rs::there_is_one",
        "a pointer read as a value there is one of, and nothing where it is null",
    ),
    (
        "host-macos/cf.rs::CfString::as_ref",
        "an owned string read as the pointer the calls take, and null where there is none",
    ),
    (
        "host-macos/acceleration.rs::CfNumber::as_ref",
        "an owned number read as the pointer the call takes, and null where there is none",
    ),
    (
        "host-macos/acceleration.rs::viewing",
        "either view of the event system as the one a run holds, and nothing where neither \
         opened",
    ),
    (
        "host-macos/acceleration.rs::a_view",
        "a client pointer read as the view it is, and nothing where it is null",
    ),
    (
        "host-windows/tray.rs::a_run",
        "a window read as the run it stands for, and nothing where there is no window",
    ),
    (
        "host-windows/suppress.rs::what_is_refused",
        "the number a procedure reads, as what the run published it for",
    ),
    (
        "host-windows/suppress.rs::unpacked",
        "one position out of the number an atomic holds, and nothing for the value standing \
         for a key this keyboard has no position for",
    ),
    (
        "host-windows/suppress.rs::a_key",
        "one call into the keyboard's procedure read as the key it carries, and nothing of \
         ours where it carries none",
    ),
    (
        "host-windows/suppress.rs::a_pointer",
        "the same for the pointer's procedure",
    ),
    (
        "host-windows/mdns.rs::opened",
        "the search a bind opened, and nothing where it opened none",
    ),
    (
        "host-windows/mdns.rs::since",
        "how long a clock has been going, in the run's own nanoseconds",
    ),
    (
        "host-windows/mdns.rs::timed_out",
        "an error read as the read timeout coming due rather than a failure",
    ),
    (
        "host-windows/capture.rs::a_message",
        "whether the queue had one, out of the three ways that call answers",
    ),
    (
        "host-windows/capture.rs::a_whole_header",
        "how much a read wrote, and none where it wrote less than a header",
    ),
    (
        "host-windows/capture.rs::what_was_written",
        "how many characters a fill wrote, and none for either way it answered nothing",
    ),
    (
        "host-windows/capture.rs::read_from",
        "one structure out of the front of a buffer the machine filled, and none where it \
         wrote too little to hold one",
    ),
    (
        "host-windows/capture.rs::came_from",
        "the machine's own name for what one arrival came from, and none for the ones that \
         came from no device",
    ),
    (
        "host-windows/capture.rs::note_the_device",
        "the same name put where the run will be offered it, where the run has neither \
         numbered it nor been offered it yet",
    ),
    (
        "host-windows/capture.rs::numbered",
        "the number the run gave the device the machine names this, and none where it has \
         given none yet",
    ),
    (
        "host-windows/capture.rs::the_next",
        "the next thing the turn put down as the event the run reads, and none while it names \
         a device the run has not numbered",
    ),
    (
        "host-windows/capture.rs::as_an_event",
        "what the machine said, under the number the run gave what it came from",
    ),
    (
        "host-windows/mdns.rs::resolve",
        "the address to connect to: the literal one an answer carried, or the name for a \
         resolver to be asked about",
    ),
    (
        "host-windows/mdns.rs::the_first_of",
        "the first address a lookup answered with, and nothing where it answered with none",
    ),
    (
        "host-macos/vhid.rs::reached",
        "the stream a call answered with, or that there is no service to reach",
    ),
    (
        "host-macos/vhid.rs::reaching",
        "the stream a connect answered with as the connection a run holds, or that there is no \
         service to reach",
    ),
    (
        "host-macos/vhid.rs::ends",
        "an open connection as the two ends of it a run holds, and nothing where a second \
         handle on it could not be had",
    ),
    (
        "host-macos/vhid.rs::held",
        "the stream a lock answered with, whether or not a thread died holding it",
    ),
    (
        "host-macos/capture.rs::there_is_one",
        "a pointer read as a value there is one of, and nothing where it is null",
    ),
    (
        "host-macos/capture.rs::with_capture",
        "the number a static holds read as the capture it stands for, and nothing before the \
         thread that set it started — handed over to the caller's own work rather than \
         returned, because a reference that outlived the reading would outlive the thread",
    ),
    (
        "host-macos/capture.rs::there_is_a_service",
        "an entry read as one there is, and nothing for the zero an exhausted iterator answers \
         with",
    ),
    (
        "host-macos/capture.rs::where_it_went",
        "a value read as the answer where the code beside it says the call went",
    ),
    (
        "host-macos/capture.rs::read_value",
        "one HID value read as the four numbers it carries, and nothing where it names no \
         element",
    ),
    (
        "host-macos/capture.rs::next_held",
        "the id of the last keyboard taken, out of the list of the ones this thread holds",
    ),
    (
        "host-macos/capture.rs::opened",
        "what the capture thread said about opening one, and `-1` where it was not there to \
         be asked",
    ),
    (
        "host-macos/capture.rs::reading",
        "the same for reading one, which has no number of the machine's to carry",
    ),
    (
        "host-macos/link.rs::holding",
        "the directory a file sits in, and the file's own path where it names no directory",
    ),
    (
        "host-windows/link.rs::holding",
        "the same on the other machine",
    ),
    (
        "host-macos/link.rs::made",
        "what the machine said about a path it would not touch, named by the path",
    ),
    (
        "host-windows/link.rs::made",
        "the same on the other machine",
    ),
    (
        "host-macos/link.rs::opened",
        "the file an open answered with, or what the machine said about not opening it",
    ),
    (
        "host-windows/link.rs::opened",
        "the same on the other machine",
    ),
    (
        "host-windows/link.rs::local_app_data",
        "the directory this profile keeps what belongs to it in, out of the environment",
    ),
    (
        "host-windows/link.rs::user_profile",
        "the same place spelled another way, for a profile that holds only the second name",
    ),
    (
        "host-windows/link.rs::favjit_directory",
        "where favjit's own files go, under whichever of those two the environment holds",
    ),
    (
        "host-windows/lib.rs::beaten",
        "what the machine said about a beat that did not go, in the words a run reads",
    ),
    ("host-macos/lib.rs::beaten", "the same on the other machine"),
    (
        "host-macos/lib.rs::bound",
        "the listener a bind opened as the link a run serves, and nothing where it opened none",
    ),
    (
        "host-macos/inject.rs::opened",
        "the two ends of an open connection as the loop a run serves and the device it writes \
         to, and nothing where there are no ends",
    ),
    (
        "host-macos/inject.rs::reached",
        "the connection a reach answered with as the one a run holds, and what the machine said \
         about there being no service to reach",
    ),
    (
        "host-windows/link.rs::connected",
        "the socket a connect opened, and nothing where it opened none",
    ),
    (
        "host-macos/pairing.rs::bound",
        "the listener a bind opened, and nothing where it opened none",
    ),
    (
        "host-macos/pairing.rs::arrived",
        "the connection something arrived on, and nothing where nothing did",
    ),
    (
        "host-macos/pairing.rs::the_port",
        "the port an address carries, and nothing where the machine named no address",
    ),
    (
        "host-windows/pairing.rs::reached",
        "the connection a connect opened, and nothing where it opened none",
    ),
    (
        "host-macos/link.rs::connected",
        "what an accept came back with: the connection, one more wait, or the socket",
    ),
    (
        "host-macos/link.rs::whatever_it_holds",
        "the text a file holds, and none where there is no file to read",
    ),
    (
        "host-macos/link.rs::listening",
        "this end over the socket a bind opened, or the machine's own error where it opened none",
    ),
    (
        "host-macos/link.rs::the_port",
        "the port an address carries",
    ),
    (
        "host-macos/link.rs::an_advertisement",
        "the registration a call answered with, or the machine's own error where it answered \
         with none",
    ),
    (
        "host-macos/lib.rs::arrived",
        "one wait on the queue this host's loops report through, read as all four things it can \
         say at once",
    ),
    (
        "host-windows/lib.rs::arrived",
        "the same wait on the other machine, read as everything it can say at once",
    ),
    (
        "host-windows/suppress.rs::there_is_a_window",
        "the window the keys are handed to, and none out of the zero that stands for none",
    ),
    (
        "host-windows/capture.rs::a_count",
        "a count a call answered with, and none where it answered zero",
    ),
    (
        "host-windows/capture.rs::filled_in",
        "how much a call filled in, and none where it answered the failure these APIs spell as \
         every bit set",
    ),
    (
        "host-windows/capture.rs::made",
        "the handle a call answered with, and none where it made nothing",
    ),
    (
        "host-windows/suppress.rs::packed",
        "a keyboard position as the one word `KBDLLHOOKSTRUCT`'s two fields make, and a word no \
         position packs to where there is none",
    ),
    (
        "host-windows/suppress.rs::refusing",
        "what a run asked to be refused, as the number the procedures read it out of",
    ),
    (
        "host-windows/suppress.rs::installed",
        "the hook a call answered with, and none where it installed one",
    ),
    (
        "host-windows/supervisor.rs::how_many",
        "a read's own answer and count read as how many probes arrived",
    ),
    (
        "host-windows/supervisor.rs::went",
        "a write's own answer and count read as whether the whole heartbeat got through",
    ),
    (
        "host-windows/mdns.rs::arrived",
        "one receive read as a datagram, a wait that came round with nothing, or a failure",
    ),
    (
        "host-windows/mdns.rs::looked_up",
        "a service record's host name as the address a resolver gives it, with the trailing dot \
         off",
    ),
    (
        "host-macos/vhid.rs::written",
        "what the machine said about a write, as `0` for the one that went and its own number \
         otherwise",
    ),
    (
        "host-macos/vhid.rs::took",
        "what one read of the socket said, as the count it answered with or as the wait, the \
         interruption or the failure it answered with instead",
    ),
    (
        "host-macos/vhid.rs::request_for",
        "which of the device's own requests posts a report, over every report there is",
    ),
    (
        "host-macos/supervisor.rs::how_many",
        "a read's own count read as how many probes arrived, and none where it failed",
    ),
    (
        "host-macos/supervisor.rs::went",
        "a write's own count read as whether the heartbeat got through",
    ),
    (
        "host-macos/repeat.rs::where_the_systems_went",
        "the iterator a lookup filled in, and nothing where its code says it filled in none",
    ),
    (
        "host-macos/repeat.rs::the_next_system",
        "the entry a step handed over, and nothing for the zero an exhausted iterator answers \
         with",
    ),
    (
        "host-macos/repeat.rs::Parameters::nanos",
        "one of the rates a dictionary holds, as the duration its nanoseconds stand for",
    ),
    (
        "host-macos/repeat.rs::parameters_in",
        "a registry property read as the dictionary it holds, and let go of where it holds \
         something else",
    ),
    (
        "host-macos/repeat.rs::nanos",
        "one entry of that dictionary read as the nanoseconds it holds",
    ),
    (
        "host-macos/control.rs::already_on",
        "a file that was not there read as converting already being on",
    ),
    (
        "host-macos/inject.rs::access_of",
        "the OS's own answer about HID access read as granted, refused, or never asked",
    ),
    (
        "host-macos/capture.rs::seizing",
        "whether the run asked for the keyboard exclusively, as the platform's own option flag",
    ),
    (
        "host-macos/capture.rs::nanos_of",
        "mach ticks as nanoseconds, and zero where the machine named no ratio to convert them \
         with",
    ),
    (
        "host-macos/acceleration.rs::fixed_in",
        "a 16.16 fixed-point property read as the number it stands for, and the value let go of",
    ),
    (
        "host-macos/acceleration.rs::integer_in",
        "a property read as the plain integer it holds, likewise",
    ),
    (
        "host-macos/acceleration.rs::text_in",
        "a property read as the text it holds, cut to the first nul in the buffer",
    ),
    (
        "host-macos/acceleration.rs::services_in",
        "the array the event system handed over read as the services in it, which takes Core \
         Foundation's own calls and asks the machine nothing",
    ),
    (
        "host-windows/link.rs::written",
        "what the machine said about a write, as `0` for the one that went and its own number \
         otherwise",
    ),
    (
        "host-macos/link.rs::registered",
        "the machine's own code read as whether the advertisement went out",
    ),
    (
        "host-macos/link.rs::whether_one_may_still_come",
        "a refused accept as the kind the socket answered with, and nothing about what it means",
    ),
    (
        "host-macos/link.rs::arrived",
        "a read of a fixed-width record read as the whole of one or as the connection going",
    ),
    (
        "host-macos/cf.rs::number_in",
        "the integer a Core Foundation value holds, which is read by asking the value what it \
         is",
    ),
    (
        "host-macos/cf.rs::string_in",
        "the text a Core Foundation value holds, likewise, cut to the first nul in the buffer \
         it was copied into",
    ),
    // The handles each machine hands over, read in and out of the number the
    // run carries them as.
    (
        "host-macos/capture.rs::a_service",
        "a registry entry there is one of as the thing the run holds, and nothing out of the \
         zero an exhausted iterator answers with",
    ),
    (
        "host-macos/capture.rs::a_device",
        "a device there is one of as the thing the run holds, and nothing where the call \
         answered null",
    ),
    (
        "host-macos/capture.rs::the_iterator",
        "the iterator the next finding is looked for on, without the one that answered with \
         nothing, and none once every one of them has",
    ),
    (
        "host-macos/capture.rs::forgotten",
        "the devices waiting without the one under a number, whose reference this file hands \
         back on the way out",
    ),
    (
        "host-macos/capture.rs::a_queue",
        "a queue there is one of as the thing the run holds, and nothing where the call \
         answered null",
    ),
    (
        "host-macos/capture.rs::an_array",
        "an array there is one of as the thing the run holds, likewise",
    ),
    (
        "host-macos/capture.rs::the_front",
        "the first queue a callback said had values, and none once none has",
    ),
    (
        "host-macos/capture.rs::no_longer_ready",
        "the queues with values on them without that one, so IOKit is asked again the next \
         time it has some",
    ),
    (
        "host-macos/capture.rs::the_first_gone",
        "the first device a callback said had gone, and none once none has",
    ),
    (
        "host-macos/capture.rs::named",
        "the number the run gave the device a queue is over",
    ),
    (
        "host-macos/capture.rs::numbered",
        "the number the run gave the device the machine names this",
    ),
    (
        "host-macos/capture.rs::queue_over",
        "the queue over the device the run is holding, and none where nothing is reading it",
    ),
    (
        "host-macos/capture.rs::a_value",
        "one value off a queue read as the numbers it carries, which are all held in the value \
         already, and none once the queue has run dry",
    ),
    (
        "host-macos/capture.rs::at_place",
        "the value a Core Foundation array holds at a place, which was in it before the call \
         was made",
    ),
    (
        "host-macos/capture.rs::how_many",
        "how many values a Core Foundation array holds, likewise",
    ),
    (
        "host-macos/capture.rs::asked_for",
        "what the run asked of the capture thread, under the machine's own name for the device \
         it named",
    ),
    (
        "host-macos/capture.rs::waiting",
        "the device waiting under the number the run gave it",
    ),
    (
        "host-macos/capture.rs::answered",
        "an answer onto the channel whoever asked is waiting on, spelled the way that ask \
         takes it",
    ),
    (
        "host-macos/capture.rs::seized",
        "the devices this run took away from everything else, with the one an open says it \
         took added",
    ),
    (
        "host-macos/capture.rs::watching",
        "the queue kept against the device it is over, and that device off the list of ones \
         waiting to be read",
    ),
    (
        "host-macos/capture.rs::gone_from",
        "every list a device that has gone was on, without it, and the number the run called \
         it by",
    ),
    (
        "host-macos/capture.rs::given_back",
        "the devices this run holds without the one a close says it gave back",
    ),
    (
        "host-macos/capture.rs::Capture::nothing_more_of",
        "the number the run gave a queue's device, as what says that queue has run dry",
    ),
    (
        "host-windows/capture.rs::found",
        "the next device a report named as the one the run is offered, under the number it \
         offered, and nothing where no report has named one",
    ),
    (
        "host-windows/capture.rs::the_window",
        "the window a step made, and a handle to nothing where it made none",
    ),
    (
        "host-windows/capture.rs::a_window_of",
        "the handle a call made as the window that closes it again",
    ),
    (
        "host-windows/capture.rs::the_raw_input",
        "the raw input a message carried, and a handle to nothing for one that carried none",
    ),
    (
        "host-windows/capture.rs::what_else_it_said",
        "what a message said besides a raw input, in this machine's own words",
    ),
    (
        "host-windows/capture.rs::a_pointer_report",
        "one raw input read as the pointer report it carries, bounded by what was written and \
         none where it carries something else",
    ),
    (
        "host-windows/capture.rs::a_removal",
        "a device notification read as one going away, and nothing for one arriving",
    ),
    (
        "host-windows/capture.rs::named_device",
        "whether what arrived came from a device at all",
    ),
    (
        "host-windows/capture.rs::put_down",
        "what a turn produced, onto the list the drain reads, and nothing where it produced \
         none",
    ),
    (
        "host-windows/capture.rs::as_many",
        "a count a call answered with, and none where it answered none",
    ),
    (
        "host-windows/capture.rs::at",
        "the machine's own name for the device at a place, and a handle to nothing where \
         nothing was looked at there",
    ),
    (
        "host-windows/capture.rs::Attached::at",
        "what the machine says the device at a place is, and nothing for a place nothing was \
         looked at in",
    ),
    (
        "host-windows/capture.rs::a_kind",
        "the number this machine lists an input device under, as what the device is",
    ),
    (
        "host-windows/capture.rs::the_text",
        "the path a fill wrote, cut to what it wrote and to the first nul in it",
    ),
    (
        "host-windows/supervisor.rs::the_pipe",
        "the pipe the probes are read off, and no pipe at all where this is not the moment to \
         look",
    ),
];

/// Every function that is neither shape, and what it is instead.
///
/// Empty, and meant to stay so. The two shapes are the whole of what a host may
/// be: one call with no turnings beside it, or a turning that reaches nothing
/// and is registered as a conversion. What a sequence of calls over one thing
/// needs in order to fit — a handle the run holds and hands back
/// ([`favjit_host::Handed`]), or a list of the steps this machine's API takes —
/// is already there, so an entry here would be a shape nobody had looked for
/// one of those for yet.
const EXCEPTIONS: &[(&str, &str)] = &[];

#[test]
fn every_case_in_the_suite_drives_a_run() {
    // ADR-0007: what the suite is worth is that it drives the same code the
    // binaries do, through the same ways in. A case that exercises the simulator
    // or one of its own helpers with no run behind it asserts on the harness, and
    // passes whatever `engine` does — so every `#[test]` in every other file here
    // has to reach one of `WAYS_IN`, itself or through the helpers of its own file.
    //
    // Read off the text rather than measured, for the reason the host rules are:
    // a run that was never started leaves nothing behind that a test could ask.
    let mut found = Vec::new();
    for (file, parsed) in suite_files() {
        let cases = Cases::of(&parsed);
        for case in cases.tests() {
            if !cases.reaches_a_way_in(&case) {
                found.push(format!("{file}::{case}"));
            }
        }
    }
    hold(
        "every case in the suite drives a run (ADR-0007: a test reaches `engine` through one of \
         its ways in, and asserts on nothing else)",
        found,
        HARNESS_ONLY,
    );
}

/// Every case that reaches no way in, and why it is allowed to.
///
/// A case about the simulator alone is not one of them: it belongs in the
/// simulator's own tests.
const HARNESS_ONLY: &[(&str, &str)] = &[(
    "asking_for_nothing.rs::listening_is_only_askable_of_a_run_that_delivers",
    "a record of what `Request` cannot express, held by compiling rather than by running: \
     there is no run to start because the point is that one cannot be asked for",
)];

/// Every test file of this suite but this one, as `("<file>", parsed)`.
///
/// This file is left out because its rules are over source text and start no
/// run, which is what the rule above is about.
fn suite_files() -> Vec<(String, syn::File)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    let this_file = std::path::Path::new(file!())
        .file_name()
        .expect("a source file has a name")
        .to_owned();
    let mut found = Vec::new();
    for path in entries {
        if path.extension().is_none_or(|kind| kind != "rs") || path.file_name() == Some(&this_file)
        {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let parsed = syn::parse_file(&text)
            .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
        found.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            parsed,
        ));
    }
    assert!(!found.is_empty(), "no test file was found beside this one");
    found
}

/// One test file's functions and what each of them calls, with the names its
/// `use` lines give `engine`'s items.
struct Cases {
    /// Every function in the file — free or in an `impl` — by name, with the paths
    /// it calls: a bare method name for a method call, the path as written for a
    /// call by path.
    calls: BTreeMap<String, Vec<Vec<String>>>,
    /// Which of them carry `#[test]`.
    tests: Vec<String>,
    /// A name a `use favjit_engine::…` line brings into the file, and the path
    /// under `favjit_engine` it stands for.
    imported: BTreeMap<String, Vec<String>>,
}

impl Cases {
    fn of(parsed: &syn::File) -> Self {
        let mut cases = Cases {
            calls: BTreeMap::new(),
            tests: Vec::new(),
            imported: BTreeMap::new(),
        };
        walk_items(&parsed.items, &mut |item| match item {
            syn::Item::Fn(function) => {
                let name = function.sig.ident.to_string();
                if function
                    .attrs
                    .iter()
                    .any(|attribute| attribute.path().is_ident("test"))
                {
                    cases.tests.push(name.clone());
                }
                let mut called = Called::default();
                called.visit_block(&function.block);
                cases.calls.entry(name).or_default().extend(called.0);
            }
            syn::Item::Impl(block) => {
                for member in &block.items {
                    if let syn::ImplItem::Fn(function) = member {
                        let mut called = Called::default();
                        called.visit_block(&function.block);
                        cases
                            .calls
                            .entry(function.sig.ident.to_string())
                            .or_default()
                            .extend(called.0);
                    }
                }
            }
            syn::Item::Use(imported) => {
                let mut prefix = Vec::new();
                engine_imports(&imported.tree, &mut prefix, &mut cases.imported);
            }
            _ => {}
        });
        cases
    }

    fn tests(&self) -> Vec<String> {
        self.tests.clone()
    }

    /// Whether this function, or any function of the file it calls, calls one of
    /// `engine`'s ways in.
    fn reaches_a_way_in(&self, function: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut pending = vec![function.to_string()];
        while let Some(name) = pending.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(calls) = self.calls.get(&name) else {
                continue;
            };
            for path in calls {
                if self.is_a_way_in(path) {
                    return true;
                }
                if let [local] = path.as_slice() {
                    if self.calls.contains_key(local) {
                        pending.push(local.clone());
                    }
                }
            }
        }
        false
    }

    /// Whether a path as written names one of `WAYS_IN`, once the file's `use`
    /// lines are read into it.
    fn is_a_way_in(&self, path: &[String]) -> bool {
        let Some((first, rest)) = path.split_first() else {
            return false;
        };
        let full: Vec<String> = if first == "favjit_engine" {
            rest.to_vec()
        } else if let Some(imported) = self.imported.get(first) {
            imported
                .iter()
                .cloned()
                .chain(rest.iter().cloned())
                .collect()
        } else {
            return false;
        };
        let [module, function] = full.as_slice() else {
            return false;
        };
        let key = format!("engine/{module}.rs::{function}");
        WAYS_IN.iter().any(|(way_in, _)| *way_in == key)
    }
}

/// Read one `use` tree, recording every name it brings in from `favjit_engine`.
fn engine_imports(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    into: &mut BTreeMap<String, Vec<String>>,
) {
    let under_engine =
        |prefix: &[String]| prefix.first().is_some_and(|root| root == "favjit_engine");
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            engine_imports(&path.tree, prefix, into);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let ident = name.ident.to_string();
            if ident == "self" {
                if under_engine(prefix) {
                    if let Some(last) = prefix.last() {
                        into.insert(last.clone(), prefix[1..].to_vec());
                    }
                }
            } else if under_engine(prefix) {
                let mut full = prefix[1..].to_vec();
                full.push(ident.clone());
                into.insert(ident, full);
            }
        }
        syn::UseTree::Rename(renamed) => {
            if under_engine(prefix) {
                let mut full = prefix[1..].to_vec();
                full.push(renamed.ident.to_string());
                into.insert(renamed.rename.to_string(), full);
            }
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                engine_imports(item, prefix, into);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

/// Every call a body makes, as the path it was written with.
#[derive(Default)]
struct Called(Vec<Vec<String>>);

impl<'ast> Visit<'ast> for Called {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*call.func {
            self.0.push(
                path.path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect(),
            );
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.0.push(vec![call.method.to_string()]);
        syn::visit::visit_expr_method_call(self, call);
    }

    /// The arguments of a macro, read as the expressions they are. A call inside
    /// `assert_eq!` is where most of the suite's runs are started, and a parse
    /// that stopped at the macro would read every such case as starting none.
    /// What does not parse as expressions — a format string with holes, a token
    /// tree of some other shape — is passed over rather than failed on.
    fn visit_macro(&mut self, invoked: &'ast syn::Macro) {
        type Arguments = syn::punctuated::Punctuated<syn::Expr, syn::Token![,]>;
        if let Ok(arguments) = invoked.parse_body_with(Arguments::parse_terminated) {
            for argument in &arguments {
                self.visit_expr(argument);
            }
        }
    }
}

/// Every item, descending through modules written inline.
fn walk_items(items: &[syn::Item], each: &mut impl FnMut(&syn::Item)) {
    for item in items {
        each(item);
        if let syn::Item::Mod(module) = item {
            if let Some((_, inner)) = &module.content {
                walk_items(inner, each);
            }
        }
    }
}
