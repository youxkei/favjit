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
// them becomes is answerable without a device, so it is
// [`favjit_core::pointer::Wanted`]'s and the values arrive already clamped.

/// A pointing device as the event system sees it.
pub struct Pointer {
    service: IOHIDServiceClientRef,
    pub vendor: Option<i64>,
    pub product: Option<i64>,
    pub name: Option<String>,
}

impl Pointer {
    /// Counts per inch, as macOS believes them.
    ///
    /// A lower number means a faster cursor: the OS divides the counts a device
    /// reports by its resolution to get a distance, so a device that claims fewer
    /// counts per inch is claiming each count is further.
    pub fn resolution(&self) -> Option<f64> {
        self.fixed(HID_POINTER_RESOLUTION)
    }

    /// The acceleration factor, from whichever property this device keeps it in.
    pub fn acceleration(&self) -> Option<f64> {
        self.fixed(self.acceleration_key())
    }

    /// Which property that is.
    ///
    /// Asked of the device, because it differs: a mouse and a pointing stick keep
    /// their factor under different names, and writing the wrong one is accepted
    /// and ignored.
    pub fn acceleration_key(&self) -> &'static str {
        if let Some(named) = self.string(HID_POINTER_ACCELERATION_TYPE) {
            // Only the two names favjit knows how to read back. Anything else is
            // left to the mouse default rather than trusted blindly, since a name
            // that cannot be verified here would fail silently.
            if named == HID_POINTER_ACCELERATION {
                return HID_POINTER_ACCELERATION;
            }
            if named == HID_MOUSE_ACCELERATION {
                return HID_MOUSE_ACCELERATION;
            }
        }
        if self.fixed(HID_POINTER_ACCELERATION).is_some() {
            return HID_POINTER_ACCELERATION;
        }
        HID_MOUSE_ACCELERATION
    }

    /// Make the cursor move this far per count, and accelerate it this much.
    ///
    /// Both together, because a resolution written on its own does not take effect
    /// until the acceleration is written after it — so the acceleration is always
    /// written, with whatever the device already had when none was asked for.
    pub fn tune(&self, dpi: Option<f64>, acceleration: Option<f64>) -> bool {
        let mut done = true;
        if let Some(dpi) = dpi {
            done &= self.set_fixed(HID_POINTER_RESOLUTION, dpi);
        }
        let factor = acceleration.or_else(|| self.acceleration());
        if let Some(factor) = factor {
            done &= self.set_fixed(self.acceleration_key(), factor);
        }
        done
    }

    fn fixed(&self, key: &str) -> Option<f64> {
        let name = CfString::new(key);
        let value = unsafe { IOHIDServiceClientCopyProperty(self.service, name.as_ref()) };
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

    fn set_fixed(&self, key: &str, value: f64) -> bool {
        let name = CfString::new(key);
        let raw = (value * 65536.0) as i32;
        let number = unsafe {
            CFNumberCreate(
                core::ptr::null(),
                kCFNumberSInt32Type,
                &raw as *const i32 as *const core::ffi::c_void,
            )
        };
        if number.is_null() {
            return false;
        }
        let set = unsafe { IOHIDServiceClientSetProperty(self.service, name.as_ref(), number) };
        unsafe { CFRelease(number) };
        set
    }

    fn string(&self, key: &str) -> Option<String> {
        let name = CfString::new(key);
        let value = unsafe { IOHIDServiceClientCopyProperty(self.service, name.as_ref()) };
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
}

/// Every pointing device the event system knows about.
///
/// Recognised by having a pointer resolution at all, rather than by matching usage
/// pages: a keyboard has none, and the property being there is the same question as
/// whether there is anything here to tune.
pub fn pointers() -> Vec<Pointer> {
    // The full client first, which is the one LinearMouse uses to set these same
    // properties; the simple client is the fallback rather than the choice, since a
    // client that cannot write is indistinguishable from a write that did nothing.
    let mut client = unsafe { IOHIDEventSystemClientCreate(core::ptr::null()) };
    if client.is_null() {
        client = unsafe { IOHIDEventSystemClientCreateSimpleClient(core::ptr::null()) };
    }
    if client.is_null() {
        return Vec::new();
    }
    let services = unsafe { IOHIDEventSystemClientCopyServices(client) };
    if services.is_null() {
        unsafe { CFRelease(client) };
        return Vec::new();
    }

    let mut found = Vec::new();
    let count = unsafe { CFArrayGetCount(services) };
    log::debug!("the event system has {count} services");
    for i in 0..count {
        let service = unsafe { CFArrayGetValueAtIndex(services, i) };
        if service.is_null() {
            continue;
        }
        let pointer = Pointer {
            service,
            vendor: None,
            product: None,
            name: None,
        };
        if pointer.resolution().is_none() {
            continue;
        }
        found.push(Pointer {
            service,
            vendor: pointer.fixed_int("VendorID"),
            product: pointer.fixed_int("ProductID"),
            name: pointer.string("Product"),
        });
    }

    // The array is released; the services it held are not, and neither is the
    // client. They are what the returned pointers point at, and this runs once for
    // the life of a process — releasing them here would leave every `Pointer`
    // dangling to save nothing.
    unsafe { CFRelease(services) };
    found
}

impl Pointer {
    /// A plain integer property, for the numbers that are not fixed point.
    fn fixed_int(&self, key: &str) -> Option<i64> {
        let name = CfString::new(key);
        let value = unsafe { IOHIDServiceClientCopyProperty(self.service, name.as_ref()) };
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
}
