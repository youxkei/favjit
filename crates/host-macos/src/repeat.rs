//! What the machine thinks a held key should do.

use std::ffi::c_void;
use std::time::Duration;

use crate::cf::CfString;
use crate::ffi::*;

/// Every `IOHIDSystem` the registry holds, which release themselves as they go.
///
/// The lookup, the step and the property read are three calls into the machine,
/// so they are three operations here and the order between them is whoever asks
/// (ADR-0006): a property can only be read off a service a step handed over, and
/// nothing about that is decided in this file.
pub struct Systems(IoIterator);

impl Systems {
    /// The next one, and nothing once the iterator has run dry.
    pub fn next(&self) -> Option<System> {
        the_next_system(unsafe { IOIteratorNext(self.0) })
    }
}

impl Drop for Systems {
    fn drop(&mut self) {
        crate::capture::release_service(self.0);
    }
}

/// One of them, let go of when this goes.
///
/// Let go of by dropping rather than beside the read: an iterator whose entries
/// are abandoned holds them, and a release written after each read is one a path
/// out of the loop can skip.
pub struct System(IoObject);

impl System {
    /// The `HIDParameters` dictionary, where this one has it.
    pub fn parameters(&self) -> Option<Parameters> {
        let key = CfString::new(HID_PARAMETERS);
        parameters_in(unsafe {
            IORegistryEntryCreateCFProperty(self.0, key.as_ref(), std::ptr::null(), 0)
        })
    }
}

impl Drop for System {
    fn drop(&mut self) {
        crate::capture::release_service(self.0);
    }
}

/// Look for them.
pub fn hid_systems() -> Option<Systems> {
    let mut services: IoIterator = 0;
    // The matching dictionary is the argument turned into what the call takes.
    let matching = unsafe { IOServiceMatching(IOHID_SYSTEM_CLASS.as_ptr()) };
    let found = unsafe { IOServiceGetMatchingServices(0, matching, &mut services) };
    where_the_systems_went(found, services)
}

/// The iterator a lookup filled in, or nothing where the code says it did not.
fn where_the_systems_went(code: i32, services: IoIterator) -> Option<Systems> {
    crate::capture::where_it_went(code, services).map(Systems)
}

/// The one a step handed over, and nothing for the zero an exhausted iterator
/// answers with.
fn the_next_system(service: IoObject) -> Option<System> {
    (service != 0).then_some(System(service))
}

/// A `CFDictionaryRef` that releases itself.
pub struct Parameters(CFDictionaryRef);

impl Parameters {
    /// One of the rates it holds, in nanoseconds.
    ///
    /// The two durations and not `favjit_engine::sink::Repeat`: naming what they
    /// are for is whoever links `engine` to say, and this crate does not.
    pub fn nanos(&self, name: &str) -> Option<Duration> {
        nanos(self.0, name).map(Duration::from_nanos)
    }
}

impl Drop for Parameters {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

/// The dictionary a property holds, and the value let go of where it holds
/// something else.
///
/// Data conversion, the way [`crate::cf::device_number`]'s is: a property that
/// came back as something else would otherwise be read as a dictionary, and the
/// crash would be inside CF.
fn parameters_in(value: CFTypeRef) -> Option<Parameters> {
    if value.is_null() {
        return None;
    }
    if unsafe { CFGetTypeID(value) } != unsafe { CFDictionaryGetTypeID() } {
        unsafe { CFRelease(value) };
        return None;
    }
    Some(Parameters(value))
}

fn nanos(parameters: CFDictionaryRef, name: &str) -> Option<u64> {
    // Every call here is data conversion: the dictionary is already in hand, so
    // nothing below asks the machine anything.
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
