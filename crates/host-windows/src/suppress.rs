//! Taking this machine's input away from it, and giving it back.
//!
//! A low-level hook is the only Windows input hook that can refuse an event rather than
//! merely watch it: returning non-zero from one ends the event, and nothing downstream
//! sees it. **Raw input is downstream too**, which is the fact this module is shaped
//! around — a key the hook refuses reaches nothing at all, favjit included
//! ([docs/platform/windows/hooks-and-raw-input.md](../../../docs/platform/windows/hooks-and-raw-input.md)).
//!
//! So **the keyboard hook is where keys are read from**, and not only where they are
//! refused: it reports the key first and turns it down second, which is the only order
//! in which both can happen. Deskflow's Windows hook is the same shape, and its
//! `RegisterRawInputDevices` count is zero.
//!
//! What that costs is the device: `KBDLLHOOKSTRUCT` says which key and not which
//! keyboard, so every key this machine forwards arrives as one unnamed external
//! keyboard. ADR-0003 allows for it — "where that information is unavailable, the
//! single pipeline still stands but per-device rules are not expressible" — and
//! `--ansi` is already a property of the machine rather than of a keyboard.
//!
//! The pointer is the other way round: a hook that refuses a pointer event leaves raw
//! input arriving, so the mouse is captured there, where its movement is a movement
//! rather than a cursor position (ADR-0016).
//!
//! **What suppresses is one number**, and the procedures read it per event. An ordinary
//! exit gives the input back by dropping the host, and nothing else has to run for that.
//!
//! **A wedge is not covered.** ADR-0008 requires a watchdog wherever suppression is
//! held, and the one favjit has supervises through POSIX pipes, so nothing supervises
//! this side: a relaying run says so and that is the whole of it. Keeping each procedure
//! to a couple of atomic reads and one post makes a wedge *in them* unreachable, which
//! is not the same as covering a wedge in the loop they are refusing input for.

use core::ptr::null_mut;
use core::sync::atomic::{AtomicU16, AtomicU8, AtomicUsize, Ordering};

use favjit_core::source::{Suppressing, SWITCH_BACK, SWITCH_TO_THE_SINK};
use log::{info, warn};

use crate::ffi::*;
use crate::scancode;

/// What is being refused as things stand, as [`Suppressing`] numbered.
///
/// A static because a hook procedure is a bare function pointer with nowhere to hang a
/// context off: Windows calls it with the event and nothing else. A number rather than
/// the enum, because that is what an atomic holds.
static REFUSING: AtomicU8 = AtomicU8::new(NOTHING);

const NOTHING: u8 = 0;
const THE_SWITCH: u8 = 1;
const EVERYTHING: u8 = 2;

/// The make codes the chord is made of.
///
/// Found once, while the hooks are installed, from the keys `core` names — so the
/// procedure compares two numbers rather than consulting a table. Zero for a key this
/// keyboard has no position for, which matches nothing: a make code of zero is Windows
/// saying it has no scan code for the event.
static TO_THE_SINK: AtomicU16 = AtomicU16::new(0);
static BACK_HERE: AtomicU16 = AtomicU16::new(0);

/// The window keys are handed to.
static KEYS_GO_TO: AtomicUsize = AtomicUsize::new(0);

/// How many pointer events have been refused.
///
/// Counted because a run that reports none of them while it was relaying is one whose
/// mouse reached neither machine, and the number is the only thing that says which. No
/// count for keys: what refuses them is what reports them, so "refused but not
/// captured" is not a state a key can be in.
static POINTERS: AtomicUsize = AtomicUsize::new(0);

/// Where keys are handed to, which is the thread that installed the hooks.
pub fn keys_go_to(window: Handle) {
    KEYS_GO_TO.store(window as usize, Ordering::SeqCst);
}

/// Refuse this machine's own input, or stop refusing it.
pub fn take(what: Suppressing) {
    REFUSING.store(
        match what {
            Suppressing::Nothing => NOTHING,
            Suppressing::TheSwitch => THE_SWITCH,
            Suppressing::Everything => EVERYTHING,
        },
        Ordering::SeqCst,
    );
}

/// How many pointer events have been refused.
pub fn refused() -> usize {
    POINTERS.load(Ordering::SeqCst)
}

/// The two hooks, for as long as they are installed.
///
/// Held by the capture thread and dropped with it. Dropping is not what ends
/// suppression — [`take`]'s number is — because this value only goes out of scope on
/// the paths that were going to end anyway, and the paths that matter are the ones
/// where no code of ours runs at all.
pub struct Hooks {
    keyboard: Handle,
    mouse: Handle,
}

impl Hooks {
    /// Install both, on the calling thread.
    ///
    /// The thread that installs a low-level hook is the thread its procedure is called
    /// on, so this has to be the thread with the message loop — a hook installed on a
    /// thread that never pumps messages is never called, and the symptom is input that
    /// is neither captured nor suppressed with nothing logged anywhere.
    pub fn install() -> Option<Self> {
        // The chord's positions, found before either procedure can be called. A key
        // this keyboard has no position for leaves a zero, which matches nothing — so
        // the chord does not move the keyboard, rather than a wrong position doing it.
        for (key, code) in [
            (SWITCH_TO_THE_SINK, &TO_THE_SINK),
            (SWITCH_BACK, &BACK_HERE),
        ] {
            match scancode::code_for(key) {
                Some(found) => code.store(found, Ordering::SeqCst),
                None => warn!(
                    "no position on this keyboard produces {key:?}; the chord that moves the \
                     keyboard is short of a key"
                ),
            }
        }

        let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, keyboard_hook, null_mut(), 0) };
        if keyboard.is_null() {
            warn!("cannot hook the keyboard: {}", last_error());
            return None;
        }
        let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, mouse_hook, null_mut(), 0) };
        if mouse.is_null() {
            // Both or neither: this is where the keys come from, so a run with one of
            // them is a run relaying half of what was typed.
            warn!("cannot hook the mouse: {}", last_error());
            unsafe { UnhookWindowsHookEx(keyboard) };
            return None;
        }
        info!("the keyboard and mouse hooks are in place");
        Some(Self { keyboard, mouse })
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        take(Suppressing::Nothing);
        unsafe {
            UnhookWindowsHookEx(self.keyboard);
            UnhookWindowsHookEx(self.mouse);
        }
    }
}

/// Whether this is the chord that moves the keyboard.
///
/// Unprefixed only, and both halves from the event itself: a procedure that counted the
/// modifier for itself would be wrong about every key held before it was installed.
/// What alt being down means is not read here at all — `core` follows the option keys
/// as they arrive, and this is only about which one key it is.
fn is_the_switch(event: &KeyboardHookEvent) -> bool {
    if event.flags & LLKHF_EXTENDED != 0 {
        return false;
    }
    let code = event.make_code as u16;
    code != 0
        && (code == TO_THE_SINK.load(Ordering::SeqCst) || code == BACK_HERE.load(Ordering::SeqCst))
}

/// Hand the key to the capture loop, and say whether to refuse it.
///
/// Reported before it is refused, and unconditionally: the loop that decides what a key
/// means has to see every one of them, including the ones this machine is not going to
/// get. That order is the whole reason the keys come from here.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: Wparam, lparam: Lparam) -> Lresult {
    if code != HC_ACTION {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let event = &*(lparam as *const KeyboardHookEvent);

    // Injected events are neither reported nor refused. There is no device behind one,
    // so nothing can relay it, and refusing what cannot be relayed is taking input away
    // with nothing to show for it — the shape of failure ADR-0008 rules out, in
    // miniature.
    if event.flags & LLKHF_INJECTED != 0 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let window = KEYS_GO_TO.load(Ordering::SeqCst) as Handle;
    if !window.is_null() {
        // The make code and the vkey in one word, the flags in the other. A post that
        // fails is a key that reached nothing, and the run reports the count.
        let packed = (event.make_code as usize & 0xFFFF) | ((event.vkey as usize & 0xFFFF) << 16);
        PostMessageW(window, WM_KEY, packed, event.flags as Lparam);
    }

    let refuse = match REFUSING.load(Ordering::SeqCst) {
        EVERYTHING => true,
        // The chord alone, so pressing it moves the keyboard rather than also reaching
        // whatever has the foreground.
        THE_SWITCH => is_the_switch(event),
        _ => false,
    };
    if refuse {
        // Non-zero, which is what ends the event. Everything after this — the foreground
        // application, the shell's own hotkeys — never hears it.
        return 1;
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// Whether to refuse this pointer event.
///
/// The pointer is not read from here: a hook that refuses one leaves raw input arriving,
/// and raw input is where a movement is still a movement.
unsafe extern "system" fn mouse_hook(code: i32, wparam: Wparam, lparam: Lparam) -> Lresult {
    // Read before the decision rather than after, so a hook called with a code that
    // carries no event never dereferences the pointer.
    let injected = code == HC_ACTION && {
        let event = &*(lparam as *const MouseHookEvent);
        event.flags & LLMHF_INJECTED != 0
    };
    if code == HC_ACTION && !injected && REFUSING.load(Ordering::SeqCst) == EVERYTHING {
        POINTERS.fetch_add(1, Ordering::SeqCst);
        return 1;
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}
