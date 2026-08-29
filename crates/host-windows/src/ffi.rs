//! The Win32 declarations this host needs, and nothing else.
//!
//! Written out rather than taken from a binding crate, for the reason the macOS
//! host writes its own: what is used here is two dozen calls and eight
//! structures, and a generated binding to the whole API surface is a large
//! dependency whose contents nobody reads. The structures are the part that has
//! to be right — a field missing from one of them is a value read from the wrong
//! offset, which decodes as a different key rather than as an error — so each one
//! is laid out beside the header's own order and [`tests`] measures it.

use core::ffi::c_void;

/// Every opaque Win32 handle: a window, a hook, a module, a device.
///
/// One type for all of them because none of them is dereferenced here. They are
/// numbers this code passes back to the API it got them from.
pub type Handle = *mut c_void;

pub type Wparam = usize;
pub type Lparam = isize;
pub type Lresult = isize;

/// A window with no screen presence, which is what a raw input target has to be.
///
/// `HWND_MESSAGE`. Passed as the parent, which is what makes the window one that
/// receives messages and never draws.
pub const HWND_MESSAGE: Handle = -3isize as Handle;

pub const WM_INPUT_DEVICE_CHANGE: u32 = 0x00FE;
pub const WM_INPUT: u32 = 0x00FF;

/// What `wParam` says a device change was.
///
/// Only the removal, because arrival is not asked about: a device is learned from
/// its first event. `GIDC_ARRIVAL` is the other value, and it is 1.
pub const GIDC_REMOVAL: Wparam = 2;

/// Usage page 1, and the one usage worth registering for.
///
/// No keyboard usage beside it: what reads a key here is a hook rather than raw input
/// ([`crate::suppress`]).
pub const USAGE_PAGE_GENERIC: u16 = 0x01;
pub const USAGE_MOUSE: u16 = 0x02;

/// Deliver input to this window whether or not it has the foreground, and say
/// when a device comes and goes.
pub const RIDEV_INPUTSINK: u32 = 0x0000_0100;
pub const RIDEV_DEVNOTIFY: u32 = 0x0000_2000;

/// The message a key is handed to the capture loop with.
///
/// Posted by the hook procedure rather than pushed into the channel directly, because a
/// procedure is a bare function pointer with nowhere to hang a channel off — and
/// posting is what the platform gives it for exactly this. Deskflow's Windows hook
/// hands keys to its own thread the same way, for the same reason.
///
/// The event travels in the two words a message carries: the make code and the vkey in
/// one, the hook's flags in the other. Nothing else about it is needed
/// ([`crate::scancode`] reads a key out of those three).
pub const WM_KEY: u32 = 0x8000;

/// What to ask [`GetRawInputData`] for.
pub const RID_INPUT: u32 = 0x1000_0003;

/// The device's interface path, which is where its vendor and product are.
pub const RIDI_DEVICENAME: u32 = 0x2000_0007;

pub const RIM_TYPEMOUSE: u32 = 0;
pub const RIM_TYPEKEYBOARD: u32 = 1;

// The bits in `RAWKEYBOARD.Flags` are in [`crate::scancode`], with the table that
// reads them: what they mean is the keyboard's encoding rather than a Win32
// signature, and that module is the one with tests over it.

/// The two hooks that can refuse an event rather than only watch it.
pub const WH_KEYBOARD_LL: i32 = 13;
pub const WH_MOUSE_LL: i32 = 14;

/// The only hook code either of them is called with that carries an event.
pub const HC_ACTION: i32 = 0;

/// Whether the event was made by software rather than by a device.
pub const LLKHF_INJECTED: u32 = 0x10;
pub const LLMHF_INJECTED: u32 = 0x01;

/// The bits in `KBDLLHOOKSTRUCT.flags` that say which key this was.
///
/// Whether the position carried the `E0` prefix, which is the one bit read here.
///
/// The rest of what a key says is read by [`crate::scancode`], which names these bits
/// itself: it is compiled everywhere and this file is not.
pub const LLKHF_EXTENDED: u32 = 0x01;

/// `KBDLLHOOKSTRUCT`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KeyboardHookEvent {
    pub vkey: u32,
    pub make_code: u32,
    pub flags: u32,
    pub time: u32,
    pub extra: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// `MSG`, in the header's order.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Msg {
    pub window: Handle,
    pub message: u32,
    pub wparam: Wparam,
    pub lparam: Lparam,
    pub time: u32,
    pub point: Point,
}

/// `WNDCLASSW`. Only two of the ten fields are set; the rest is what a window
/// that never draws needs none of.
#[repr(C)]
pub struct WindowClass {
    pub style: u32,
    pub procedure: Option<unsafe extern "system" fn(Handle, u32, Wparam, Lparam) -> Lresult>,
    pub class_extra: i32,
    pub window_extra: i32,
    pub instance: Handle,
    pub icon: Handle,
    pub cursor: Handle,
    pub background: Handle,
    pub menu_name: *const u16,
    pub class_name: *const u16,
}

/// `RAWINPUTDEVICE`, one per class of device to register for.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RawInputDevice {
    pub usage_page: u16,
    pub usage: u16,
    pub flags: u32,
    pub target: Handle,
}

/// `RAWINPUTHEADER`, which every raw input starts with.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RawInputHeader {
    pub kind: u32,
    pub size: u32,
    pub device: Handle,
    pub wparam: Wparam,
}

/// `RAWMOUSE`.
///
/// The two bytes after `flags` are the padding in front of the union the header
/// puts there — `ulButtons` and the pair of `USHORT`s are the same four bytes, and
/// this reads them as the pair. Named rather than left implicit, because a
/// structure whose fields are read out of a byte buffer has to say where every
/// byte goes.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RawMouse {
    pub flags: u16,
    pub padding: u16,
    pub button_flags: u16,
    pub button_data: u16,
    pub raw_buttons: u32,
    pub x: i32,
    pub y: i32,
    pub extra: u32,
}

/// `RAWINPUTDEVICELIST`, for enumerating what is attached.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RawInputDeviceList {
    pub device: Handle,
    pub kind: u32,
}

/// `MSLLHOOKSTRUCT`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouseHookEvent {
    pub point: Point,
    pub data: u32,
    pub flags: u32,
    pub time: u32,
    pub extra: usize,
}

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassW(class: *const WindowClass) -> u16;
    pub fn CreateWindowExW(
        extended_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        parameter: *mut c_void,
    ) -> Handle;
    pub fn DestroyWindow(window: Handle) -> i32;
    pub fn DefWindowProcW(window: Handle, message: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
    pub fn GetMessageW(message: *mut Msg, window: Handle, first: u32, last: u32) -> i32;
    pub fn DispatchMessageW(message: *const Msg) -> Lresult;
    pub fn PostMessageW(window: Handle, message: u32, wparam: Wparam, lparam: Lparam) -> i32;

    pub fn RegisterRawInputDevices(devices: *const RawInputDevice, count: u32, size: u32) -> i32;
    pub fn GetRawInputData(
        input: Handle,
        command: u32,
        data: *mut c_void,
        size: *mut u32,
        header_size: u32,
    ) -> u32;
    pub fn GetRawInputDeviceInfoW(
        device: Handle,
        command: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> u32;
    pub fn GetRawInputDeviceList(list: *mut RawInputDeviceList, count: *mut u32, size: u32) -> u32;

    /// No `KillTimer` beside it: a timer belongs to the window it was set on, and
    /// destroying that window is what the capture thread does when it ends.
    pub fn SetTimer(window: Handle, id: usize, elapse_ms: u32, callback: *const c_void) -> usize;

    pub fn SetWindowsHookExW(
        kind: i32,
        procedure: unsafe extern "system" fn(i32, Wparam, Lparam) -> Lresult,
        module: Handle,
        thread: u32,
    ) -> Handle;
    pub fn UnhookWindowsHookEx(hook: Handle) -> i32;
    pub fn CallNextHookEx(hook: Handle, code: i32, wparam: Wparam, lparam: Lparam) -> Lresult;
}

/// The timer that wakes the capture loop to look for a probe, and the message it
/// arrives as.
///
/// One timer, so the id is a constant: the window it is set on has nothing else to
/// be woken for.
pub const PROBE_TIMER: usize = 1;
pub const WM_TIMER: u32 = 0x0113;

/// A pipe handle that answers rather than waiting.
///
/// `PIPE_NOWAIT` on a byte-mode pipe, which is what keeps neither direction of the
/// watchdog link able to stall the loop holding the keyboards (ADR-0006). Accepted on
/// an anonymous pipe, which was established on hardware rather than assumed
/// ([docs/platform/windows/inherited-pipes.md](../../../docs/platform/windows/inherited-pipes.md)).
pub const PIPE_NOWAIT: u32 = 0x0000_0001;

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> Handle;
    pub fn ReadFile(
        file: Handle,
        buffer: *mut c_void,
        to_read: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn WriteFile(
        file: Handle,
        buffer: *const c_void,
        to_write: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn SetNamedPipeHandleState(
        pipe: Handle,
        mode: *mut u32,
        max_collection: *mut u32,
        timeout: *mut u32,
    ) -> i32;
}

/// A Rust string as Win32 wants it: UTF-16, with the terminator.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// What the last failing call set, spelled out.
pub fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes the SDK's headers produce, on the 64-bit build.
    ///
    /// Checked because every one of these structures is filled in by Windows into
    /// memory this crate reserved: one that is too small is a write past the end
    /// of a buffer, and one whose fields have moved is a make code read out of the
    /// middle of a timestamp. Neither fails in a way that says what happened.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_structures_are_the_size_the_headers_say() {
        assert_eq!(size_of::<Msg>(), 48);
        assert_eq!(size_of::<RawInputDevice>(), 16);
        assert_eq!(size_of::<RawInputHeader>(), 24);
        assert_eq!(size_of::<RawMouse>(), 24);
        assert_eq!(size_of::<RawInputDeviceList>(), 16);
        assert_eq!(size_of::<KeyboardHookEvent>(), 24);
        assert_eq!(size_of::<MouseHookEvent>(), 32);
        assert_eq!(size_of::<WindowClass>(), 72);
    }

    #[test]
    fn a_wide_string_is_terminated() {
        assert_eq!(wide("ab"), vec![0x61, 0x62, 0]);
        assert_eq!(wide(""), vec![0]);
    }
}
