//! How far macOS moves the cursor for a device, and how to change it.
//!
//! The distance a pointing device covers is not only what its reports say: macOS
//! scales them per device, from two properties on the HID event system's view of
//! the device rather than on the `IOHIDDevice` that capture reads
//! (`docs/platform/macos/pointer-acceleration.md`).
//!
//! Reached from here rather than by multiplying the reports favjit relays, for the
//! same reason a mouse has a resolution and not a multiplier: a factor large enough
//! to feel right also multiplies the smallest report the hardware can make, so slow
//! movement goes in steps. Telling the OS what kind of device it is keeps the OS's
//! own curve, and the curve is what makes a slow drag precise and a quick one fast.

use crate::cf::CfString;
use crate::ffi::*;

// The bounds these two properties are kept in are not here: what a number outside
// them becomes is answerable without a device, so this crate is handed the values
// already clamped rather than clamping them itself.

/// A pointing device as the event system sees it.
///
/// Four readers and nothing that reads them: which property holds this device's
/// acceleration, which of the machine's pointers is favjit's own, and the order
/// the two writes have to go in are all decided by the run, where the suite
/// drives them (ADR-0006, ADR-0011).
pub struct Pointer {
    service: IOHIDServiceClientRef,
}

impl favjit_host::Pointing for Pointer {
    /// A 16.16 fixed-point property, as the number it stands for.
    ///
    /// The divide is the conversion: macOS keeps these as an integer with the
    /// binary point in the middle, and a caller handed the raw value would be
    /// handed 65536 times the resolution the device actually claims.
    fn fixed(&mut self, key: &str) -> Option<f64> {
        // `CopyProperty` is the one call; the key and the read are data
        // conversion, the way [`crate::cf::device_number`]'s are.
        let name = CfString::new(key);
        fixed_in(unsafe { IOHIDServiceClientCopyProperty(self.service, name.as_ref()) })
    }

    /// A plain integer property, for the numbers that are not fixed point.
    fn integer(&mut self, key: &str) -> Option<i64> {
        // The same one call as [`favjit_host::Pointing::fixed`], read without
        // the fixed-point divide.
        let name = CfString::new(key);
        integer_in(unsafe { IOHIDServiceClientCopyProperty(self.service, name.as_ref()) })
    }

    /// A string property.
    fn text(&mut self, key: &str) -> Option<String> {
        // The same one call again, read as a string instead.
        let name = CfString::new(key);
        text_in(unsafe { IOHIDServiceClientCopyProperty(self.service, name.as_ref()) })
    }

    /// Write a 16.16 fixed-point property, multiplying back out.
    fn set_fixed(&mut self, key: &str, value: f64) -> bool {
        // `SetProperty` is the one call, and the only one here that changes
        // anything: the key and the number it takes are data conversion, and the
        // number is let go of by going out of scope rather than by a release
        // beside the call.
        let name = CfString::new(key);
        let number = CfNumber::fixed(value);
        unsafe { IOHIDServiceClientSetProperty(self.service, name.as_ref(), number.as_ref()) }
    }
}

/// The number a 16.16 fixed-point property stands for, and the value let go of.
///
/// The divide is the conversion: macOS keeps these as an integer with the binary
/// point in the middle, so a caller handed the raw value would be handed 65536
/// times the resolution the device claims.
fn fixed_in(value: CFTypeRef) -> Option<f64> {
    if value.is_null() {
        return None;
    }
    let mut raw: i32 = 0;
    let read = unsafe {
        CFNumberGetValue(
            value,
            kCFNumberSInt32Type,
            &mut raw as *mut i32 as *mut core::ffi::c_void,
        )
    };
    unsafe { CFRelease(value) };
    read.then(|| raw as f64 / 65536.0)
}

/// The plain integer a property holds, and the value let go of.
fn integer_in(value: CFTypeRef) -> Option<i64> {
    if value.is_null() {
        return None;
    }
    let mut raw: i64 = 0;
    let read = unsafe {
        CFNumberGetValue(
            value,
            kCFNumberSInt64Type,
            &mut raw as *mut i64 as *mut core::ffi::c_void,
        )
    };
    unsafe { CFRelease(value) };
    read.then_some(raw)
}

/// The text a property holds, and the value let go of.
///
/// The buffer is fixed and the text is cut to the first nul in it: these
/// properties hold a device's kind, which is a word.
fn text_in(value: CFTypeRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut buffer = [0i8; 128];
    let read = unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            buffer.len() as CFIndex,
            kCFStringEncodingUTF8,
        )
    };
    unsafe { CFRelease(value) };
    if !read {
        return None;
    }
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8(bytes).ok()
}

/// A number these properties take, which is let go of when this goes.
///
/// A type rather than a pointer and a release beside the call that uses it: the
/// release would be the second thing in a body that makes one call (ADR-0006),
/// and a path out that skipped it would leak.
struct CfNumber(Option<crate::cf::Owned>);

impl CfNumber {
    /// A number as the 16.16 fixed-point value these properties take.
    ///
    /// The multiply is the conversion, and the cast saturates rather than
    /// checking: what a value outside these properties' range becomes is
    /// answerable without a device, so this crate is handed them already clamped
    /// (the note at the top of this file).
    fn fixed(value: f64) -> Self {
        let raw = (value * 65536.0) as i32;
        Self(crate::cf::there_is_one(unsafe {
            CFNumberCreate(
                core::ptr::null(),
                kCFNumberSInt32Type,
                &raw as *const i32 as *const core::ffi::c_void,
            )
        }))
    }

    /// The pointer the call takes, and null where Core Foundation made no
    /// number — which the call is the thing that answers for.
    fn as_ref(&self) -> CFTypeRef {
        self.0
            .as_ref()
            .map_or(core::ptr::null(), crate::cf::Owned::pointer)
    }
}

/// The view a client pointer is, and nothing where it is null.
fn a_view(client: IOHIDEventSystemClientRef) -> Option<EventSystem> {
    (!client.is_null()).then_some(EventSystem { client })
}

/// The event system's own view of this machine's devices.
///
/// A type of its own rather than the client itself, so what the run holds is
/// this view and not a platform pointer: the crate that names no platform must
/// not be handed one (ADR-0006).
pub struct EventSystem {
    client: IOHIDEventSystemClientRef,
}

/// Open the writable view, or nothing where it will not open.
///
/// The one LinearMouse uses to set these same properties, and the one worth
/// asking for first — but which of the two a run settles for is the run's, so
/// this answers with the one it opened and nothing else (ADR-0006).
pub fn event_system() -> Option<EventSystem> {
    a_view(unsafe { IOHIDEventSystemClientCreate(core::ptr::null()) })
}

/// Open the simpler view of it, which reads these properties and cannot write
/// them.
pub fn simple_event_system() -> Option<EventSystem> {
    a_view(unsafe { IOHIDEventSystemClientCreateSimpleClient(core::ptr::null()) })
}

/// Either view as the one a run holds, and nothing where neither opened.
pub fn viewing(opened: Option<EventSystem>) -> Option<Box<dyn favjit_host::Pointers>> {
    opened.map(|view| Box::new(view) as Box<dyn favjit_host::Pointers>)
}

impl favjit_host::Pointers for EventSystem {
    fn look(&mut self) -> Vec<Box<dyn favjit_host::Pointing>> {
        services_in(unsafe { IOHIDEventSystemClientCopyServices(self.client) })
    }
}

/// Every service in the array the event system handed over.
///
/// Several of Core Foundation's own calls and not one of them asks the machine
/// anything: a `CFArray` cannot be read without asking how long it is, and the
/// array is released here while the services it held are not — they are what
/// the returned pointers point at (the exception stated at [`crate::cf`]'s
/// calls).
fn services_in(services: CFArrayRef) -> Vec<Box<dyn favjit_host::Pointing>> {
    if services.is_null() {
        return Vec::new();
    }
    let mut found: Vec<Box<dyn favjit_host::Pointing>> = Vec::new();
    let count = unsafe { CFArrayGetCount(services) };
    for i in 0..count {
        let service = unsafe { CFArrayGetValueAtIndex(services, i) };
        if service.is_null() {
            continue;
        }
        found.push(Box::new(Pointer { service }));
    }
    // The array only. The client is not released either, for the same reason:
    // this runs once for the life of a process, and releasing what a `Pointer`
    // points at would leave every one of them dangling to save nothing.
    unsafe { CFRelease(services) };
    found
}
