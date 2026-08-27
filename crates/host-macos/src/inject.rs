//! Handing converted input to macOS, as a device rather than as synthesised
//! events (ADR-0011).
//!
//! Only the writing. What the reports should say is in [`crate::keyboard`], which
//! is pure and therefore testable; this is the part that cannot be.

use favjit_core::{Injected, Key, PointerReport};

use crate::ffi::*;
use crate::vhid::{Unavailable, VirtualDevice};
use favjit_core::hid::report::{Keyboard, Sent};

/// Sends keyboard and pointing reports to the virtual HID device.
pub struct Injector {
    device: VirtualDevice,
    keyboard: Keyboard,
}

impl Injector {
    pub fn open(wait: core::time::Duration) -> Result<Self, Unavailable> {
        Ok(Self {
            device: VirtualDevice::open(wait)?,
            keyboard: Keyboard::default(),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.device.is_connected()
    }

    /// Send one event, or report the key that has no usage to send it with.
    pub fn post(&mut self, injected: Injected) -> Result<(), Key> {
        let (key, modifiers, down) = match injected {
            Injected::Pointer(report) => {
                self.device.send_pointer(report);
                return Ok(());
            }
            Injected::Modifiers(keys) => {
                let report = self.keyboard.modifiers(keys);
                self.device.send(Sent::Keyboard(report));
                return Ok(());
            }
            Injected::KeyDown { key, modifiers } => (key, modifiers, true),
            Injected::KeyUp { key, modifiers } => (key, modifiers, false),
        };

        for report in self.keyboard.key(key, modifiers, down)? {
            self.device.send(report);
        }
        Ok(())
    }

    /// Let go of everything, without taking the device away from other clients.
    ///
    /// The device outlives this process — it belongs to a daemon — so a modifier
    /// left down in the last report stays down for whatever runs next. Terminating
    /// the device instead would take it from any other client also using it.
    pub fn release_all(&mut self) {
        for report in self.keyboard.release_all() {
            self.device.send(report);
        }
        self.device.send_pointer(PointerReport::default());
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Whether this process may listen to and post key events, asked without
/// triggering a prompt.
///
/// Output goes out as a device now, so posting is not what carries a keystroke —
/// but the answer still says whether this process has the access that listening to
/// a tap would need, which is worth reporting when a run finds nothing.
pub fn access() -> (bool, bool) {
    unsafe { (CGPreflightListenEventAccess(), CGPreflightPostEventAccess()) }
}

/// Whether this process may receive HID reports: granted, denied, or never asked.
///
/// The question [`access`] answers is the window server's, and it is the wrong one
/// for a run that captures through `IOHIDDevice` — a daemon has no window server
/// session, so that answer says nothing about whether the keyboards can be read
/// (`docs/platform/macos/input-permissions.md`).
pub fn hid_access() -> HidAccess {
    // Anything that is not one of the two decided answers is treated as undecided,
    // rather than matching `HID_ACCESS_UNKNOWN` and leaving a fourth value to fall
    // through: what the OS returns here is not something this side controls.
    match unsafe { IOHIDCheckAccess(HID_REQUEST_LISTEN) } {
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

/// Whether this process is trusted for Accessibility, and ask if it is not.
///
/// Asked here as well as [`hid_access`] because on macOS 26 an Accessibility grant
/// can cover input monitoring, and this is the request that can actually put a
/// dialog in front of somebody — the HID one cannot, so a process with a session
/// should try this first (`docs/platform/macos/input-permissions.md`).
pub fn ax_trusted(prompt: bool) -> bool {
    if !prompt {
        return unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) };
    }
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
