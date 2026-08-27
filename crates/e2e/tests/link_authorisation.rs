//! Who gets to send input, and what happens to what they send.
//!
//! ADR-0004 puts the decision on the sink and makes refusal the default. Each of
//! these is a whole run of the converter with the other machine on the other end of
//! its link, so the sequence — ask who is calling, look them up, only then read
//! anything — is checked where it actually happens, and without a platform.
//!
//! What a key actually typed is read off the reports the run wrote, decoded back
//! with `favjit-hid`'s inverse of the encoding: which device the sink resolved a
//! source's own numbering to is `engine`'s bookkeeping and never crosses back out,
//! but a report that never arrived, or one that shows a key released again, does.

use favjit_engine::link::Refused;
use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::{DeviceId, Key, Layout};
use favjit_hid::report::Report;
use favjit_hid::{page, usage};
use favjit_host::{EventKind, OutputReport};
use favjit_host_sim::{identity, SimHost, SimLink};

/// A source that has been paired, and one that has not.
const PAIRED: u8 = 0xaa;
const STRANGER: u8 = 0xbb;

/// What the source calls its keyboard.
const REMOTE: DeviceId = DeviceId(7);

/// The file `favjit --pair` leaves behind, holding that one key.
///
/// Written here rather than built with the simulator's own writer: a script that
/// started from the same function the simulator writes that file with would be
/// checking that function against itself.
fn paired_list() -> String {
    format!("{}\n", hex(identity(PAIRED).public()))
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A run that converts and listens, which is the only run a link belongs to.
fn converting() -> Request {
    Request::Injecting { listen: true }
}

/// A whole run, with this authorised list and this script on the other end.
fn served(authorized: String, script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(authorized);
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );
    mac
}

/// Every report the run wrote, decoded back into the keyboard state it
/// described — nothing here types on a control's page.
fn keyboard_reports(mac: &SimHost) -> Vec<Report> {
    mac.reports()
        .iter()
        .map(|(_, report, bytes)| {
            assert_eq!(*report, OutputReport::Keyboard, "not a control");
            Report::from_bytes(bytes).expect("the bytes of a keyboard report")
        })
        .collect()
}

/// The report a keyboard holding only this key would carry.
fn holding(key: Key) -> Report {
    let (on_page, code) = usage::of(key).expect("a key with a usage");
    assert_eq!(on_page, page::KEYBOARD_OR_KEYPAD, "not a plain key");
    let mut report = Report::default();
    report.press(code);
    report
}

#[test]
fn input_from_a_paired_source_arrives_as_events() {
    let mac = served(paired_list(), |link| {
        link.connect(identity(PAIRED))
            .attach_external(REMOTE, 1, 2)
            .press(REMOTE, Key::K)
            .release(REMOTE, Key::K)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![holding(Key::K), Report::default()],
        "typed alongside whatever this machine's own keyboards would produce"
    );
}

#[test]
fn a_source_nobody_paired_gets_nothing_through() {
    // Refused before a frame is read, not after: input that reached the converter
    // and was then discarded would already have been converted.
    let mac = served(paired_list(), |link| {
        link.connect(identity(STRANGER))
            .press(REMOTE, Key::K)
            .release(REMOTE, Key::K)
            .hang_up();
    });

    // The refusal and nothing else: no keystroke reached the converter, and what
    // did is the record that says why the link went — which is what a person
    // reading this machine's recording afterwards has to go on (ADR-0009).
    assert_eq!(
        mac.delivered(),
        vec![EventKind::LinkEnded {
            why: Refused::NotPaired as u32
        }]
    );
    assert_eq!(mac.refused(), vec![identity(STRANGER).public().to_vec()]);
    assert_eq!(mac.frames_read(), 0);
    assert_eq!(keyboard_reports(&mac), Vec::new());
}

#[test]
fn an_empty_list_refuses_everyone() {
    // The state a machine starts in, and the one a lost list leaves it in.
    let mac = served(String::new(), |link| {
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::K)
            .hang_up();
    });

    // The refusal and nothing else: no keystroke reached the converter, and what
    // did is the record that says why the link went — which is what a person
    // reading this machine's recording afterwards has to go on (ADR-0009).
    assert_eq!(
        mac.delivered(),
        vec![EventKind::LinkEnded {
            why: Refused::NotPaired as u32
        }]
    );
    assert_eq!(mac.refused(), vec![identity(PAIRED).public().to_vec()]);
}

#[test]
fn a_line_that_is_not_a_key_leaves_the_rest_of_the_list_working() {
    // The file is a person's to edit, and a comment, a blank line or half a key
    // pasted in are all things it can end up holding. What must not happen is the
    // whole list failing: that refuses every machine, which from the desk looks
    // exactly like favjit having stopped.
    let list = format!(
        "# the Windows machine\n\nnot a key\nabc\n{}\n",
        hex(identity(PAIRED).public())
    );
    let mac = served(list, |link| {
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::K)
            .hang_up();
    });

    assert!(
        mac.refused().is_empty(),
        "the paired machine is still paired"
    );
    assert!(!mac.delivered().is_empty(), "and its keys still arrive");
}

#[test]
fn pairing_takes_effect_on_the_next_connection() {
    // The list is asked for per connection, so authorising a source does not need
    // the converter restarted — and neither does removing one.
    let mac = served(String::new(), |link| {
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::K)
            .hang_up();
        link.pair(identity(PAIRED).public().to_vec());
        link.connect(identity(PAIRED))
            .attach_external(REMOTE, 1, 2)
            .press(REMOTE, Key::K)
            .hang_up();
    });

    assert_eq!(mac.refused(), vec![identity(PAIRED).public().to_vec()]);
    assert_eq!(
        keyboard_reports(&mac),
        vec![holding(Key::K), Report::default()],
        "only the second connection, once it was authorised, reached the OS"
    );
}

#[test]
fn a_frame_that_cannot_be_read_ends_the_session() {
    // A peer talking a language this end does not is a peer to hang up on: acting
    // on the messages around one that could not be read is acting on a stream
    // whose meaning is already in doubt.
    let mac = served(paired_list(), |link| {
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::K)
            .nonsense()
            .press(REMOTE, Key::J)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![holding(Key::K), Report::default()],
        "k reached the OS and was released when the session ended; j never arrived"
    );
    assert_eq!(mac.link_closed().len(), 1);
}

#[test]
fn one_source_at_a_time_and_the_next_one_is_served() {
    // A link that stopped after the first source would be a keyboard that works
    // until the Windows machine reboots.
    let mac = served(paired_list(), |link| {
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::K)
            .hang_up();
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::J)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![
            holding(Key::K),
            Report::default(),
            holding(Key::J),
            Report::default(),
        ],
        "each session's key released at its own end, and the second one served at all"
    );
}

#[test]
fn a_connection_that_never_became_a_session_does_not_stop_the_link() {
    // A handshake fails for a stranger, a wrong pattern or a truncated read, and
    // none of those is a reason to stop taking connections — a link that gave up
    // would be a keyboard that stops working because somebody port-scanned the
    // machine.
    let mac = served(paired_list(), |link| {
        link.rejected();
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::K)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![holding(Key::K), Report::default()]
    );
}

#[test]
fn input_stops_when_the_converter_has_gone() {
    // The events go to the loop that converts them, and nothing is on the other end
    // of that any more. Reading more from the source would be collecting keystrokes
    // to drop, and the source would have no way to know.
    let mac = served(paired_list(), |link| {
        link.converter_stopped();
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::K)
            .press(REMOTE, Key::J)
            .hang_up();
    });

    assert_eq!(mac.frames_read(), 1);
}
