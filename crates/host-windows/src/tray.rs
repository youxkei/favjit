//! The calls the tray item's channel is carried by (`docs/platform/windows/tray-item-as-its-own-program.md`).
//!
//! Where the window is found, what the state is published as, which message an ask
//! arrives in and what each number means are `favjit_tray_wire`'s — every one of those is
//! favjit's own name rather than the platform's, which is the one thing ADR-0006 does not
//! let a host state. What is here is the calls, one per operation.
//!
//! **Finding the window and reading the property are two operations**, and the item that
//! wants both composes them: a run is not the only program that talks over this channel,
//! and one call that did both would hide the answer that matters most — that there is no
//! window, so nothing is forwarding at all.
//!
//! A window property rather than a file, because it dies with the process: a file would
//! outlive the run that wrote it, so an item drawn from one would show a keyboard driving
//! the Mac when nothing at all is forwarding
//! ([../../../docs/platform/windows/window-properties-and-messages.md](../../../docs/platform/windows/window-properties-and-messages.md)
//! is the measurement, including that neither of the item's reads waits on the thread
//! holding the keyboards).

use core::ptr::null_mut;

use favjit_host::source::{Driving, Suppressing};

use crate::ffi::*;

/// A running favjit, as the window it can be reached at.
///
/// Opaque, so that what a caller outside this crate holds is "the run" rather than a
/// Win32 handle it could pass anywhere.
#[derive(Debug, Clone, Copy)]
pub struct Running(Handle);

/// The running favjit on this machine, if there is one.
pub fn running() -> Option<Running> {
    let class = wide(favjit_tray_wire::WINDOW_CLASS);
    a_run(unsafe { FindWindowW(class.as_ptr(), null_mut()) })
}

/// The run a window stands for, and nothing where there is no window.
fn a_run(window: Handle) -> Option<Running> {
    (!window.is_null()).then_some(Running(window))
}

/// What that run is refusing, which is where the keyboard is.
///
/// `None` for a number this channel cannot read, which includes the property a run has
/// not published one on yet.
pub fn refusing(run: Running) -> Option<Suppressing> {
    let name = wide(favjit_tray_wire::REFUSING);
    let published = unsafe { GetPropW(run.0, name.as_ptr()) } as usize;
    favjit_tray_wire::from_refusing(published)
}

/// Ask that run for the keyboard to be driving this machine or the other one.
///
/// `false` when the message could not be posted. The ask is not answered: what it moves
/// is a state the run's loop owns, and reading it back is [`refusing`] on the next draw.
pub fn ask(run: Running, driving: Driving) -> bool {
    let asked = favjit_tray_wire::asked_as(driving);
    unsafe { PostMessageW(run.0, favjit_tray_wire::WM_ASKED, asked, 0) != 0 }
}

/// Ask that run to stop.
///
/// `false` when the message could not be posted. What it stops is the forwarding, which
/// is what a person reaching for Quit wants: the keyboards come back and nothing brings
/// the run up again until the next logon.
pub fn stop(run: Running) -> bool {
    unsafe { PostMessageW(run.0, favjit_tray_wire::WM_STOP, 0, 0) != 0 }
}

/// Publish what is being refused, so the tray item can draw it.
///
/// Called from where the refusing itself is decided, so the two cannot come apart.
///
/// Unchecked: a failure here costs a tray item drawn one state behind, and there is
/// nothing to do about it worth doing on the thread holding the keyboards.
pub(crate) fn say(window: Handle, what: Suppressing) {
    let name = wide(favjit_tray_wire::REFUSING);
    let state = favjit_tray_wire::refusing_as(what);
    unsafe { SetPropW(window, name.as_ptr(), state as Handle) };
}

/// Which machine an arriving ask names, if it names one.
pub(crate) fn asked(wparam: Wparam) -> Option<Driving> {
    favjit_tray_wire::from_ask(wparam)
}
