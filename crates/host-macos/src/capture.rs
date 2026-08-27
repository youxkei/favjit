//! Capturing key events, with the keyboard they came from, and taking the
//! keyboard away from everyone else while we do.
//!
//! Devices come from the IOKit registry, not from an `IOHIDManager`. That is
//! what makes suppression possible at all: a manager opens every device
//! non-exclusively, so a later seize is a second open on a device already held
//! and takes hold of nothing while still reporting success. Creating the device
//! from its `io_service_t` makes the seize the only open there is. The measured
//! difference is in `docs/platform/macos/input-suppression.md`.
//!
//! Values then arrive from an `IOHIDQueue` per device rather than from a
//! manager's callback, and the callbacks land on a CFRunLoop, so the run loop
//! gets a thread of its own and everything it produces is funnelled into one
//! channel. That is where ADR-0006 puts unavoidable concurrency: inside the
//! host, never in `core`.

use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use favjit_core::{
    Buttons, DeviceId, DeviceInfo, DeviceMatch, EventKind, HostEvent, Instant, Key, PointerReport,
};
use log::{debug, info, trace, warn};

use favjit_core::hid::{self, usage};

use crate::{cf, ffi::*};

/// How often the probe descriptor is looked at.
///
/// A poll rather than a run-loop source on the descriptor: adding a
/// `CFFileDescriptor` to the loop would put the watchdog's liveness check behind
/// the same loop it is meant to be testing, so a wedge there would silence the
/// probe instead of answering it. Polling from the loop's own turn has the same
/// blind spot, but the watchdog's timeout covers it — a probe that produces no
/// heartbeat is the verdict either way.
const PROBE_POLL: f64 = 0.05;

/// What to watch, and whether to take it exclusively.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Keyboards to leave entirely alone — not announced, not converted, not
    /// seized.
    pub ignore: Vec<DeviceMatch>,
    /// Stop the OS delivering these keyboards' events to anyone else, so the
    /// converted keystroke replaces the original instead of joining it.
    pub suppress: bool,
    /// Leave the Mac's own keyboard entirely alone.
    ///
    /// Not expressible through `ignore`, which matches on vendor and product ids
    /// the internal keyboard does not have. It exists because the built-in
    /// keyboard is the one a person recovers with: while suppression is being
    /// exercised on purpose, keeping it out means the worst outcome is a dead
    /// external keyboard rather than a machine that cannot be typed at.
    pub skip_built_in: bool,
    /// Queue every element a keyboard has, not only the ones the tables can name.
    ///
    /// For finding a key that reports somewhere unexpected — `Fn` did, on a page
    /// no header names. Left off for a run that converts, because a queue that
    /// admits a page it has no names for delivers into the interactive path at
    /// whatever rate the device chooses, pointer data included. A mode that runs
    /// no conversion has nothing to slow down and everything to discover.
    pub watch_everything: bool,
}

/// A key press the layout has no name for, reported so the gap can be measured
/// rather than guessed at.
///
/// The page is part of it: a usage number alone cannot be looked up, and a key
/// missing from the tables is as likely to be on a page they do not cover — `Fn`
/// is, on Apple's top case page — as at an unlisted usage on a page they do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownUsage {
    pub device: DeviceId,
    pub page: u32,
    pub usage: u32,
}

/// What asking for exclusive access to one keyboard returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seizure {
    pub device: DeviceId,
    /// The raw `IOReturn`. Reported rather than reduced to a bool because which
    /// failure it is decides what to do about it: `kIOReturnNotPrivileged`
    /// (`0xE00002C1`) says to run with privilege, `kIOReturnExclusiveAccess`
    /// (`0xE00002C5`) says something else already holds the keyboard.
    pub code: i32,
    /// Whether exclusivity was asked for.
    ///
    /// Carried alongside the code because success means two different things: a
    /// plain open shares the keyboard, and a report that called that "held
    /// exclusively" would say the physical keystrokes were being suppressed when
    /// they were still arriving.
    pub exclusive: bool,
}

/// What the threads beside the run loop send up.
///
/// One channel for all of them, because the order the loop sees is the order things
/// happened (ADR-0006): a second way in would let the end of a link overtake the
/// keystrokes that arrived over it.
pub enum Captured {
    Event(HostEvent),
    /// A loop the run handed over to be turned alongside its own has returned.
    ///
    /// Sent from that thread rather than left in a flag the loop polls, so that it
    /// lands behind everything that loop delivered: a flag read at the top of a
    /// wait would end the run with those events still in the channel.
    AlongsideStopped,
    /// A page and usage [`usage::named`] has no key for.
    Unknown(UnknownUsage),
    Seized(Seizure),
    /// Nanoseconds between the HID system stamping a value and this thread
    /// reaching it.
    ///
    /// Sent up rather than logged where it is measured: a write to stderr per
    /// keystroke sits in the interactive path, and a latency measurement that
    /// adds latency measures itself. Sent per value rather than summarised here
    /// for the same reason — the summary is arithmetic, and it belongs where
    /// nothing is waiting on it.
    Delay(u64),
}

/// Devices currently held exclusively, by pointer.
///
/// Shared with the owner rather than kept on the capture thread, because the run
/// loop never returns and so can never release anything: without a handle the
/// only release left is the process dying. That does work — the platform ends a
/// seize with the process (`docs/platform/macos/input-suppression.md`) — but it
/// is the watchdog's path, and an ordinary exit should not need a kill to give
/// the keyboard back.
pub type Held = Arc<Mutex<Vec<usize>>>;

/// One keyboard we are reading.
struct Watched {
    id: DeviceId,
    /// The IOKit registry entry id, which is what identifies the keyboard.
    /// `IOHIDDeviceCreate` hands back a fresh object each time it is called, so
    /// two enumerations of the same keyboard produce two unequal
    /// `IOHIDDeviceRef`s and a pointer cannot tell them apart — which would
    /// leave one keyboard captured twice and every keystroke converted twice.
    registry: u64,
    device: IOHIDDeviceRef,
    queue: IOHIDQueueRef,
    /// The pointer report being assembled from this device's element values, and
    /// the buttons it currently holds.
    pointer: Pointing,
}

/// A device's pointer, between the element values that describe it.
///
/// The queue hands over one element value at a time, so the X and the Y of one
/// movement arrive separately and have to be put back together: the OS applies
/// its acceleration per report, so a diagonal relayed as one report per axis is
/// accelerated as two short movements and falls short of the same motion of the
/// thumb (`docs/platform/macos/virtual-hid-device.md`).
///
/// The buttons persist between reports and the deltas do not, which is the
/// difference between state and a movement: a queue delivers a value when it
/// changes, so a button held down through a movement is reported once and has to
/// be remembered, while an axis that says nothing has not moved.
#[derive(Debug, Default, Clone, Copy)]
struct Pointing {
    buttons: Buttons,
    pending: PointerReport,
    /// The mach stamp the pending values share. Values from one HID report carry
    /// the same one, which is what says where a report ends.
    stamp: u64,
    /// Whether anything has been put into `pending` since the last flush.
    open: bool,
}

impl Pointing {
    /// Take what has been assembled, if anything.
    fn take(&mut self) -> Option<PointerReport> {
        if !self.open {
            return None;
        }
        let report = PointerReport {
            buttons: self.buttons,
            ..self.pending
        };
        self.pending = PointerReport::default();
        self.open = false;
        Some(report)
    }
}

/// The host's clock, shared by everything that stamps an event.
///
/// One base, copied to whoever needs it, rather than a clock per stamping site:
/// two bases would put the capture thread's timestamps and the loop's timer wake
/// -ups on two different origins, and `core` compares them against each other.
///
/// Arrival time rather than the timestamp on the HID value itself: that one is in
/// mach units and needs `mach_timebase_info` to convert, and it has not been
/// established here what it measures. Swapping it in later changes nothing above
/// the boundary, because ADR-0010 has `core` read time only off the event.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    base: std::time::Instant,
}

impl Clock {
    pub(crate) fn start() -> Self {
        Self {
            base: std::time::Instant::now(),
        }
    }

    pub fn now(&self) -> Instant {
        Instant::from_nanos(self.base.elapsed().as_nanos() as u64)
    }
}

/// How long ago, in nanoseconds, a HID value's stamp was.
///
/// The mach clock, not [`Clock`], and only because both ends of this one
/// measurement have to be on it: a value's stamp is in mach ticks, and there is
/// no conversion from those to the `std::time::Instant` the events carry.
///
/// Saturating, so a stamp from a clock this does not understand reads as zero
/// rather than as an enormous latency.
fn since_stamp(stamp: u64) -> u64 {
    let mut info = MachTimebase::default();
    unsafe { mach_timebase_info(&mut info) };
    if info.denom == 0 {
        return 0;
    }
    let ticks = unsafe { mach_absolute_time() }.saturating_sub(stamp) as u128;
    (ticks * info.numer as u128 / info.denom as u128) as u64
}

/// Per-callback state. Lives on the capture thread and is reached from the C
/// callbacks through a raw pointer, which is sound because every callback fires
/// on that one run loop.
struct Capture {
    out: Sender<Captured>,
    clock: Clock,
    config: Config,
    held: Held,
    /// A list rather than a map keyed three ways: the value callback is handed a
    /// queue, the removal callback a device, and matching gives a registry id,
    /// and there are only ever a handful of keyboards to scan.
    watched: Vec<Watched>,
    next_id: u64,
}

impl Capture {
    fn send(&self, kind: EventKind) {
        let _ = self
            .out
            .send(Captured::Event(HostEvent::new(self.clock.now(), kind)));
    }

    /// Open, seize, and start reading one device. Returns whether the device was
    /// taken on, so the caller knows whether to release it.
    fn adopt(&mut self, device: IOHIDDeviceRef, registry: u64) -> bool {
        if self.watched.iter().any(|w| w.registry == registry) {
            return false;
        }
        let info = describe(device, DeviceId(self.next_id));
        if !is_keyboard(device)
            || (self.config.skip_built_in && info.is_built_in)
            || self.config.ignore.iter().any(|m| m.matches(&info))
        {
            trace!(
                "leaving alone: built_in={} vendor={:?} product={:?}",
                info.is_built_in,
                info.vendor_id,
                info.product_id
            );
            return false;
        }
        let key = device as usize;

        let options = if self.config.suppress {
            kIOHIDOptionsTypeSeizeDevice
        } else {
            kIOHIDOptionsTypeNone
        };
        let code = unsafe { IOHIDDeviceOpen(device, options) };
        // Reported whether or not exclusivity was asked for: a plain open can
        // fail too, and a keyboard that silently never appears is the hardest
        // thing here to diagnose.
        let _ = self.out.send(Captured::Seized(Seizure {
            device: info.id,
            code,
            exclusive: self.config.suppress,
        }));
        if code != kIOReturnSuccess {
            warn!(
                "could not open vendor={:?} product={:?}: {code:#010x}",
                info.vendor_id, info.product_id
            );
            return false;
        }
        if self.config.suppress {
            self.held.lock().unwrap().push(key);
        }

        let Some(queue) = start_queue(device, self.config.watch_everything) else {
            unsafe { IOHIDDeviceClose(device, options) };
            self.held.lock().unwrap().retain(|&h| h != key);
            return false;
        };

        self.next_id += 1;
        self.watched.push(Watched {
            id: info.id,
            registry,
            device,
            queue,
            pointer: Pointing::default(),
        });
        unsafe {
            IOHIDQueueRegisterValueAvailableCallback(queue, on_value, queue as *mut c_void);
            IOHIDQueueScheduleWithRunLoop(queue, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
            IOHIDQueueStart(queue);
            IOHIDDeviceRegisterRemovalCallback(device, on_removed, device as *mut c_void);
            IOHIDDeviceScheduleWithRunLoop(device, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
        }
        info!(
            "watching device {} built_in={} vendor={:?} product={:?}{}",
            info.id.0,
            info.is_built_in,
            info.vendor_id,
            info.product_id,
            if self.config.suppress {
                ", held exclusively"
            } else {
                ""
            }
        );
        self.send(EventKind::DeviceAttached(info));
        true
    }

    fn drop_device(&mut self, device: IOHIDDeviceRef) {
        let Some(at) = self.watched.iter().position(|w| w.device == device) else {
            return;
        };
        let watched = self.watched.remove(at);
        unsafe {
            IOHIDQueueStop(watched.queue);
            CFRelease(watched.queue);
        }
        let key = watched.device as usize;
        let mut held = self.held.lock().unwrap();
        if let Some(at) = held.iter().position(|&h| h == key) {
            held.remove(at);
            // Close a keyboard that is already gone: the seize is state held
            // against a device that may come back, and leaving it behind would
            // make a reconnect look permanently taken.
            unsafe { IOHIDDeviceClose(watched.device, kIOHIDOptionsTypeSeizeDevice) };
        }
        drop(held);
        unsafe { CFRelease(watched.device) };
        debug!("device {} gone", watched.id.0);
        self.send(EventKind::DeviceDetached(watched.id));
    }
}

/// Build a queue over the key elements of a device.
///
/// Which elements those are is [`usage::watched`]'s to say, so the pages this
/// host reads are named in one place rather than split between the filter here
/// and the naming there — a page admitted here and unnamed there is a device
/// whose events arrive and go nowhere.
fn start_queue(device: IOHIDDeviceRef, everything: bool) -> Option<IOHIDQueueRef> {
    unsafe {
        let elements = IOHIDDeviceCopyMatchingElements(device, std::ptr::null(), 0);
        if elements.is_null() {
            return None;
        }
        // A queue drops values when it fills, and the value most costly to lose
        // is a key-up: the modifier it belongs to stays down inside every
        // application. Depth is bounded memory and nothing else, so it is sized
        // past any burst a person can type rather than to a number that has to be
        // argued for. Karabiner uses the same figure.
        let queue = IOHIDQueueCreate(std::ptr::null(), device, 1024, 0);
        if queue.is_null() {
            CFRelease(elements);
            return None;
        }
        let mut added = 0;
        for i in 0..CFArrayGetCount(elements) {
            let element = CFArrayGetValueAtIndex(elements, i);
            if !element.is_null()
                && (everything
                    || usage::watched(
                        IOHIDElementGetUsagePage(element),
                        IOHIDElementGetUsage(element),
                    ))
            {
                IOHIDQueueAddElement(queue, element);
                added += 1;
            }
        }
        CFRelease(elements);
        if added == 0 {
            CFRelease(queue);
            return None;
        }
        Some(queue)
    }
}

/// Is this device one whose key presses we want?
fn is_keyboard(device: IOHIDDeviceRef) -> bool {
    cf::device_number(device, "PrimaryUsagePage") == Some(hid::page::GENERIC_DESKTOP as i64)
        && cf::device_number(device, "PrimaryUsage") == Some(hid::KEYBOARD_COLLECTION as i64)
}

/// What `core` gets told about a keyboard.
///
/// `is_built_in` reads `Transport`, not a `BuiltIn` property: no device on this
/// machine has one, and the internal keyboard has no vendor or product id either
/// — `Transport = "FIFO"` and its product string are all that set it apart. See
/// `docs/platform/macos/hid-device-enumeration.md`.
/// Read a keyboard's identity, and decide whether it is the Mac's own.
///
/// Two signals, either of which is enough, because the whole layout hangs on this
/// answer and each one alone is a single point of failure. `Transport == "FIFO"`
/// is the SPI bus the internal keyboard sits on, and it is the only signal that
/// works at all for a device with no vendor or product id. The product string is
/// the second: an internal keyboard names itself `Apple Internal …`. Karabiner
/// carries the same pair.
///
/// Getting it wrong is not a missing key but a wrong keyboard: an internal
/// keyboard read as external takes the raw-JIS remaps instead of the Dudrack
/// layers.
fn describe(device: IOHIDDeviceRef, id: DeviceId) -> DeviceInfo {
    let transport = cf::device_string(device, "Transport");
    let product = cf::device_string(device, "Product");
    DeviceInfo {
        id,
        is_built_in: transport.as_deref() == Some("FIFO")
            || product
                .as_deref()
                .is_some_and(|p| p.starts_with("Apple Internal ")),
        vendor_id: cf::device_number(device, "VendorID").and_then(|n| u16::try_from(n).ok()),
        product_id: cf::device_number(device, "ProductID").and_then(|n| u16::try_from(n).ok()),
    }
}

/// The one `Capture` on the capture thread.
///
/// A static rather than a context pointer threaded through every callback: the
/// device-matching callback and the per-device removal callback each want a
/// different context of their own, so one of them would have to find its way
/// back here regardless.
static CAPTURE: Mutex<usize> = Mutex::new(0);

fn with_capture(f: impl FnOnce(&mut Capture)) {
    let raw = *CAPTURE.lock().unwrap();
    if raw != 0 {
        f(unsafe { &mut *(raw as *mut Capture) });
    }
}

extern "C" fn on_value(ctx: *mut c_void, _r: i32, _sender: *mut c_void) {
    let queue = ctx as IOHIDQueueRef;
    with_capture(|capture| {
        let Some(index) = capture.watched.iter().position(|w| w.queue == queue) else {
            return;
        };
        let id = capture.watched[index].id;

        // The callback says a value is available, not what it is, so the queue is
        // drained here at a zero timeout until it runs dry.
        loop {
            let value = unsafe { IOHIDQueueCopyNextValueWithTimeout(queue, 0.0) };
            if value.is_null() {
                // Draining is what ends a pointer report: the values of one are
                // all in the queue together, so running dry is as certain a
                // boundary as the stamp changing.
                if let Some(report) = capture.watched[index].pointer.take() {
                    capture.send(EventKind::Pointer { device: id, report });
                }
                return;
            }
            let element = unsafe { IOHIDValueGetElement(value) };
            if element.is_null() {
                unsafe { CFRelease(value) };
                continue;
            }
            let usage = unsafe { IOHIDElementGetUsage(element) };
            let page = unsafe { IOHIDElementGetUsagePage(element) };
            let stamp = unsafe { IOHIDValueGetTimeStamp(value) };
            let delay = since_stamp(stamp);
            let integer = unsafe { IOHIDValueGetIntegerValue(value) };
            unsafe { CFRelease(value) };

            // The pointer first, and before anything looks at the value: a delta
            // is any number at all, where a key is only ever 0 or 1.
            if usage::pointer(page, usage) {
                let pointer = &mut capture.watched[index].pointer;
                if pointer.open && stamp != pointer.stamp {
                    if let Some(report) = pointer.take() {
                        capture.send(EventKind::Pointer { device: id, report });
                    }
                }
                let pointer = &mut capture.watched[index].pointer;
                pointer.stamp = stamp;
                pointer.open = true;
                if page == hid::page::GENERIC_DESKTOP {
                    match usage {
                        usage::POINTER_X => pointer.pending.dx = integer as i32,
                        usage::POINTER_Y => pointer.pending.dy = integer as i32,
                        usage::POINTER_WHEEL => pointer.pending.vertical_wheel = integer as i32,
                        // Unreachable while `usage::pointer` admits only those
                        // three, and left rather than made a panic: a fourth
                        // usage admitted there should relay nothing until it is
                        // given a field, not bring the process down holding a
                        // seized keyboard.
                        _ => {}
                    }
                } else {
                    // A button's usage number is which button it is, counting from
                    // one, and its value says whether it is down.
                    pointer.buttons = pointer.buttons.set(usage as u8, integer != 0);
                }
                continue;
            }

            // A keyboard reports elements alongside real presses whose value is
            // neither a press nor a release; those are not key events.
            let down = match integer {
                0 => false,
                1 => true,
                _ => continue,
            };
            let _ = capture.out.send(Captured::Delay(delay));
            match usage::named(page, usage) {
                Some(key) => capture.send(if down {
                    EventKind::KeyDown { device: id, key }
                } else {
                    EventKind::KeyUp { device: id, key }
                }),
                None if down => {
                    let _ = capture.out.send(Captured::Unknown(UnknownUsage {
                        device: id,
                        page,
                        usage,
                    }));
                }
                None => {}
            }
        }
    });
}

extern "C" fn on_removed(ctx: *mut c_void, _r: i32, _sender: *mut c_void) {
    let device = ctx as IOHIDDeviceRef;
    with_capture(|capture| capture.drop_device(device));
}

extern "C" fn on_matched(_ctx: *mut c_void, iterator: IoIterator) {
    // The iterator has to be drained even for devices we do not want, or the
    // notification never fires again.
    loop {
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            return;
        }
        let mut registry: u64 = 0;
        let known = unsafe { IORegistryEntryGetRegistryEntryID(service, &mut registry) }
            == kIOReturnSuccess;
        let device = unsafe { IOHIDDeviceCreate(std::ptr::null(), service) };
        unsafe { IOObjectRelease(service) };
        if device.is_null() {
            continue;
        }
        // Without an id there is no way to tell this keyboard from one already
        // being read, and reading one twice converts every keystroke twice.
        let mut adopted = false;
        if known {
            with_capture(|capture| adopted = capture.adopt(device, registry));
        }
        if !adopted {
            unsafe { CFRelease(device) };
        }
    }
}

/// Start capturing. The returned channel carries everything the run loop sees,
/// and the [`Held`] handle is how whatever owns this releases the keyboards.
///
/// Handed out raw as well as behind [`crate::MacOsHost`], so a caller can watch
/// the stream without running a conversion over it — which is how the usage of a
/// key the tables cannot name gets established without printing what was typed.
/// Take the keyboards and start reading them, into a stream that already exists.
///
/// The channel comes from the caller so that whatever else observes input — the
/// link from the other machine — puts its events into this same stream, and can do
/// so before any keyboard has been taken. A second stream would need the loop to
/// choose between them, and that choice is the reordering ADR-0006 keeps out of
/// `core`.
pub fn capture(config: Config, out: Sender<Captured>) -> (Held, Clock) {
    let held: Held = Arc::new(Mutex::new(Vec::new()));
    let theirs = Arc::clone(&held);
    // Started here rather than on the capture thread, so the caller reads the
    // same clock the events are stamped with.
    let clock = Clock::start();
    std::thread::Builder::new()
        .name("favjit-hid-capture".into())
        .spawn(move || {
            let capture = Box::into_raw(Box::new(Capture {
                out,
                clock,
                config,
                held: theirs,
                watched: Vec::new(),
                next_id: 0,
            }));
            *CAPTURE.lock().unwrap() = capture as usize;
            unsafe {
                // Hot-plug first, then the devices already there: registering
                // the notification before enumerating means a keyboard that
                // arrives between the two is seen twice rather than missed, and
                // `adopt` ignores a device it already has.
                let port = IONotificationPortCreate(0);
                let mut hotplug: IoIterator = 0;
                IOServiceAddMatchingNotification(
                    port,
                    IO_MATCHED_NOTIFICATION.as_ptr(),
                    IOServiceMatching(IOHID_DEVICE_CLASS.as_ptr()),
                    on_matched,
                    std::ptr::null_mut(),
                    &mut hotplug,
                );
                CFRunLoopAddSource(
                    CFRunLoopGetCurrent(),
                    IONotificationPortGetRunLoopSource(port),
                    kCFRunLoopDefaultMode,
                );
                on_matched(std::ptr::null_mut(), hotplug);

                let mut existing: IoIterator = 0;
                if IOServiceGetMatchingServices(
                    0,
                    IOServiceMatching(IOHID_DEVICE_CLASS.as_ptr()),
                    &mut existing,
                ) == kIOReturnSuccess
                {
                    on_matched(std::ptr::null_mut(), existing);
                    IOObjectRelease(existing);
                }

                // Turned by hand rather than handed to `CFRunLoopRun`, so the
                // probe descriptor gets looked at between turns. Letting the
                // run loop own the thread outright would leave nothing to check
                // it with.
                let supervisor = crate::supervisor::Supervisor::from_env();
                loop {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, PROBE_POLL, false);
                    if supervisor.probe_fd().is_some() {
                        for _ in 0..supervisor.take_probes() {
                            with_capture(|capture| capture.send(EventKind::Probe));
                        }
                    }
                }
            }
        })
        .expect("the capture thread is the first thing this host does");
    (held, clock)
}

/// Give back every keyboard held exclusively.
pub fn release(held: &Held) {
    for key in held.lock().unwrap().drain(..) {
        unsafe { IOHIDDeviceClose(key as IOHIDDeviceRef, kIOHIDOptionsTypeSeizeDevice) };
    }
}

/// Keys the layout mentions that this host cannot recognise yet.
pub fn unmapped_keys() -> &'static [Key] {
    usage::UNMAPPED
}
