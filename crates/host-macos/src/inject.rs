//! Handing converted input to macOS, as a device rather than as synthesised
//! events (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
//!
//! Only the writing. Rendering an `Injected` event against the device's state is
//! `engine`'s, ahead of the call here — this takes what came out of that and
//! writes it, which is the one part of it that cannot be driven by the suite
//! (ADR-0006).

use favjit_host::OutputReport;

use crate::ffi::*;
use crate::vhid::{Reaching, Serving, VirtualDevice};

/// Sends keyboard and pointing reports to the virtual HID device.
pub struct Injector {
    device: VirtualDevice,
}

/// The open connection, as the run holds it before it is spoken on.
///
/// A type of this crate's own around [`Reaching`] rather than the boundary trait
/// on that one directly: what the socket is and how it is framed is `vhid`'s,
/// and which of these two ends converted input is written to is this file's
/// (ADR-0006).
pub struct Reached(Reaching);

impl Reached {
    /// Reach the service, and hand the connection over as it is: what is asked
    /// of it, in what order, and how long its answers are allowed are the run's
    /// (ADR-0006).
    pub fn reach(
        clock: crate::capture::Clock,
        events: std::sync::mpsc::Sender<crate::capture::Captured>,
    ) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
        reached(crate::vhid::reach(clock, events))
    }
}

impl favjit_host::sink::Reaching for Reached {
    fn do_not_block_writes(&mut self, write_timeout: core::time::Duration) -> bool {
        self.0.bound_writes(write_timeout)
    }

    fn both_ends(self: Box<Self>) -> Option<favjit_host::sink::Opened> {
        opened(self.0.both_ends())
    }
}

/// The connection a reach answered with, as the run holds it, or what the
/// machine said about there being no service to reach.
fn reached(
    answered: Result<Reaching, favjit_host::NoOutput>,
) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
    answered.map(|reaching| Box::new(Reached(reaching)) as Box<dyn favjit_host::sink::Reaching>)
}

impl favjit_host::sink::Injecting for Injector {
    /// Write what `engine` rendered.
    fn send_report(&mut self, report: OutputReport, bytes: &[u8]) -> i32 {
        self.device.send(report, bytes)
    }
}

/// Where converted input goes on a run that opens no device.
///
/// A type of its own rather than a flag inside [`Injector`], because there is no
/// device behind this one and nothing here to answer for: a writer that checked
/// whether it had a device would be the question every write against it asks
/// (ADR-0006).
pub struct Nowhere;

impl favjit_host::sink::Injecting for Nowhere {
    /// `0`, the same as a write that went: nothing was asked of the machine, so
    /// there is no number of its to carry — and a run told the write failed
    /// would say that converted input is reaching nothing, which is true of
    /// every run in this mode and worth saying about none of them.
    fn send_report(&mut self, _report: OutputReport, _bytes: &[u8]) -> i32 {
        0
    }
}

/// The two ends of the connection as the run holds them: the loop it serves
/// alongside its own, and what converted input is written to.
fn opened(ends: Option<(VirtualDevice, Serving)>) -> Option<favjit_host::sink::Opened> {
    ends.map(|(device, serving)| favjit_host::sink::Opened {
        serving: Box::new(serving),
        writing: Box::new(Injector { device }),
    })
}

// The window server's own answer about listening to and posting events is not
// asked here. Output goes out as a device (`docs/platform/macos/output-through-a-virtual-hid-device.md`), so posting is not what
// carries a keystroke, and what capture needs is [`hid_access`] — a run that
// reported the window server's answer would be reporting a permission it does
// not use (`docs/platform/macos/input-permissions.md`).

/// Whether this process may receive HID reports: granted, denied, or never asked.
///
/// The question [`access`] answers is the window server's, and it is the wrong one
/// for a run that captures through `IOHIDDevice` — a daemon has no window server
/// session, so that answer says nothing about whether the keyboards can be read
/// (`docs/platform/macos/input-permissions.md`).
pub fn hid_access() -> HidAccess {
    access_of(unsafe { IOHIDCheckAccess(HID_REQUEST_LISTEN) })
}

/// What the OS's own answer about HID access stands for.
///
/// Anything that is not one of the two decided answers is undecided, rather than
/// matching `HID_ACCESS_UNKNOWN` and leaving a fourth value to fall through: what
/// the OS returns here is not something this side controls.
fn access_of(answered: u32) -> HidAccess {
    match answered {
        HID_ACCESS_GRANTED => HidAccess::Granted,
        HID_ACCESS_DENIED => HidAccess::Denied,
        _ => HidAccess::Unknown,
    }
}

/// Ask the user for it, and say what came back.
///
/// Worth calling even where no prompt can appear, because asking is also what puts
/// the process in the Input Monitoring list — a switch to turn on is a shorter path
/// than finding a binary under `/usr/local/libexec` in a file dialog.
pub fn request_hid_access() -> bool {
    unsafe { IOHIDRequestAccess(HID_REQUEST_LISTEN) }
}

/// Whether this process is trusted for Accessibility.
///
/// Asked as well as [`hid_access`] because on macOS 26 an Accessibility grant can
/// cover input monitoring (`docs/platform/macos/input-permissions.md`).
///
/// Its own operation and not a flag on [`ax_trusted_asking`]: the two pass
/// different options to the same call, and which of them a run makes is the run's
/// — asking puts a dialog in front of somebody (ADR-0006).
pub fn ax_trusted() -> bool {
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

/// The same question with the option that puts a dialog up.
///
/// This is the request that can actually prompt — the HID one cannot — so a
/// process with a session has this to try (`docs/platform/macos/input-permissions.md`).
pub fn ax_trusted_asking() -> bool {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt as CFTypeRef];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        CFRelease(options);
        trusted
    }
}

/// What [`hid_access`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidAccess {
    Granted,
    Denied,
    /// Never asked. Distinct from denied because it is the state a request can
    /// still change, and because it is what a fresh install starts in.
    Unknown,
}
