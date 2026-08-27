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
//! host, never in `engine`.
//!
//! Naming what a page and a usage mean — a key, a pointer axis — is `engine`'s;
//! this file reports the numbers a device gave it and nothing it decided about
//! them (ADR-0006). Deciding which keyboard to take is `engine`'s too:
//! [`on_matched`] reports every keyboard-class device this run could name and
//! takes none of them until asked to, one call at a time, from whichever
//! thread is asking. Every element a taken device has is queued, since which
//! ones are worth reading is that same naming table's question.

use std::ffi::c_void;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use favjit_host::{DeviceId, EventKind, Handed, HostEvent, Instant};

use crate::{cf, ffi::*};

/// One call `engine` asks this thread to make on a device it has already been
/// told about, answered through `reply`.
///
/// A message rather than a direct call, because the calls IOKit ties to this
/// thread's run loop — opening a device, scheduling its queue — have to run on
/// it, and `engine`'s own loop is a different thread's.
pub(crate) enum Request {
    Take {
        device: DeviceId,
        exclusive: bool,
        reply: Sender<i32>,
    },
    Read {
        device: DeviceId,
        reply: Sender<bool>,
    },
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

/// Devices currently held exclusively.
///
/// Shared with the owner rather than kept on the capture thread, because the run
/// loop never returns and so can never release anything: without a handle the
/// only release left is the process dying. That does work — the platform ends a
/// seize with the process (`docs/platform/macos/input-suppression.md`) — but it
/// is the watchdog's path, and an ordinary exit should not need a kill to give
/// the keyboard back.
pub type Held = Arc<Mutex<Vec<HeldDevice>>>;

/// The identity `engine` knows and the pointer the one close call needs.
#[derive(Clone, Copy)]
pub struct HeldDevice {
    pub id: DeviceId,
    device: usize,
}

/// A keyboard IOKit reported, not yet asked for.
///
/// Keyed by the number the loop above gave it and not by the registry's own id
/// for it, because recognising the same keyboard twice is that loop's and the
/// number is what everything after it names the device by.
struct Pending {
    id: DeviceId,
    device: IOHIDDeviceRef,
}

/// A keyboard taken and read.
struct Watched {
    id: DeviceId,
    device: IOHIDDeviceRef,
    queue: IOHIDQueueRef,
}

/// A queue IOKit has said has values on it.
///
/// A record of its own rather than the handle alone, so that what is put down is
/// something this file's declarations name: what a callback can do is write down
/// which queue it was told about, and the values come off it in the run's own
/// drain.
struct Ready {
    queue: IOHIDQueueRef,
}

/// A device IOKit has said has gone, likewise.
struct Departed {
    device: IOHIDDeviceRef,
}

/// The host's clock, shared by everything that stamps an event.
///
/// One base, copied to whoever needs it, rather than a clock per stamping site:
/// two bases would put the capture thread's timestamps and the loop's timer wake
/// -ups on two different origins, and `engine` compares them against each other.
///
/// Arrival time rather than the timestamp on the HID value itself: that one is in
/// mach units and needs `mach_timebase_info` to convert, and it has not been
/// established here what it measures. Swapping it in later changes nothing above
/// the boundary, because ADR-0010 has `engine` read time only off the event.
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
        Instant {
            nanos: self.base.elapsed().as_nanos() as u64,
        }
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
    nanos_of(timebase(), mach_ticks().saturating_sub(stamp))
}

/// Mach ticks as nanoseconds, and zero where the machine named no ratio to
/// convert them with.
///
/// Zero rather than a guess at the ratio: every Apple machine so far reports one,
/// and a made-up one would be a latency figure that reads as measured.
fn nanos_of(info: MachTimebase, ticks: u64) -> u64 {
    match info.denom == 0 {
        true => 0,
        false => (ticks as u128 * info.numer as u128 / info.denom as u128) as u64,
    }
}

/// A reference there is one of, out of a pointer a call may answer null with.
fn there_is_one<T>(value: *const T) -> Option<*const T> {
    (!value.is_null()).then_some(value)
}

/// Keep an iterator devices come off, and say whether the machine gave one.
fn came_off(iterators: &mut Vec<IoIterator>, found: Option<IoIterator>) -> bool {
    iterators.extend(found);
    found.is_some()
}

/// The iterator the next finding is looked for on, dropping the one that
/// answered with nothing, and none once every one of them has.
fn the_iterator(iterators: &mut Vec<IoIterator>, exhausted: bool) -> IoIterator {
    let _ = (exhausted && !iterators.is_empty()).then(|| iterators.remove(0));
    match iterators.first() {
        Some(iterator) => *iterator,
        None => 0,
    }
}

/// A service there is one of, as the run holds it.
fn a_service(service: IoObject) -> Option<Handed> {
    (service != 0).then_some(Handed(service as u64))
}

/// The service behind what the run is holding.
fn the_service(found: Handed) -> IoObject {
    found.0 as IoObject
}

/// A device there is one of, as the run holds it.
fn a_device(device: IOHIDDeviceRef) -> Option<Handed> {
    (!device.is_null()).then_some(Handed(device as u64))
}

/// The device behind what the run is holding.
fn the_device(held: Handed) -> IOHIDDeviceRef {
    held.0 as IOHIDDeviceRef
}

/// A queue there is one of, as the run holds it.
fn a_queue(queue: IOHIDQueueRef) -> Option<Handed> {
    (!queue.is_null()).then_some(Handed(queue as u64))
}

/// The first queue that said it had values, and none once none has.
fn the_front(ready: &[Ready]) -> Option<Handed> {
    ready.first().and_then(|ready| a_queue(ready.queue))
}

/// Let go of it as one with values on it, so IOKit is asked again the next time
/// it has some.
fn no_longer_ready(ready: &mut Vec<Ready>, from: Handed) {
    let at = ready.iter().position(|one| one.queue == the_queue(from));
    let _ = at.map(|at| ready.remove(at));
}

/// The first device that said it had gone, and none once none has.
fn the_first_gone(gone: &[Departed]) -> Option<Handed> {
    gone.first().and_then(|gone| a_device(gone.device))
}

/// The number the run gave the device that queue is over.
fn named(watched: &[Watched], queue: Handed) -> Option<DeviceId> {
    watched
        .iter()
        .find(|w| w.queue == the_queue(queue))
        .map(|w| w.id)
}

/// The queue over the device the run is holding, and none where nothing is
/// reading it.
fn queue_over(watched: &[Watched], device: Handed) -> Option<Handed> {
    watched
        .iter()
        .find(|w| w.device == the_device(device))
        .and_then(|w| a_queue(w.queue))
}

/// One value off a queue, read as the numbers it carries, and none once the
/// queue has run dry.
///
/// Read and let go of with Core Foundation's and IOKit's own accessors, which
/// ask the machine nothing: the element, the page, the usage, the stamp and the
/// integer are all held in the value already. A value this crate could not read
/// is answered as none, which ends the drain — a queue that has to run dry for
/// the callback to fire again is one this comes back round to on the next turn.
fn a_value(value: IOHIDValueRef, device: Option<DeviceId>) -> Option<EventKind> {
    let read = there_is_one(value).and_then(read_value);
    let _ = there_is_one(value).map(let_go);
    read.zip(device).map(|(read, device)| EventKind::HidValue {
        device,
        page: read.page,
        usage: read.usage,
        stamp: read.stamp,
        value: read.value,
    })
}

/// Take a device that has gone off every list this thread keeps it on, and say
/// what the run called it.
///
/// The seized list too, because a seize is state held against a device that may
/// come back: one left behind would make a reconnect look permanently taken.
fn gone_from(
    watched: &mut Vec<Watched>,
    pending: &mut Vec<Pending>,
    gone: &mut Vec<Departed>,
    held: &Held,
    device: Handed,
) -> Option<DeviceId> {
    let key = the_device(device);
    let read = watched.iter().position(|w| w.device == key);
    let was = read.map(|at| watched.remove(at)).map(|watched| watched.id);
    let waiting = pending.iter().position(|p| p.device == key);
    let was = was.or_else(|| pending.get(waiting?).map(|p| p.id));
    let _ = waiting.map(|at| pending.remove(at));
    let seized = held
        .lock()
        .unwrap()
        .iter()
        .position(|h| h.device == key as usize);
    let _ = seized.map(|at| held.lock().unwrap().remove(at));
    let departed = gone.iter().position(|one| one.device == key);
    let _ = departed.map(|at| gone.remove(at));
    was
}

/// The queue behind what the run is holding.
fn the_queue(held: Handed) -> IOHIDQueueRef {
    held.0 as IOHIDQueueRef
}

/// An array there is one of, as the run holds it.
fn an_array(array: CFArrayRef) -> Option<Handed> {
    (!array.is_null()).then_some(Handed(array as u64))
}

/// The array behind what the run is holding.
fn the_array(held: Handed) -> CFArrayRef {
    held.0 as CFArrayRef
}

/// What the run asked of this thread, under the machine's own name for the
/// device it named — and nothing where it asked about one this thread has not
/// got waiting.
fn asked_for(
    answering: &Option<Request>,
    pending: &[Pending],
) -> Option<favjit_host::capture::Ask> {
    match answering {
        Some(Request::Take {
            device, exclusive, ..
        }) => waiting(pending, *device).map(|device| favjit_host::capture::Ask::Take {
            device,
            exclusive: *exclusive,
        }),
        Some(Request::Read { device, .. }) => {
            waiting(pending, *device).map(|device| favjit_host::capture::Ask::Read { device })
        }
        None => None,
    }
}

/// The device waiting under the number the run gave it.
fn waiting(pending: &[Pending], id: DeviceId) -> Option<Handed> {
    pending
        .iter()
        .find(|p| p.id == id)
        .map(|p| Handed(p.device as u64))
}

/// Send the answer back to whoever is waiting on it, spelled the way that ask
/// takes it.
fn answered(answering: &Option<Request>, code: i32) {
    match answering {
        Some(Request::Take { reply, .. }) => {
            let _ = reply.send(code);
        }
        Some(Request::Read { reply, .. }) => {
            let _ = reply.send(went(code));
        }
        None => {}
    }
}

/// Keep a device this run took away from everything else, so that an ordinary
/// exit can give it back.
///
/// Only one the open says it took, and only where it was asked for
/// exclusively: the others are open and nothing has to give them back.
fn seized(held: &Held, pending: &[Pending], device: Handed, code: i32, exclusive: bool) {
    let taken = (went(code) && exclusive)
        .then_some(device)
        .and_then(|device| numbered(pending, device));
    let _ = taken.map(|id| {
        held.lock().unwrap().push(HeldDevice {
            id,
            device: the_device(device) as usize,
        })
    });
}

/// The number the run gave the device the machine names this.
fn numbered(pending: &[Pending], device: Handed) -> Option<DeviceId> {
    pending
        .iter()
        .find(|p| p.device == the_device(device))
        .map(|p| p.id)
}

/// How many values a Core Foundation array holds.
fn how_many(array: CFArrayRef) -> usize {
    unsafe { CFArrayGetCount(array) }.max(0) as usize
}

/// The value at that place, which is null where the array holds nothing there.
///
/// Core Foundation's own accessor, which asks the machine nothing: what it
/// answers was in the array before the call was made.
fn at_place(array: CFArrayRef, at: usize) -> CFTypeRef {
    unsafe { CFArrayGetValueAtIndex(array, at as isize) }
}

/// Keep the queue against the device it is over, and take that device off the
/// list of ones waiting to be read.
fn watching(watched: &mut Vec<Watched>, pending: &mut Vec<Pending>, queue: Handed, device: Handed) {
    let at = pending.iter().position(|p| p.device == the_device(device));
    let _ = at.map(|at| pending.remove(at)).map(|pending| {
        watched.push(Watched {
            id: pending.id,
            device: pending.device,
            queue: the_queue(queue),
        })
    });
}

/// The list of devices waiting without the one under this number, whose
/// reference this file hands back on the way out.
///
/// A conversion because nothing in it asks the machine anything: `CFRelease`
/// gives back a reference this file took and touches no device.
fn forgotten(pending: &mut Vec<Pending>, device: DeviceId) {
    let at = pending.iter().position(|p| p.id == device);
    let _ = at
        .map(|at| pending.remove(at))
        .map(|pending| let_go(pending.device));
}

/// One HID value, as the numbers it carries.
///
/// Reading a value takes several of IOKit's own calls and not one of them asks
/// the machine anything, which is [`crate::cf`]'s exception on the same terms:
/// the element, the page, the usage, the stamp and the integer are all held in
/// the value already.
struct Read {
    page: u32,
    usage: u32,
    stamp: u64,
    value: i64,
}

fn read_value(value: IOHIDValueRef) -> Option<Read> {
    there_is_one(unsafe { IOHIDValueGetElement(value) }).map(|element| Read {
        page: unsafe { IOHIDElementGetUsagePage(element) },
        usage: unsafe { IOHIDElementGetUsage(element) },
        stamp: unsafe { IOHIDValueGetTimeStamp(value) },
        value: unsafe { IOHIDValueGetIntegerValue(value) } as i64,
    })
}

/// Ask to be told when this device goes away.
fn watch_for_removal(device: IOHIDDeviceRef) {
    unsafe { IOHIDDeviceRegisterRemovalCallback(device, on_removed, device as *mut c_void) };
}

/// Put it on this thread's run loop.
fn schedule_device(device: IOHIDDeviceRef) {
    unsafe { IOHIDDeviceScheduleWithRunLoop(device, this_run_loop(), kCFRunLoopDefaultMode) };
}

/// Ask to be told when this queue has values.
fn watch_for_values(queue: IOHIDQueueRef) {
    unsafe { IOHIDQueueRegisterValueAvailableCallback(queue, on_value, queue as *mut c_void) };
}

/// Put it on this thread's run loop.
fn schedule_queue(queue: IOHIDQueueRef) {
    unsafe { IOHIDQueueScheduleWithRunLoop(queue, this_run_loop(), kCFRunLoopDefaultMode) };
}

/// Start it delivering.
fn start_reading(queue: IOHIDQueueRef) {
    unsafe { IOHIDQueueStart(queue) };
}

/// Stop it delivering.
fn stop_reading(queue: IOHIDQueueRef) {
    unsafe { IOHIDQueueStop(queue) };
}

/// Give up a Core Foundation object.
fn let_go(value: CFTypeRef) {
    unsafe { CFRelease(value) };
}

/// The run loop this thread is turning.
fn this_run_loop() -> CFRunLoopRef {
    unsafe { CFRunLoopGetCurrent() }
}

/// What one mach tick is worth, as the ratio the clock is scaled by.
fn timebase() -> MachTimebase {
    let mut info = MachTimebase::default();
    unsafe { mach_timebase_info(&mut info) };
    info
}

/// The mach clock, in its own ticks.
fn mach_ticks() -> u64 {
    unsafe { mach_absolute_time() }
}

/// Per-callback state. Lives on the capture thread and is reached from the C
/// callbacks through a raw pointer, which is sound because every callback fires
/// on that one run loop.
struct Capture {
    out: Sender<Captured>,
    clock: Clock,
    held: Held,
    /// Lists rather than maps keyed several ways: the value callback is handed a
    /// queue, the removal callback a device, and matching gives a registry id,
    /// and there are only ever a handful of keyboards to scan.
    pending: Vec<Pending>,
    watched: Vec<Watched>,
    /// What the callbacks have put down for the run's own drain to read.
    ready: Vec<Ready>,
    gone: Vec<Departed>,
    /// The iterators devices come off, oldest first.
    ///
    /// More than one, because the loop above asks to be told about arrivals
    /// before it lists what is here: a device that arrives in between is on
    /// both, and which of the two a finding came off is nothing the loop has to
    /// know.
    iterators: Vec<IoIterator>,
    /// Whether the one at the front of that list answered with nothing.
    ///
    /// Kept rather than acted on where it happened, because a call that also
    /// dropped the iterator would be a turning beside it: the next call reads
    /// this and starts on the one behind it (ADR-0006).
    exhausted: bool,
    /// What the run has asked of this thread and is waiting on.
    asked: Receiver<Request>,
    /// The one it is answering, which is where the answer goes once the calls
    /// that ask was made of have been made.
    answering: Option<Request>,
}

impl Capture {
    /// Put this on the run's stream, and say whether anything is reading it.
    fn hand_up(&self, kind: EventKind) -> bool {
        self.out
            .send(Captured::Event(HostEvent {
                at: self.clock.now(),
                kind,
            }))
            .is_ok()
    }

    /// The next service one of the iterators has.
    ///
    /// One call on the first of them, and the one that answered with nothing is
    /// dropped so that the next call is made on the next: IOKit re-arms its
    /// telling only once the arrivals iterator has been drained, so an iterator
    /// that stopped being looked at is the last arrival this thread ever heard
    /// about. The run comes back round for the rest, which is one turn later for
    /// a device that arrived while the ones already here were being listed.
    fn next_found(&mut self) -> Option<Handed> {
        let service = unsafe { IOIteratorNext(the_iterator(&mut self.iterators, self.exhausted)) };
        self.exhausted = service == 0;
        a_service(service)
    }

    /// The next thing the run has asked of this thread, under the machine's own
    /// name for the device it asked about.
    fn next_ask(&mut self) -> Option<favjit_host::capture::Ask> {
        self.answering = self.asked.try_recv().ok();
        asked_for(&self.answering, &self.pending)
    }

    /// Answer the ask this thread is on.
    fn answer_the_ask(&mut self, code: i32) {
        answered(&self.answering, code)
    }

    /// Keep the iterator the devices already here come off, and say whether
    /// the registry gave one.
    fn look_here(&mut self) -> bool {
        came_off(&mut self.iterators, every_hid_device())
    }

    /// The registry's own id for one finding.
    fn name_of(&mut self, found: Handed) -> Option<u64> {
        registry_id(the_service(found))
    }

    /// A HID device over that registry entry.
    fn open_it(&mut self, found: Handed) -> Option<Handed> {
        make_device(the_service(found))
    }

    fn let_go_of_the_finding(&mut self, found: Handed) {
        release_service(the_service(found))
    }

    fn watch_for_its_removal(&mut self, device: Handed) {
        watch_for_removal(the_device(device))
    }

    fn put_it_where_reports_arrive(&mut self, device: Handed) {
        schedule_device(the_device(device))
    }

    fn describe(&mut self, device: Handed, as_this: DeviceId) -> EventKind {
        describe(the_device(device), as_this)
    }

    fn keep(&mut self, device: Handed, as_this: DeviceId) {
        self.pending.push(Pending {
            id: as_this,
            device: the_device(device),
        });
    }

    fn let_go_of_the_device(&mut self, device: Handed) {
        let_go(the_device(device))
    }

    /// Let go of a device the loop above does not want under that number.
    fn forget(&mut self, device: DeviceId) {
        forgotten(&mut self.pending, device)
    }

    /// One device IOKit found. Keep it and say so, unless it is one this thread
    /// already has.
    /// Open one device, exclusively or not, and say what the platform's own
    /// call returned.
    fn seize(&mut self, device: Handed, exclusive: bool) -> i32 {
        unsafe { IOHIDDeviceOpen(the_device(device), seizing(exclusive)) }
    }

    /// Keep it among the ones this run took away from everything else.
    fn keep_what_was_seized(&mut self, device: Handed, code: i32, exclusive: bool) {
        seized(&self.held, &self.pending, device, code, exclusive)
    }

    /// Every element the device declares.
    fn every_element_of(&mut self, device: Handed) -> Option<Handed> {
        an_array(unsafe {
            IOHIDDeviceCopyMatchingElements(the_device(device), std::ptr::null(), 0)
        })
    }

    /// A queue over that device, deep enough not to drop what a person types.
    ///
    /// A queue drops values when it fills, and the value most costly to lose is
    /// a key-up: the modifier it belongs to stays down inside every application.
    /// Depth is bounded memory and nothing else, so it is sized past any burst a
    /// person can type rather than to a number that has to be argued for.
    /// Karabiner uses the same figure.
    fn make_a_queue_over(&mut self, device: Handed) -> Option<Handed> {
        a_queue(unsafe { IOHIDQueueCreate(std::ptr::null(), the_device(device), QUEUE_DEPTH, 0) })
    }

    /// How many elements that array holds.
    ///
    /// Core Foundation's own count, which asks the machine nothing: it was in
    /// the array before the call was made.
    fn how_many_elements_of(&mut self, elements: Handed) -> usize {
        how_many(the_array(elements))
    }

    /// Whether the array holds an element at that place at all.
    fn has_an_element_at(&mut self, elements: Handed, at: usize) -> bool {
        !at_place(the_array(elements), at).is_null()
    }

    /// Put the one at that place on the queue.
    fn put_one_element_on(&mut self, queue: Handed, elements: Handed, at: usize) -> bool {
        add_element(the_queue(queue), at_place(the_array(elements), at))
    }

    fn let_go_of_the_elements(&mut self, elements: Handed) {
        let_go(the_array(elements) as CFTypeRef)
    }

    fn watch_for_its_values(&mut self, queue: Handed) {
        watch_for_values(the_queue(queue))
    }

    fn put_it_where_values_arrive(&mut self, queue: Handed) {
        schedule_queue(the_queue(queue))
    }

    fn start_it_delivering(&mut self, queue: Handed) {
        start_reading(the_queue(queue))
    }

    /// Keep the queue against the device it is over, and take that device off
    /// the list of ones waiting to be read.
    fn keep_the_queue(&mut self, queue: Handed, device: Handed) {
        watching(&mut self.watched, &mut self.pending, queue, device)
    }

    fn let_go_of_the_queue(&mut self, queue: Handed) {
        let_go(the_queue(queue))
    }

    /// Put down that this queue has values, for the run's own drain to read
    /// them off it.
    ///
    /// Put down rather than drained here: the callback fires from inside the
    /// turn, so a drain written here would be every keystroke read and
    /// delivered in the one place no test reaches — and the drain the run makes
    /// is in that same turn, so nothing waits longer for it (ADR-0006).
    fn a_queue_has_values(&mut self, queue: IOHIDQueueRef) {
        self.ready.push(Ready { queue });
    }

    /// Put down that this device has gone, likewise.
    fn a_device_has_gone(&mut self, device: IOHIDDeviceRef) {
        self.gone.push(Departed { device });
    }

    /// The next queue that said it had values.
    fn next_thing_with_values(&mut self) -> Option<Handed> {
        the_front(&self.ready)
    }

    /// One value off it, as the numbers it carries.
    fn next_value_of(&mut self, from: Handed) -> Option<EventKind> {
        a_value(
            unsafe { IOHIDQueueCopyNextValueWithTimeout(the_queue(from), 0.0) },
            named(&self.watched, from),
        )
    }

    /// What says that queue has run dry.
    fn nothing_more_of(&mut self, from: Handed) -> EventKind {
        EventKind::HidValuesDone {
            device: named(&self.watched, from).unwrap_or(DeviceId(0)),
        }
    }

    fn done_with(&mut self, from: Handed) {
        no_longer_ready(&mut self.ready, from)
    }

    /// The next device that said it had gone.
    fn next_gone(&mut self) -> Option<Handed> {
        the_first_gone(&self.gone)
    }

    /// The queue over it, and none where nothing was reading it.
    fn its_queue(&mut self, device: Handed) -> Option<Handed> {
        queue_over(&self.watched, device)
    }

    /// Give the device back, which is what a seize has to be undone with.
    fn give_it_back(&mut self, device: Handed) -> i32 {
        unsafe { IOHIDDeviceClose(the_device(device), kIOHIDOptionsTypeSeizeDevice) }
    }

    /// Let go of it as one this thread knows about, and say what the run called
    /// it.
    fn forget_it(&mut self, device: Handed) -> Option<DeviceId> {
        gone_from(
            &mut self.watched,
            &mut self.pending,
            &mut self.gone,
            &self.held,
            device,
        )
    }
}

/// How many values a queue holds before it starts dropping them.
///
/// Bounded memory and nothing else, so it is sized past any burst a person can
/// type rather than to a number that has to be argued for. Karabiner uses the
/// same figure. The value most costly to lose is a key-up: the modifier it
/// belongs to stays down inside every application.
const QUEUE_DEPTH: isize = 1024;

/// Put one element on a queue.
///
/// Answers that it went, because the call says nothing about it: what an
/// element on a queue does is deliver, and whether it does is what the queue
/// waking up says.
fn add_element(queue: IOHIDQueueRef, element: CFTypeRef) -> bool {
    unsafe { IOHIDQueueAddElement(queue, element) };
    true
}

/// The properties `engine` is handed about one device.
///
/// Every one of them read and none of them read *into* anything: whether this
/// is a keyboard at all, whether it is the Mac's own, and what USB identity it
/// has are three tables, and they are `engine`'s where the suite drives them
/// (ADR-0006). See `docs/platform/macos/hid-device-enumeration.md` for what
/// each property carries.
fn describe(device: IOHIDDeviceRef, id: DeviceId) -> EventKind {
    EventKind::HidDeviceFound {
        device: id,
        primary_usage_page: cf::device_number(device, "PrimaryUsagePage"),
        primary_usage: cf::device_number(device, "PrimaryUsage"),
        transport: cf::device_string(device, "Transport"),
        product: cf::device_string(device, "Product"),
        vendor_id: cf::device_number(device, "VendorID"),
        product_id: cf::device_number(device, "ProductID"),
    }
}

/// The end of the capture loop `engine` turns (ADR-0006).
///
/// Holds the state directly rather than reaching for it through the static the
/// callbacks use: this loop is turned on the thread that made it, so there is
/// always one — and a pointer that could be absent would have every operation
/// ask whether there is one before making its call, which is a turning beside
/// that call.
///
/// Two borrows of that state alive at once cannot happen from here: a value
/// callback IOKit makes reaches it too, but only from inside
/// [`turn_the_run_loop`], and nothing below holds a borrow across that.
pub struct Turning {
    supervisor: crate::supervisor::Supervisor,
    capture: *mut Capture,
    /// The port IOKit delivers device notifications on, made by the first of
    /// the steps below and asked of by the two after it.
    port: IONotificationPortRef,
}

/// The one `Capture` this thread made.
///
/// Sound while this thread is the one turning the run loop, which is the only
/// time any of it is reached: the thread makes it before it turns the loop and
/// the loop never comes back.
fn the_capture(raw: *mut Capture) -> &'static mut Capture {
    unsafe { &mut *raw }
}

impl favjit_host::capture::Reading for Turning {
    fn turn_the_loop(&mut self, poll: core::time::Duration) {
        turn_the_run_loop(poll);
    }

    fn take_probes(&mut self) -> usize {
        self.supervisor.take_probes()
    }

    fn deliver(&mut self, event: EventKind) -> bool {
        the_capture(self.capture).hand_up(event)
    }

    /// The thread lives as long as the process does.
    ///
    /// Nothing ends it: giving the keyboards back is `Drop` on the run's own
    /// side, and a loop that stopped turning would leave the queues delivering
    /// to a run loop nobody is running (ADR-0008).
    fn over(&mut self) -> bool {
        false
    }
}

impl favjit_host::capture::SinkCapture for Turning {
    fn open_a_notification_port(&mut self) -> bool {
        self.port = notification_port();
        there_is_one(self.port).is_some()
    }

    fn ask_to_be_told_about_devices(&mut self) -> bool {
        let hotplug = watch_for_devices(self.port);
        let arriving = there_is_a_service(hotplug);
        came_off(&mut the_capture(self.capture).iterators, arriving)
    }

    fn put_that_port_on_this_loop(&mut self) {
        add_run_loop_source(self.port);
    }

    fn look_at_the_devices_here(&mut self) -> bool {
        the_capture(self.capture).look_here()
    }

    fn next_device(&mut self) -> Option<Handed> {
        the_capture(self.capture).next_found()
    }

    fn name_of(&mut self, found: Handed) -> Option<u64> {
        the_capture(self.capture).name_of(found)
    }

    fn open_it(&mut self, found: Handed) -> Option<Handed> {
        the_capture(self.capture).open_it(found)
    }

    fn let_go_of_the_finding(&mut self, found: Handed) {
        the_capture(self.capture).let_go_of_the_finding(found)
    }

    fn watch_for_its_removal(&mut self, device: Handed) {
        the_capture(self.capture).watch_for_its_removal(device)
    }

    fn put_it_where_reports_arrive(&mut self, device: Handed) {
        the_capture(self.capture).put_it_where_reports_arrive(device)
    }

    fn describe(&mut self, device: Handed, as_this: DeviceId) -> EventKind {
        the_capture(self.capture).describe(device, as_this)
    }

    fn keep(&mut self, device: Handed, as_this: DeviceId) {
        the_capture(self.capture).keep(device, as_this)
    }

    fn let_go_of_the_device(&mut self, device: Handed) {
        the_capture(self.capture).let_go_of_the_device(device)
    }

    fn forget(&mut self, device: DeviceId) {
        the_capture(self.capture).forget(device)
    }

    fn next_ask(&mut self) -> Option<favjit_host::capture::Ask> {
        the_capture(self.capture).next_ask()
    }

    fn seize(&mut self, device: Handed, exclusive: bool) -> i32 {
        the_capture(self.capture).seize(device, exclusive)
    }

    fn keep_what_was_seized(&mut self, device: Handed, code: i32, exclusive: bool) {
        the_capture(self.capture).keep_what_was_seized(device, code, exclusive)
    }

    fn every_element_of(&mut self, device: Handed) -> Option<Handed> {
        the_capture(self.capture).every_element_of(device)
    }

    fn make_a_queue_over(&mut self, device: Handed) -> Option<Handed> {
        the_capture(self.capture).make_a_queue_over(device)
    }

    fn how_many_elements_of(&mut self, elements: Handed) -> usize {
        the_capture(self.capture).how_many_elements_of(elements)
    }

    fn has_an_element_at(&mut self, elements: Handed, at: usize) -> bool {
        the_capture(self.capture).has_an_element_at(elements, at)
    }

    fn put_one_element_on(&mut self, queue: Handed, elements: Handed, at: usize) -> bool {
        the_capture(self.capture).put_one_element_on(queue, elements, at)
    }

    fn let_go_of_the_elements(&mut self, elements: Handed) {
        the_capture(self.capture).let_go_of_the_elements(elements)
    }

    fn watch_for_its_values(&mut self, queue: Handed) {
        the_capture(self.capture).watch_for_its_values(queue)
    }

    fn put_it_where_values_arrive(&mut self, queue: Handed) {
        the_capture(self.capture).put_it_where_values_arrive(queue)
    }

    fn start_it_delivering(&mut self, queue: Handed) {
        the_capture(self.capture).start_it_delivering(queue)
    }

    fn keep_the_queue(&mut self, queue: Handed, device: Handed) {
        the_capture(self.capture).keep_the_queue(queue, device)
    }

    fn let_go_of_the_queue(&mut self, queue: Handed) {
        the_capture(self.capture).let_go_of_the_queue(queue)
    }

    fn answer_the_ask(&mut self, code: i32) {
        the_capture(self.capture).answer_the_ask(code)
    }

    fn next_thing_with_values(&mut self) -> Option<Handed> {
        the_capture(self.capture).next_thing_with_values()
    }

    fn next_value_of(&mut self, from: Handed) -> Option<EventKind> {
        the_capture(self.capture).next_value_of(from)
    }

    /// How long ago the HID system stamped a value.
    ///
    /// The mach clock, not [`Clock`], and only because both ends of this one
    /// measurement have to be on it: a value's stamp is in mach ticks, and there
    /// is no conversion from those to the `std::time::Instant` the events carry.
    fn how_long_ago(&mut self, stamp: u64) -> u64 {
        since_stamp(stamp)
    }

    fn nothing_more_of(&mut self, from: Handed) -> EventKind {
        the_capture(self.capture).nothing_more_of(from)
    }

    fn done_with(&mut self, from: Handed) {
        the_capture(self.capture).done_with(from)
    }

    fn next_gone(&mut self) -> Option<Handed> {
        the_capture(self.capture).next_gone()
    }

    fn its_queue(&mut self, device: Handed) -> Option<Handed> {
        the_capture(self.capture).its_queue(device)
    }

    fn stop_it_delivering(&mut self, queue: Handed) {
        stop_reading(the_queue(queue))
    }

    fn give_it_back(&mut self, device: Handed) -> i32 {
        the_capture(self.capture).give_it_back(device)
    }

    fn forget_it(&mut self, device: Handed) -> Option<DeviceId> {
        the_capture(self.capture).forget_it(device)
    }
}

/// The one `Capture` on the capture thread, for the callbacks to find.
///
/// A static rather than a context pointer threaded through every callback: the
/// device-matching callback and the per-device removal callback each want a
/// different context of their own, so one of them would have to find its way
/// back here regardless.
static CAPTURE: Mutex<usize> = Mutex::new(0);

/// Hand what one callback was told to the function that acts on it, against the
/// state on this thread.
///
/// A function and a value rather than a closure, so that what a callback does
/// with that state is a named function of its own with its own one call — a
/// closure would put the whole of it inside the callback, which is the one place
/// no test reaches (ADR-0006).
fn with_capture<T>(what: T, f: fn(&mut Capture, T)) {
    let raw = *CAPTURE.lock().unwrap();
    // Sound while the thread that set it is still on the run loop, which is the
    // only time a callback can be reached at all; zero is what it holds before
    // that thread starts and after it comes back.
    let _ = (raw != 0)
        .then_some(raw)
        .map(|raw| f(unsafe { &mut *(raw as *mut Capture) }, what));
}

extern "C" fn on_value(ctx: *mut c_void, _r: i32, _sender: *mut c_void) {
    with_capture(ctx as IOHIDQueueRef, Capture::a_queue_has_values);
}

extern "C" fn on_removed(ctx: *mut c_void, _r: i32, _sender: *mut c_void) {
    with_capture(ctx as IOHIDDeviceRef, Capture::a_device_has_gone);
}

/// What IOKit calls when a device it was asked about arrives.
///
/// Nothing, because the arrival is on the iterator the notification was
/// registered with and the loop takes it off there. A callback that took it
/// instead would be the one place a run's own order cannot reach.
extern "C" fn on_matched(_ctx: *mut c_void, _iterator: IoIterator) {}

/// A port for IOKit to deliver notifications on.
fn notification_port() -> IONotificationPortRef {
    unsafe { IONotificationPortCreate(0) }
}

/// Ask to be told about every HID device that arrives, and hand back the ones
/// already there.
fn watch_for_devices(port: IONotificationPortRef) -> IoIterator {
    let mut hotplug: IoIterator = 0;
    unsafe {
        IOServiceAddMatchingNotification(
            port,
            IO_MATCHED_NOTIFICATION.as_ptr(),
            IOServiceMatching(IOHID_DEVICE_CLASS.as_ptr()),
            on_matched,
            std::ptr::null_mut(),
            &mut hotplug,
        )
    };
    hotplug
}

/// Put that port's source on this thread's run loop.
fn add_run_loop_source(port: IONotificationPortRef) {
    unsafe {
        CFRunLoopAddSource(
            this_run_loop(),
            IONotificationPortGetRunLoopSource(port),
            kCFRunLoopDefaultMode,
        )
    };
}

/// Every HID device the registry holds right now.
fn every_hid_device() -> Option<IoIterator> {
    let mut existing: IoIterator = 0;
    let found = unsafe {
        IOServiceGetMatchingServices(
            0,
            IOServiceMatching(IOHID_DEVICE_CLASS.as_ptr()),
            &mut existing,
        )
    };
    where_it_went(found, existing)
}

/// Let the run loop deliver whatever is waiting, for at most `poll` — and come
/// back as soon as it has delivered something.
///
/// A poll rather than a run-loop source for the probe descriptor and a pending
/// device request, because adding a `CFFileDescriptor` to the loop would put
/// the watchdog's liveness check behind the same loop it is meant to be
/// testing, so a wedge there would silence the probe instead of answering it.
/// Polling from the loop's own turn has the same blind spot, but the
/// watchdog's timeout covers it — a probe that produces no heartbeat is the
/// verdict either way.
///
/// Back after the first source rather than at the end of the slice, because a
/// callback here puts down which queue has values and nothing more: the values
/// are read off by the loop that made this call, once it returns. Kept running
/// to the end of the slice, every keystroke and every pointer report would wait
/// for it — measured as bursts of a dozen or more reports once per slice
/// (`docs/platform/macos/input-latency.md`).
fn turn_the_run_loop(poll: core::time::Duration) {
    unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, poll.as_secs_f64(), true) };
}

/// The registry's own id for it.
fn registry_id(service: IoObject) -> Option<u64> {
    let mut registry: u64 = 0;
    let read = unsafe { IORegistryEntryGetRegistryEntryID(service, &mut registry) };
    where_it_went(read, registry)
}

/// A HID device over that registry entry, as the run holds it.
fn make_device(service: IoObject) -> Option<Handed> {
    a_device(unsafe { IOHIDDeviceCreate(std::ptr::null(), service) })
}

/// Give up a registry entry.
pub(crate) fn release_service(service: IoObject) {
    unsafe { IOObjectRelease(service) };
}

/// Start looking for keyboards. The returned channel carries everything the run
/// loop sees; [`Held`] is how whatever owns this releases the keyboards it took,
/// and the [`Sender<Request>`] is how it takes and reads the ones it wants.
///
/// The channel comes from the caller so that whatever else observes input — the
/// link from the other machine — puts its events into this same stream, and can do
/// so before any keyboard has been taken. A second stream would need the loop to
/// choose between them, and that choice is the reordering ADR-0006 keeps out of
/// `engine`.
pub fn capture(
    out: Sender<Captured>,
    supervisor: crate::supervisor::Supervisor,
    work: favjit_host::capture::SinkLoop,
) -> (Reading, Clock) {
    let held: Held = Arc::new(Mutex::new(Vec::new()));
    let theirs = Arc::clone(&held);
    // Started here rather than on the capture thread, so the caller reads the
    // same clock the events are stamped with.
    let clock = Clock::start();
    let (requests, incoming): (Sender<Request>, Receiver<Request>) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("favjit-hid-capture".into())
        .spawn(move || {
            let capture = Box::into_raw(Box::new(Capture {
                out,
                clock,
                held: theirs,
                pending: Vec::new(),
                watched: Vec::new(),
                ready: Vec::new(),
                gone: Vec::new(),
                iterators: Vec::new(),
                exhausted: false,
                asked: incoming,
                answering: None,
            }));
            *CAPTURE.lock().unwrap() = capture as usize;
            work(&mut Turning {
                supervisor,
                capture,
                port: std::ptr::null_mut(),
            });
        })
        .expect("the capture thread is the first thing this host does");
    (Reading { held, requests }, clock)
}

/// The loop reading this machine's keyboards, as the run holds it.
pub struct Reading {
    /// The keyboards taken exclusively, shared with the thread that opens them.
    held: Held,
    /// The way to ask that thread for one: a device's queue is delivered onto
    /// the thread that opened it, so opening one anywhere else would be a
    /// keyboard nothing reads.
    requests: Sender<Request>,
}

impl favjit_host::sink::Capturing for Reading {
    fn take_device(&mut self, device: DeviceId, exclusive: bool) -> i32 {
        let (reply, answer) = std::sync::mpsc::channel();
        let asked = self.requests.send(Request::Take {
            device,
            exclusive,
            reply,
        });
        opened(asked, &answer)
    }

    fn read_device(&mut self, device: DeviceId) -> bool {
        let (reply, answer) = std::sync::mpsc::channel();
        let asked = self.requests.send(Request::Read { device, reply });
        reading(asked, &answer)
    }

    fn next_held_device(&mut self) -> Option<favjit_host::sink::Holding> {
        next_held(&self.held)
    }

    fn give_it_back(&mut self, device: Handed) -> i32 {
        unsafe { IOHIDDeviceClose(the_device(device), kIOHIDOptionsTypeSeizeDevice) }
    }

    fn forget_what_was_given_back(&mut self, device: Handed, code: i32) {
        given_back(&self.held, device, code)
    }
}

/// What the capture thread said about opening one, and `-1` where it was not
/// there to be asked — which is the number a call that could not be made stands
/// for everywhere else.
fn opened(asked: Result<(), std::sync::mpsc::SendError<Request>>, answer: &Receiver<i32>) -> i32 {
    match asked {
        Ok(()) => answer.recv().unwrap_or(-1),
        Err(_) => -1,
    }
}

/// The same for reading one, which has no number of the machine's to carry: a
/// thread that has gone and a read that would not start are both a keyboard
/// nothing is reading.
fn reading(
    asked: Result<(), std::sync::mpsc::SendError<Request>>,
    answer: &Receiver<bool>,
) -> bool {
    asked.is_ok() && answer.recv().unwrap_or(false)
}

/// The next keyboard still held exclusively, under both names it goes by.
pub fn next_held(held: &Held) -> Option<favjit_host::sink::Holding> {
    held.lock()
        .unwrap()
        .last()
        .map(|held| favjit_host::sink::Holding {
            device: held.id,
            at: Handed(held.device as u64),
        })
}

/// Take one off the list of keyboards this run holds, where the close says it
/// went.
///
/// Only then: a device this still holds is one a later run has to be able to try
/// again for.
pub fn given_back(held: &Held, device: Handed, code: i32) {
    let key = the_device(device) as usize;
    let at = went(code)
        .then(|| {
            held.lock()
                .unwrap()
                .iter()
                .position(|one| one.device == key)
        })
        .flatten();
    let _ = at.map(|at| held.lock().unwrap().remove(at));
}

/// The option flag that says whether an open takes the device away from
/// everything else.
fn seizing(exclusive: bool) -> u32 {
    match exclusive {
        true => kIOHIDOptionsTypeSeizeDevice,
        false => kIOHIDOptionsTypeNone,
    }
}

/// Whether an IOKit call did what it was asked, as its own code says.
fn went(code: i32) -> bool {
    code == kIOReturnSuccess
}

/// A value an IOKit call filled in, where its code says it filled one in.
pub(crate) fn where_it_went<T>(code: i32, value: T) -> Option<T> {
    went(code).then_some(value)
}

/// A service there is one of, out of the zero an exhausted iterator answers
/// with.
fn there_is_a_service(service: IoObject) -> Option<IoObject> {
    (service != 0).then_some(service)
}
