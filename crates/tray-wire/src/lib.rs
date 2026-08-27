//! What the tray item and a running favjit say to each other (`docs/platform/windows/tray-item-as-its-own-program.md`).
//!
//! Where the window is found, what the state is published as, which message an ask
//! arrives in, and what each number means — stated once here rather than by each of the
//! two programs, for the reason [`favjit_link_wire`]'s frame is stated once rather than by
//! each machine. A copy that disagrees is not an error anywhere: it is a click that does
//! nothing, and an icon drawn from a state nobody published.
//!
//! It is not in either program because both read it, and it is not in a host because
//! every name here is favjit's own rather than the platform's — which is the one thing
//! ADR-0006 does not let a host state.
//!
//! What is not here is the calls that carry it. The item's half — finding the window,
//! reading the property, posting the message — is the item's own, and the run's half is
//! `host-windows`'s: one call per operation, from the thread that decided what to
//! publish.
//!
//! [`favjit_link_wire`]: https://docs.rs/favjit-link-wire

#![no_std]

use favjit_host::source::{Driving, Suppressing};

/// The class of the window a running favjit can be found by.
///
/// The capture window, and not one of its own: favjit has exactly one window, the one
/// raw input is delivered to, and a second would be a second thing to keep alive.
pub const WINDOW_CLASS: &str = "favjit-capture";

/// The property what is being refused is published as.
pub const REFUSING: &str = "favjit-refusing";

/// The message an ask arrives in.
///
/// In the range Windows reserves for an application's own messages, one past the one a
/// key is handed to the capture loop with, so the two cannot be confused and neither
/// collides with anything the system sends.
pub const WM_ASKED: u32 = 0x8001;

/// The message a request to stop arrives in.
///
/// Its own message rather than a third value in [`WM_ASKED`], because it is a different
/// question: where the keyboard is, against whether there is a run at all. What it asks
/// for is the run ending the way a bound reached ends it — the keyboards given back and
/// nothing restarted, since a run that stopped because it was asked to has not failed.
pub const WM_STOP: u32 = 0x8002;

/// What is being refused, as the number a window property carries.
///
/// A hook procedure reads the same state out of an atomic, because Windows calls one with
/// the event and nothing else to hang a context off; the item reads it off the property,
/// because that is what a program outside the run can read without waiting on it. Both
/// are numbers, and stated once here they are the same number — a second numbering is
/// what would let the icon and the refusing disagree.
pub const fn refusing_as(what: Suppressing) -> usize {
    match what {
        // Zero is what a property nobody has written yet reads as, and refusing nothing
        // is what a run that has published nothing is doing.
        Suppressing::Nothing => 0,
        Suppressing::TheSwitch => 1,
        Suppressing::Everything => 2,
        // `Suppressing` is `#[non_exhaustive]`: a state this channel cannot yet say is
        // said as the one that sends nobody to the wrong machine.
        _ => 0,
    }
}

/// What a number on that property names, or `None` for one nothing here wrote.
pub const fn from_refusing(number: usize) -> Option<Suppressing> {
    match number {
        0 => Some(Suppressing::Nothing),
        1 => Some(Suppressing::TheSwitch),
        2 => Some(Suppressing::Everything),
        _ => None,
    }
}

/// Which machine the keyboard is asked to drive, as the number a message carries.
///
/// Numbered apart from the states above because they are two different questions — where
/// the keyboard is has three answers and where it is asked to go has two — and they
/// travel by different means, a property against a message. Neither is zero, so a message
/// posted with nothing in it names no machine.
pub const fn asked_as(driving: Driving) -> usize {
    match driving {
        Driving::TheSink => 1,
        Driving::ThisMachine => 2,
    }
}

/// Which machine an arriving number names, or `None` for one nothing here wrote.
///
/// `None` rather than a guess, because the message range belongs to the application: what
/// can be posting into it is a favjit of another version, and moving the keyboard on a
/// number that version meant differently is the one outcome worth avoiding.
pub const fn from_ask(number: usize) -> Option<Driving> {
    match number {
        1 => Some(Driving::TheSink),
        2 => Some(Driving::ThisMachine),
        _ => None,
    }
}
