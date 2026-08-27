//! What the machine thinks a held key should do.

use std::ffi::c_void;
use std::time::Duration;

use favjit_core::sink::Repeat;

use crate::cf::CfString;
use crate::ffi::*;

/// The initial delay and the interval, as the OS holds them.
///
/// Read once at start rather than per keystroke: a rate that changed mid-repeat
/// would be a value `core` read that no event carries, which is what ADR-0010
/// keeps out of the loop. Changing it takes a restart, and that is the trade.
///
/// `None` when the values are not there to read, which leaves the sink with no
/// repeat at all rather than a number invented here.
pub fn system_repeat() -> Option<Repeat> {
    let parameters = hid_parameters()?;
    let initial = nanos(parameters.as_ref(), "HIDInitialKeyRepeat");
    let interval = nanos(parameters.as_ref(), "HIDKeyRepeat");
    match (initial, interval) {
        (Some(initial), Some(interval)) => Some(Repeat {
            initial: Duration::from_nanos(initial),
            interval: Duration::from_nanos(interval),
        }),
        _ => None,
    }
}

/// A `CFDictionaryRef` that releases itself.
struct Parameters(CFDictionaryRef);

impl Parameters {
    fn as_ref(&self) -> CFDictionaryRef {
        self.0
    }
}

impl Drop for Parameters {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

/// The `HIDParameters` dictionary off the first `IOHIDSystem` in the registry.
fn hid_parameters() -> Option<Parameters> {
    let key = CfString::new(HID_PARAMETERS);
    let mut services: IoIterator = 0;
    let matching = unsafe { IOServiceMatching(IOHID_SYSTEM_CLASS.as_ptr()) };
    if unsafe { IOServiceGetMatchingServices(0, matching, &mut services) } != kIOReturnSuccess {
        return None;
    }
    let mut found = None;
    loop {
        let service = unsafe { IOIteratorNext(services) };
        if service == 0 {
            break;
        }
        // The iterator is drained rather than left after the first hit, because
        // an abandoned iterator holds its remaining entries.
        if found.is_none() {
            found = dictionary(service, &key);
        }
        unsafe { IOObjectRelease(service) };
    }
    unsafe { IOObjectRelease(services) };
    found
}

fn dictionary(service: IoObject, key: &CfString) -> Option<Parameters> {
    let value =
        unsafe { IORegistryEntryCreateCFProperty(service, key.as_ref(), std::ptr::null(), 0) };
    if value.is_null() {
        return None;
    }
    // Type-checked before use: a property that came back as something else would
    // otherwise be read as a dictionary, and the crash would be inside CF.
    if unsafe { CFGetTypeID(value) } != unsafe { CFDictionaryGetTypeID() } {
        unsafe { CFRelease(value) };
        return None;
    }
    Some(Parameters(value))
}

fn nanos(parameters: CFDictionaryRef, name: &str) -> Option<u64> {
    let key = CfString::new(name);
    // Not retained and not released: `CFDictionaryGetValue` hands back a
    // reference the dictionary keeps owning, and the dictionary outlives this.
    let value = unsafe { CFDictionaryGetValue(parameters, key.as_ref()) };
    if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFNumberGetTypeID() } {
        return None;
    }
    let mut read: i64 = 0;
    let ok = unsafe {
        CFNumberGetValue(
            value,
            kCFNumberSInt64Type,
            &mut read as *mut i64 as *mut c_void,
        )
    };
    // A zero or negative interval would be a deadline that is always due, which
    // is an event loop that never waits again.
    (ok && read > 0).then_some(read as u64)
}
