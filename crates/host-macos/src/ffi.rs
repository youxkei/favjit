//! Raw declarations for the frameworks this host sits on.
//!
//! Everything `unsafe` about reaching macOS is declared here or in [`crate::cf`]
//! so the rest of the crate reads as ordinary Rust (ADR-0001).
//!
//! Constant values are not written from memory. The `kVK_` codes come from the
//! SDK's `Events.h`, the HID usages from `IOHIDUsageTables.h`, and the CGEvent
//! flag masks from compiling a program against `CGEventTypes.h` and printing
//! them — `kCGEventFlagMask*` expand to `NX_*MASK`, whose defining header is not
//! shipped in the Command Line Tools SDK.
#![allow(non_upper_case_globals)]

use std::ffi::{c_char, c_void, CStr};

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;
pub type CFRunLoopRef = *const c_void;
pub type CFRunLoopSourceRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type IOHIDDeviceRef = *const c_void;
pub type IOHIDElementRef = *const c_void;
pub type IOHIDQueueRef = *const c_void;
pub type IOHIDEventSystemClientRef = *const c_void;
pub type IOHIDServiceClientRef = *const c_void;
pub type IOHIDValueRef = *const c_void;
pub type IONotificationPortRef = *mut c_void;
pub type IoObject = u32;
pub type IoIterator = u32;
pub type MachPort = u32;

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt32Type: CFIndex = 3;
pub const kCFNumberSInt64Type: CFIndex = 4;

pub const kIOReturnSuccess: i32 = 0;

/// `IOHIDOptionsType`. Seizing is documented as stopping the system and every
/// other client from receiving the device's events, which is what suppression
/// means here.
pub const kIOHIDOptionsTypeNone: u32 = 0x00;
pub const kIOHIDOptionsTypeSeizeDevice: u32 = 0x01;

// The HID pages and usages themselves are not here. They are numbers a table
// answers with, which is `favjit_core::hid`'s — what this file is for is the C API
// those numbers are handed to.

pub type IOHIDCallback = extern "C" fn(*mut c_void, i32, *mut c_void);
pub type IoServiceMatchingCallback = extern "C" fn(*mut c_void, IoIterator);

/// The registry class every HID device on this machine is a kind of. Matching on
/// it rather than on the concrete classes — `AppleHIDTransportHIDDevice` and
/// friends — keeps the match from depending on which transport a keyboard came
/// in over.
pub const IOHID_DEVICE_CLASS: &CStr = c"IOHIDDevice";
/// `kIOMatchedNotification`. Spelled out because the constant lives in a header
/// the Command Line Tools SDK does not ship.
pub const IO_MATCHED_NOTIFICATION: &CStr = c"IOServiceMatched";
/// The registry class carrying the key repeat rates the OS itself uses. Read from
/// there rather than from this user's preferences: the preference keys are absent
/// until something writes them, and a keyboard's repeat is a property of the
/// machine's input system, not of one login session.
pub const IOHID_SYSTEM_CLASS: &CStr = c"IOHIDSystem";
/// The dictionary on that entry the rates live inside. They are not properties of
/// the entry itself, so a plain property read comes back empty — see
/// `docs/platform/macos/key-repeat.md`.
pub const HID_PARAMETERS: &str = "HIDParameters";

/// Counts per inch, as `IOFixed` — a 32-bit integer of 65536ths.
pub const HID_POINTER_RESOLUTION: &str = "HIDPointerResolution";

/// Which property holds this device's acceleration factor.
///
/// A device says so itself; [`HID_MOUSE_ACCELERATION`] is what to use when it does
/// not, and there is no way to know which without asking.
pub const HID_POINTER_ACCELERATION_TYPE: &str = "HIDPointerAccelerationType";

pub const HID_POINTER_ACCELERATION: &str = "HIDPointerAcceleration";
pub const HID_MOUSE_ACCELERATION: &str = "HIDMouseAcceleration";

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFStringCreateWithCString(a: CFAllocatorRef, s: *const c_char, e: u32) -> CFStringRef;
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    pub fn CFNumberGetTypeID() -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFNumberGetValue(n: CFTypeRef, t: CFIndex, v: *mut c_void) -> bool;
    pub fn CFStringGetCString(s: CFStringRef, b: *mut c_char, n: CFIndex, e: u32) -> bool;
    pub fn CFDictionaryGetTypeID() -> CFTypeID;
    pub fn CFDictionaryGetValue(d: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    pub fn CFArrayGetCount(a: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(a: CFArrayRef, i: CFIndex) -> CFTypeRef;
    pub fn CFNumberCreate(a: CFAllocatorRef, t: CFIndex, v: *const c_void) -> CFTypeRef;
    pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub fn CFRunLoopAddSource(rl: CFRunLoopRef, s: CFRunLoopSourceRef, mode: CFStringRef);
    pub fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, after_source: bool) -> i32;
    pub static kCFRunLoopDefaultMode: CFStringRef;
}

// No `IOHIDManager` is declared here, deliberately: it opens every device
// non-exclusively as soon as it is opened, which turns a later per-device seize
// into a second open on a device already held. That seize returns
// `kIOReturnSuccess` and suppresses nothing. Taking the `io_service_t` from the
// registry and building the device by hand makes the seize the only open there
// is. See `docs/platform/macos/input-suppression.md`.
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    pub fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    pub fn IOServiceGetMatchingServices(
        port: MachPort,
        matching: CFMutableDictionaryRef,
        it: *mut IoIterator,
    ) -> i32;
    pub fn IOServiceAddMatchingNotification(
        port: IONotificationPortRef,
        kind: *const c_char,
        matching: CFMutableDictionaryRef,
        cb: IoServiceMatchingCallback,
        ctx: *mut c_void,
        it: *mut IoIterator,
    ) -> i32;
    pub fn IONotificationPortCreate(port: MachPort) -> IONotificationPortRef;
    pub fn IONotificationPortGetRunLoopSource(p: IONotificationPortRef) -> CFRunLoopSourceRef;
    pub fn IOIteratorNext(it: IoIterator) -> IoObject;
    pub fn IOObjectRelease(o: IoObject) -> i32;
    pub fn IORegistryEntryGetRegistryEntryID(entry: IoObject, id: *mut u64) -> i32;
    pub fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> CFTypeRef;

    /// A client of the HID event system, which is where a *device's* pointer
    /// acceleration lives.
    ///
    /// Separate from the `IOHIDDevice` API used for capture: that one reads
    /// reports, this one reaches the properties macOS applies to a device's
    /// movement before an application sees it
    /// (`docs/platform/macos/pointer-acceleration.md`).
    pub fn IOHIDEventSystemClientCreate(a: CFAllocatorRef) -> IOHIDEventSystemClientRef;
    pub fn IOHIDEventSystemClientCreateSimpleClient(a: CFAllocatorRef)
        -> IOHIDEventSystemClientRef;
    pub fn IOHIDEventSystemClientCopyServices(c: IOHIDEventSystemClientRef) -> CFArrayRef;
    pub fn IOHIDServiceClientCopyProperty(s: IOHIDServiceClientRef, key: CFStringRef) -> CFTypeRef;
    pub fn IOHIDServiceClientSetProperty(
        s: IOHIDServiceClientRef,
        key: CFStringRef,
        value: CFTypeRef,
    ) -> bool;

    pub fn IOHIDDeviceCreate(a: CFAllocatorRef, service: IoObject) -> IOHIDDeviceRef;
    pub fn IOHIDDeviceOpen(d: IOHIDDeviceRef, o: u32) -> i32;
    pub fn IOHIDDeviceClose(d: IOHIDDeviceRef, o: u32) -> i32;
    pub fn IOHIDDeviceGetProperty(d: IOHIDDeviceRef, k: CFStringRef) -> CFTypeRef;
    pub fn IOHIDDeviceCopyMatchingElements(
        d: IOHIDDeviceRef,
        matching: CFTypeRef,
        o: u32,
    ) -> CFArrayRef;
    pub fn IOHIDDeviceRegisterRemovalCallback(
        d: IOHIDDeviceRef,
        cb: IOHIDCallback,
        ctx: *mut c_void,
    );
    pub fn IOHIDDeviceScheduleWithRunLoop(d: IOHIDDeviceRef, rl: CFRunLoopRef, mode: CFStringRef);

    pub fn IOHIDQueueCreate(
        a: CFAllocatorRef,
        d: IOHIDDeviceRef,
        depth: CFIndex,
        o: u32,
    ) -> IOHIDQueueRef;
    pub fn IOHIDQueueAddElement(q: IOHIDQueueRef, e: IOHIDElementRef);
    pub fn IOHIDQueueStart(q: IOHIDQueueRef);
    pub fn IOHIDQueueStop(q: IOHIDQueueRef);
    pub fn IOHIDQueueScheduleWithRunLoop(q: IOHIDQueueRef, rl: CFRunLoopRef, mode: CFStringRef);
    pub fn IOHIDQueueRegisterValueAvailableCallback(
        q: IOHIDQueueRef,
        cb: IOHIDCallback,
        ctx: *mut c_void,
    );
    pub fn IOHIDQueueCopyNextValueWithTimeout(q: IOHIDQueueRef, timeout: f64) -> IOHIDValueRef;

    pub fn IOHIDValueGetElement(v: IOHIDValueRef) -> IOHIDElementRef;
    pub fn IOHIDValueGetIntegerValue(v: IOHIDValueRef) -> CFIndex;
    /// When the HID system stamped this value, on the mach absolute clock — the
    /// same clock [`mach_absolute_time`] reads, which is what makes the
    /// difference between them a latency rather than an offset
    /// (`docs/platform/macos/input-latency.md`).
    pub fn IOHIDValueGetTimeStamp(v: IOHIDValueRef) -> u64;
    pub fn IOHIDElementGetUsagePage(e: IOHIDElementRef) -> u32;
    pub fn IOHIDElementGetUsage(e: IOHIDElementRef) -> u32;

    /// Whether this process may receive HID reports, without asking the user.
    ///
    /// [`HID_ACCESS_GRANTED`], [`HID_ACCESS_DENIED`], or 2 for a process nobody has
    /// decided about yet.
    pub fn IOHIDCheckAccess(request: u32) -> u32;

    /// Ask the user for it. Returns whether access ended up granted.
    ///
    /// `IOHIDDeviceOpen` asks on the process's behalf when this is not called, so
    /// calling it is about *when* the request happens rather than whether: at
    /// startup, where the answer can be reported, instead of inside the first
    /// device open.
    pub fn IOHIDRequestAccess(request: u32) -> bool;
}

/// `kIOHIDRequestTypeListenEvent` — receiving reports through the `IOHIDDevice`
/// API, which is how favjit captures (`docs/platform/macos/hid-input-callbacks.md`).
///
/// Not `kIOHIDRequestTypePostEvent`, which is 0: that governs `IOHIDPostEvent`, an
/// API favjit does not use, since output goes out as a device (ADR-0011).
pub const HID_REQUEST_LISTEN: u32 = 1;

pub const HID_ACCESS_GRANTED: u32 = 0;
pub const HID_ACCESS_DENIED: u32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGPreflightListenEventAccess() -> bool;
    pub fn CGPreflightPostEventAccess() -> bool;
}

// Bonjour, for telling the other machine where this one is. Declared here from
// `dns_sd.h` rather than taken from a crate: the registration is one call and a
// handle to keep, and the service is part of the system.
extern "C" {
    /// The `port` is in **network byte order** — the header says so, and a
    /// little-endian port is advertised as a different port with nothing to
    /// suggest why nothing connects.
    pub fn DNSServiceRegister(
        service: *mut DnsServiceRef,
        flags: u32,
        interface: u32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        host: *const c_char,
        port: u16,
        txt_len: u16,
        txt: *const c_void,
        callback: *const c_void,
        context: *mut c_void,
    ) -> i32;
    pub fn DNSServiceRefDeallocate(service: DnsServiceRef);
}

pub type DnsServiceRef = *mut c_void;

/// `kDNSServiceInterfaceIndexAny`: every interface the machine has.
pub const DNS_SERVICE_INTERFACE_ANY: u32 = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    /// Whether this process is trusted for Accessibility, prompting if the options
    /// say so.
    ///
    /// Worth having beside [`IOHIDCheckAccess`] because on macOS 26 an Accessibility
    /// grant can cover input monitoring as well, and this one can put a dialog in
    /// front of somebody — where the HID request cannot.
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;

    pub fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
    pub static kCFBooleanTrue: CFTypeRef;
}

/// The ratio mach ticks are turned into nanoseconds by.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MachTimebase {
    pub numer: u32,
    pub denom: u32,
}

extern "C" {
    pub fn mach_absolute_time() -> u64;
    pub fn mach_timebase_info(info: *mut MachTimebase) -> i32;
}
