//! The little of Core Foundation this host needs, wrapped.

use std::ffi::{c_char, c_void, CString};

use crate::ffi::*;

/// A value there is one of, which lets it go when this goes.
///
/// Releasing nothing is a crash, so the null case is the absence of one of these
/// rather than a case inside its `Drop`: a `Drop` asking whether it holds
/// anything would be a host taking a turning beside the one call it makes
/// (ADR-0006), and there is nothing to ask once a null pointer cannot be one of
/// these at all.
pub struct Owned(CFTypeRef);

impl Owned {
    /// The pointer, for the calls that take one.
    pub fn pointer(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

/// A `CFStringRef` that releases itself.
pub struct CfString(Option<Owned>);

impl CfString {
    pub fn new(s: &str) -> Self {
        let c = CString::new(s).expect("property names hold no interior nul");
        Self(there_is_one(unsafe {
            CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8)
        }))
    }

    pub fn as_ref(&self) -> CFStringRef {
        // Null where there is none, which is what every call taking one of these
        // is handed for a string Core Foundation would not make: the calls
        // themselves answer for it, and this has nothing to decide.
        self.0
            .as_ref()
            .map_or(std::ptr::null(), |owned| owned.0.cast())
    }
}

/// A value there is one of, out of a pointer that may be null.
pub fn there_is_one(value: CFTypeRef) -> Option<Owned> {
    (!value.is_null()).then_some(Owned(value))
}

/// Read an integer property off a HID device.
pub fn device_number(device: IOHIDDeviceRef, name: &str) -> Option<i64> {
    // Data conversion, not a second call: a property name reaches Core Foundation
    // only as one of its own strings.
    let key = CfString::new(name);
    number_in(unsafe { IOHIDDeviceGetProperty(device, key.as_ref()) })
}

/// The integer a Core Foundation value holds, where it holds one.
///
/// A `CFTypeRef` cannot be read without asking it what it is, and the type it is
/// asked against is Core Foundation's own — so the asking is part of taking the
/// number out and not a second question about the device.
fn number_in(value: CFTypeRef) -> Option<i64> {
    if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFNumberGetTypeID() } {
        return None;
    }
    let mut n: i64 = 0;
    let ok = unsafe {
        CFNumberGetValue(
            value,
            kCFNumberSInt64Type,
            &mut n as *mut i64 as *mut c_void,
        )
    };
    ok.then_some(n)
}

/// Read a string property off a HID device.
pub fn device_string(device: IOHIDDeviceRef, name: &str) -> Option<String> {
    // Both conversions, for the reason [`device_number`] gives.
    let key = CfString::new(name);
    string_in(unsafe { IOHIDDeviceGetProperty(device, key.as_ref()) })
}

/// The text a Core Foundation value holds, where it holds any.
///
/// The buffer is fixed and the text is cut to the first nul in it: what a device
/// calls itself is a name, and 256 bytes is more than one of those.
fn string_in(value: CFTypeRef) -> Option<String> {
    if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let mut buf = [0 as c_char; 256];
    let ok = unsafe {
        CFStringGetCString(
            value,
            buf.as_mut_ptr(),
            buf.len() as CFIndex,
            kCFStringEncodingUTF8,
        )
    };
    if !ok {
        return None;
    }
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
