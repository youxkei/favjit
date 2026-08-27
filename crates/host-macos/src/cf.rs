//! The little of Core Foundation this host needs, wrapped.

use std::ffi::{c_char, c_void, CString};

use crate::ffi::*;

/// A `CFStringRef` that releases itself.
pub struct CfString(CFStringRef);

impl CfString {
    pub fn new(s: &str) -> Self {
        let c = CString::new(s).expect("property names hold no interior nul");
        Self(unsafe {
            CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8)
        })
    }

    pub fn as_ref(&self) -> CFStringRef {
        self.0
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Read an integer property off a HID device.
pub fn device_number(device: IOHIDDeviceRef, name: &str) -> Option<i64> {
    let key = CfString::new(name);
    let value = unsafe { IOHIDDeviceGetProperty(device, key.as_ref()) };
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
    let key = CfString::new(name);
    let value = unsafe { IOHIDDeviceGetProperty(device, key.as_ref()) };
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
