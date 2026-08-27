//! Reading the keyboards and the mice, with the device each event came from.
//!
//! Raw input, and **not** a low-level hook, which is the other way to see this
//! machine's input. A hook is handed a `KBDLLHOOKSTRUCT` with no device in it, so
//! every keyboard would arrive as one with no vendor and product for a rule to
//! match (ADR-0003); and a `MSLLHOOKSTRUCT` carries where the cursor now is rather
//! than how far the mouse moved, which is already accelerated and stops at the
//! edge of the screen, where ADR-0011 relays what the hardware said. Raw input
//! carries both — the device on every event, and `lLastX`/`lLastY`.
//!
//! It arrives as a message to a window, so there is a window and a message loop,
//! and both live on a thread of their own with everything they produce funnelled
//! into one channel. That is where ADR-0006 puts unavoidable concurrency: inside
//! the host, never in `engine`.
//!
//! Refusing the input is [`crate::suppress`]'s, on this same thread, and the two
//! are not paired up: nothing here matches an event against what was refused, so
//! neither has to identify what the other saw. What that costs is stated where the
//! counts are reported — a run has to see input arrive here while the hooks are
//! refusing it, or the input has gone nowhere.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;

use core::mem::size_of;
use core::ptr::null_mut;

use favjit_host::{DeviceId, EventKind, HostEvent, Instant};

use crate::ffi::*;
use crate::supervisor::Supervisor;
use crate::suppress;

/// The chord's two positions, as a hook reports them, for
/// [`suppress::Hooks::install`] to compare against.
///
/// Which keys the chord is made of, and finding and spelling their positions on
/// this keyboard, are settled before this is reached — `engine`'s decision and
/// `favjit-hid`'s tables respectively (ADR-0006) — so what arrives here is
/// already the pair to compare, and absent for a keyboard with no position for
/// one of them.
///
/// Whether the hooks may be installed at all is not here: `engine` asks for
/// that when it asks to start reading, because when a run becomes able to
/// refuse input is part of the order it decides.
#[derive(Debug, Clone, Copy, Default)]
pub struct Chord {
    pub switch_to_the_sink: Option<(u16, bool)>,
    pub switch_back: Option<(u16, bool)>,
}

/// What the capture thread sends up, which is events and nothing else.
///
/// One kind, because everything this thread has to say is something that
/// happened: a fact the run would have to ask for again would be one it read
/// out of order with the keystrokes it arrived among (ADR-0006).
pub type Captured = HostEvent;

/// The host's clock, shared by everything that stamps an event.
///
/// One base copied to whoever needs it, so the capture thread's stamps and any
/// other are on one origin — `engine` compares them against each other.
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

/// What starting the reading left behind.
///
/// A record and not a kind, because the clock is there either way: a caller
/// handed one of two alternatives would have to ask which it was before it could
/// read the time, and asking is a turning beside the call it makes (ADR-0006).
pub struct Started {
    /// Whether the thread started, which is the one failure visible from here:
    /// everything it does after that — the window, the registration — fails on
    /// the thread, and shows up as a stream that ended.
    pub reading: bool,
    /// The clock the stamps on what arrives are on.
    pub clock: Clock,
}

/// Start reading, and hand back the stream and the clock its stamps are on.
pub fn capture(
    chord: Chord,
    supervisor: Supervisor,
    probe_tick: core::time::Duration,
    raw_bytes: usize,
    work: favjit_host::capture::SourceLoop,
    events: Sender<Captured>,
) -> Started {
    let clock = Clock::start();
    let theirs = clock;
    let spawned = std::thread::Builder::new()
        .name(String::from("favjit-capture"))
        .spawn(move || {
            watch(
                chord, theirs, events, supervisor, probe_tick, raw_bytes, work,
            )
        });
    Started {
        reading: started(spawned),
        clock,
    }
}

/// Whether a thread was there to fill the stream.
fn started(spawned: std::io::Result<std::thread::JoinHandle<()>>) -> bool {
    spawned.is_ok()
}

/// Start the thread the hooks and raw input belong to, and turn `work` on it.
///
/// Everything about the order is `work`'s: what is here is the state that thread
/// holds and the one call that begins it (ADR-0006).
#[allow(clippy::too_many_arguments)]
fn watch(
    chord: Chord,
    clock: Clock,
    events: Sender<Captured>,
    supervisor: Supervisor,
    probe_tick: core::time::Duration,
    raw_bytes: usize,
    work: favjit_host::capture::SourceLoop,
) {
    // This is the thread that looks at the probe pipe, because the message loop is
    // the only thing that comes back round often enough to answer one.
    let mut turning = Turning {
        supervisor,
        probe_tick,
        chord,
        keyboard_hook: None,
        pointer_hook: None,
        window: None,
        // The name the tray item finds a running favjit by, so it is stated
        // where both ends read it from (`favjit_tray_wire`).
        class_name: wide(favjit_tray_wire::WINDOW_CLASS),
        watching: Watching {
            clock,
            events,
            devices: Vec::new(),
            pending: Vec::new(),
            arrived: VecDeque::new(),
            buffer: vec![0u8; raw_bytes],
        },
        message: Msg {
            window: null_mut(),
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            point: Point::default(),
        },
        ended: false,
    };
    work(&mut turning);
}

/// The end of the capture loop `engine` turns, on the thread the hooks belong
/// to.
///
/// Every hook call and every raw input report is delivered to this thread, so
/// what the loop above turns is this: one message a turn, and the facts it
/// carries put on the run's stream.
struct Turning {
    supervisor: Supervisor,
    probe_tick: core::time::Duration,
    chord: Chord,
    /// Held for as long as the loop turns, and let go of before the window
    /// because the hooks are what is taking input away: an unhook that waited
    /// for the window to close would leave the keyboard refused for that long.
    keyboard_hook: Option<suppress::Hooked>,
    pointer_hook: Option<suppress::Hooked>,
    window: Option<Window>,
    /// The class name, held for as long as the window is: the calls that read it
    /// read it through a pointer, so a name that lived only as long as one of
    /// them would leave the other reading freed memory.
    class_name: Vec<u16>,
    watching: Watching,
    message: Msg,
    /// Whether the message loop has been told there is nothing more.
    ended: bool,
}

/// The window a step before this one made, and a handle to nothing where it
/// made none.
///
/// A handle either way, because the calls taking one are made unconditionally: a
/// step that asked whether there is a window first would be a turning beside the
/// call it makes (ADR-0006), and no step here is reached at all unless the one
/// that makes the window went.
fn the_window(window: &Option<Window>) -> Handle {
    match window {
        Some(window) => window.handle,
        None => null_mut(),
    }
}

/// The window a call made, as the thing that closes it again.
fn a_window_of(handle: Option<Handle>) -> Option<Window> {
    handle.map(|handle| Window { handle })
}

/// The class the window is made of.
///
/// The name is borrowed rather than made here, because the call that reads it
/// reads it through a pointer: a name made inside this would be freed before
/// that call was made.
fn window_class(class_name: &[u16]) -> WindowClass {
    WindowClass {
        style: 0,
        procedure: Some(DefWindowProcW),
        class_extra: 0,
        window_extra: 0,
        instance: this_module(),
        icon: null_mut(),
        cursor: null_mut(),
        background: null_mut(),
        menu_name: null_mut(),
        class_name: class_name.as_ptr(),
    }
}

impl favjit_host::capture::Reading for Turning {
    /// One message off this thread's queue.
    ///
    /// `poll` is no bound here — a message wait ends when a message arrives, and
    /// the probe timer is what makes one arrive. The hooks are called from
    /// inside this, which is why the thread that installed them is this one.
    fn turn_the_loop(&mut self, _poll: core::time::Duration) {
        self.ended = !next_message(&mut self.message);
    }

    fn take_probes(&mut self) -> usize {
        self.supervisor.take_probes(the_timers_turn(&self.message))
    }

    fn deliver(&mut self, event: EventKind) -> bool {
        self.watching.deliver(event)
    }

    fn over(&mut self) -> bool {
        self.ended
    }
}

impl favjit_host::capture::SourceCapture for Turning {
    /// The class before the window, which is the order the API takes: there is
    /// no window of a class that does not exist.
    fn register_the_window_class(&mut self) {
        register_class(&window_class(&self.class_name));
    }

    fn make_a_window(&mut self) -> bool {
        self.window = a_window_of(make_window(&window_class(&self.class_name)));
        self.window.is_some()
    }

    fn ask_for_the_mice(&mut self) -> bool {
        register(the_window(&self.window))
    }

    fn send_the_keys_to_that_window(&mut self) {
        suppress::keys_go_to(the_window(&self.window));
    }

    fn state_the_chord(&mut self) {
        suppress::the_chord_is_at(self.chord.switch_to_the_sink, self.chord.switch_back);
    }

    fn start_the_probe_timer(&mut self) {
        start_probe_timer(the_window(&self.window), self.probe_tick);
    }

    fn hook_the_keyboard(&mut self) -> bool {
        self.keyboard_hook = suppress::hook_the_keyboard();
        self.keyboard_hook.is_some()
    }

    fn hook_the_pointer(&mut self) -> bool {
        self.pointer_hook = suppress::hook_the_pointer();
        self.pointer_hook.is_some()
    }

    /// The next device a report named that nothing has a number for, under this
    /// number.
    ///
    /// Read off what the turns put down rather than a list of its own, because a
    /// device here is learned from its first report: what says there is a new
    /// one is an arrival naming a device nothing has a number for.
    fn next_device(&mut self, as_this: DeviceId) -> Option<favjit_host::capture::Found> {
        found(
            &mut self.watching.pending,
            &mut self.watching.devices,
            as_this,
        )
    }

    /// The raw input the message carried, and whatever else it said.
    ///
    /// Read off this platform's own message numbers, which is what a frame's
    /// kind is on the other one: what a message means is the pump's, and a
    /// boundary carrying the number instead would be a run reading a message
    /// pump it cannot turn.
    fn take_what_the_turn_produced(&mut self) {
        self.watching.input(the_raw_input(&self.message));
        self.watching.hand_up(what_else_it_said(&self.message));
    }

    /// Hand the message to the window procedure.
    ///
    /// Every message, raw input included: the procedure is `DefWindowProc`, and
    /// for raw input that is what releases what the system allocated to deliver
    /// it — which is why it comes after the payload has been read out.
    fn let_go_of_the_turn(&mut self) {
        dispatch(&self.message);
    }

    fn next_arrival(&mut self) -> Option<EventKind> {
        the_next(&mut self.watching.arrived, &self.watching.devices)
    }
}

/// A window with no screen presence, for raw input to be delivered to.
struct Window {
    handle: Handle,
}

/// Start the timer that wakes this loop to look for a probe, every `tick`.
fn start_probe_timer(window: Handle, tick: core::time::Duration) -> bool {
    let started = unsafe { SetTimer(window, PROBE_TIMER, tick.as_millis() as u32, null_mut()) };
    started != 0
}

/// Wait for the next message on this thread's queue, `false` for a queue with no
/// more to come.
///
/// The call answers three ways and this is two, because the two that are not a
/// message are the same thing to the loop: a queue asked to stop and a queue that
/// cannot be read are both one it has nothing more to take from. Reading either
/// as a message is what a plain non-zero test would do, and that spins on a
/// broken queue forever.
fn next_message(message: &mut Msg) -> bool {
    a_message(unsafe { GetMessageW(message, null_mut(), 0, 0) })
}

/// Whether the queue had one, out of the three ways that call answers.
fn a_message(answered: i32) -> bool {
    !matches!(answered, -1 | 0)
}

/// The raw input this message carried, and a handle to nothing for one that
/// carried none.
///
/// A handle either way, because the read is made unconditionally: a read asked
/// about first would be a turning beside it (ADR-0006), and a read of no raw
/// input answers with the failure that says there is no report here.
fn the_raw_input(message: &Msg) -> Handle {
    match message.message {
        WM_INPUT => message.lparam as Handle,
        _ => null_mut(),
    }
}

/// What this message said besides a raw input, and nothing where it said none
/// of those either.
fn what_else_it_said(message: &Msg) -> Option<(Handle, Said)> {
    match message.message {
        WM_INPUT_DEVICE_CHANGE => a_removal(message.wparam, message.lparam as Handle),
        // A key, handed over by the hook that also decided whether this machine
        // gets it.
        WM_KEY => Some((null_mut(), a_key(message.wparam, message.lparam))),
        // Quit, from the tray item. Handed up as a fact rather than acted on
        // here: what stopping means is the run's, and it is what gives the
        // keyboards back on the way out (ADR-0008).
        favjit_tray_wire::WM_STOP => Some((null_mut(), Said::Nothing(EventKind::AskedToStop))),
        // The tray item asking for the keyboard. On this stream and not read
        // from anywhere, because this loop's one wait is the message above: an
        // ask that had to be polled for would be seen on the next keystroke, and
        // having no usable keyboard is when it is asked
        // (`docs/platform/windows/tray-item-as-its-own-program.md`).
        favjit_tray_wire::WM_ASKED => crate::tray::asked(message.wparam)
            .map(|driving| (null_mut(), Said::Nothing(EventKind::Asked(driving)))),
        _ => None,
    }
}

/// Whether the turn just taken was the probe timer's.
///
/// Off the timer rather than every turn, because reading the pipe is what
/// empties it: a turn woken by a keystroke that also took the probes would
/// answer them at the rate somebody types.
fn the_timers_turn(message: &Msg) -> bool {
    message.message == WM_TIMER && message.wparam == PROBE_TIMER
}

/// Hand it to the window procedure.
fn dispatch(message: &Msg) {
    unsafe { DispatchMessageW(message) };
}

/// One raw input into this buffer, and how many bytes it wrote.
///
/// `None` for a read that failed or wrote less than the header: a structure
/// shorter than the one this expects would otherwise be read as the zeroes after
/// it, and a make code out of untouched buffer is a key nobody pressed.
fn read_raw_input(input: Handle, buffer: &mut [u8]) -> Option<u32> {
    let mut size = buffer.len() as u32;
    let read = unsafe {
        GetRawInputData(
            input,
            RID_INPUT,
            buffer.as_mut_ptr().cast(),
            &mut size,
            size_of::<RawInputHeader>() as u32,
        )
    };
    a_whole_header(read)
}

/// How much a read wrote, and none where it wrote less than a header.
///
/// Short of a header is not a report: what follows the header is read against
/// its own fields, and there are none to read.
fn a_whole_header(answered: u32) -> Option<u32> {
    filled_in(answered).filter(|read| *read as usize >= size_of::<RawInputHeader>())
}

/// A count a call answered with, and none where it answered zero.
fn a_count(answered: u32) -> Option<u32> {
    (answered != 0).then_some(answered)
}

/// How much a call filled in, and none where it answered the failure these APIs
/// spell as every bit set.
fn filled_in(answered: u32) -> Option<u32> {
    (answered != u32::MAX).then_some(answered)
}

/// The handle a call answered with, and none where it made nothing.
fn made(answered: Handle) -> Option<Handle> {
    (!answered.is_null()).then_some(answered)
}

/// This process's own module, which a window class belongs to.
fn this_module() -> Handle {
    unsafe { GetModuleHandleW(null_mut()) }
}

/// Register a window class.
///
/// The result is not read: a class that is already registered is the ordinary
/// case on a second call, and whether the class exists is what creating the
/// window answers.
fn register_class(class: &WindowClass) {
    unsafe { RegisterClassW(class) };
}

/// A window of that class with no screen presence.
fn make_window(class: &WindowClass) -> Option<Handle> {
    let handle = unsafe {
        CreateWindowExW(
            0,
            class.class_name,
            class.class_name,
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            class.instance,
            null_mut(),
        )
    };
    made(handle)
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.handle) };
    }
}

/// Ask for the mice, delivered whether or not this window has the foreground.
///
/// `RIDEV_INPUTSINK` is the whole reason a window is involved and never shown: input has
/// to arrive while the person is looking at the other machine's screen, which is exactly
/// when nothing here has the foreground.
///
/// **The keyboards are not registered for.** A key that is refused never reaches raw
/// input, so the only place one can be both read and refused is the hook that refuses it
/// ([`crate::suppress`], and
/// [docs/platform/windows/hooks-and-raw-input.md](../../../docs/platform/windows/hooks-and-raw-input.md)).
/// A pointer is the other way round, which is why the mouse is here: raw input is where
/// its movement is a movement rather than a cursor position (ADR-0011).
fn register(window: Handle) -> bool {
    let mice = RawInputDevice {
        usage_page: USAGE_PAGE_GENERIC,
        usage: USAGE_MOUSE,
        flags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
        target: window,
    };
    let registered =
        unsafe { RegisterRawInputDevices(&mice, 1, size_of::<RawInputDevice>() as u32) };
    registered != 0
}

/// One device this host is reading.
///
/// Only what Windows' own vocabulary needs remembered. What a device is holding,
/// whether the sink has heard of it, and its pointer's own state between reports
/// are `engine`'s, because they are about what the sink is told rather than
/// about what Windows said.
struct Device {
    /// The machine's own name for it, which is what says two reports came from
    /// the same one.
    ///
    /// Beside the number rather than instead of it, because the two are one
    /// fact: the number is what the run gave this handle, so a list of either
    /// alone could not answer the other (ADR-0006).
    at: Handle,
    id: DeviceId,
}

/// One thing a turn produced, before the run has said what to call the device
/// it came from.
///
/// A kind and not the event itself, because a device is learned from its first
/// report and the number it is called by is the run's: what the turn can put
/// down is the machine's own words, and the event is made of them once the
/// number is there.
struct Arrived {
    /// The machine's own name for whatever it came from.
    ///
    /// Null for the one keyboard a hook can name, which has no handle of its
    /// own — and for the things that came from no device, which are the ones
    /// [`Said::Nothing`] carries whole.
    at: Handle,
    said: Said,
}

/// What the machine said, in its own words.
enum Said {
    /// A key, which is always that one keyboard.
    Key {
        make_code: u16,
        vkey: u16,
        flags: u32,
    },
    /// One pointer report.
    Pointer(RawMouse),
    /// The device has gone.
    Lost,
    /// Something that names no device at all, already in the run's own words.
    Nothing(EventKind),
}

/// The devices, and what this turn has put down.
struct Watching {
    clock: Clock,
    events: Sender<Captured>,
    /// The pointers and the one keyboard, by the machine's own name for each.
    devices: Vec<Device>,
    /// The machine's names for the devices the run has not numbered yet, oldest
    /// first.
    pending: Vec<Handle>,
    /// What this turn put down, in the order it came.
    arrived: VecDeque<Arrived>,
    /// Where one raw input is read into, reused rather than allocated per
    /// message: this sits in the interactive path.
    buffer: Vec<u8>,
}

impl Watching {
    /// Put one thing down for the drain to read, under the machine's own name
    /// for whatever it came from — and nothing where the machine said nothing.
    ///
    /// The one place anything is put down, so that the order the run reads is
    /// the order things happened however many kinds this thread produces. It
    /// takes the absence too, so that a turn that produced nothing is one call
    /// here rather than a question asked in front of one.
    fn hand_up(&mut self, one: Option<(Handle, Said)>) {
        note_the_device(&mut self.pending, &self.devices, &one);
        put_down(&mut self.arrived, one);
    }

    /// Put this on the run's own stream, and say whether anything is reading it.
    fn deliver(&mut self, kind: EventKind) -> bool {
        let at = self.clock.now();
        self.events.send(HostEvent { at, kind }).is_ok()
    }

    /// One raw input message.
    ///
    /// Taken out of `self` for the read rather than borrowed, because what is
    /// read out of it is put down alongside a `&mut self` the buffer would
    /// otherwise still be borrowed by.
    fn input(&mut self, input: Handle) {
        let mut buffer = std::mem::take(&mut self.buffer);
        let read = read_raw_input(input, &mut buffer);
        let report = a_pointer_report(read, &buffer);
        self.buffer = buffer;
        self.hand_up(report);
    }
}

/// One key, as the hook packed it into a message.
///
/// From the hook and not from raw input, because refusing a key takes it away
/// from raw input as well — so the only place a key can be both read and refused
/// is the procedure that refuses it ([`crate::suppress`]).
///
/// **One keyboard, unnamed.** A hook says which key and not which keyboard, so
/// everything this machine forwards is attributed to one device with no vendor
/// and no product. ADR-0003 allows for it.
///
/// The structure's fields and nothing about what they mean — not even whether
/// they are a key: three of the things arriving here are not one, and which is
/// `engine`'s to read (ADR-0006), reached once this arrives at the entry point
/// the run came in through. So the keyboard is numbered on the first message
/// rather than the first key, and what the sink is told about is still the first
/// key: announcing a device to it is `favjit_engine::source`'s, and it does that
/// when a key needs one.
fn a_key(packed: Wparam, flags: Lparam) -> Said {
    Said::Key {
        make_code: (packed & 0xFFFF) as u16,
        vkey: ((packed >> 16) & 0xFFFF) as u16,
        flags: flags as u32,
    }
}

/// One pointer report out of a raw input this thread read, and none where it
/// read something else or read too little to name.
///
/// Bounded by what was actually written rather than by the buffer, so that a
/// structure shorter than the one this expects is nothing rather than the zeroes
/// after it: a make code read out of untouched buffer is a key nobody pressed.
/// And only the mouse is registered for, so only the mouse is read out — a
/// registration that grew a usage should not read one structure as another.
///
/// Every report, including one that says nothing new and one that says where the
/// pointer is rather than how far it moved: which reports are worth relaying,
/// and what `usFlags` means, are `favjit_engine::source`'s (ADR-0006).
fn a_pointer_report(read: Option<u32>, buffer: &[u8]) -> Option<(Handle, Said)> {
    let (at, end) = read
        .zip(read_from::<RawInputHeader>(buffer))
        .filter(|(_, header)| header.kind == RIM_TYPEMOUSE)
        .map(|(read, header)| (header.device, (read as usize).min(buffer.len())))?;
    let payload = &buffer[size_of::<RawInputHeader>()..end];
    read_from::<RawMouse>(payload).map(|raw| (at, Said::Pointer(raw)))
}

/// A device going away, and nothing for one arriving.
///
/// An arrival is nothing to put down: what says there is a device here is a
/// report naming one nothing has a number for, so a run told twice would number
/// the same keyboard twice.
fn a_removal(what: Wparam, handle: Handle) -> Option<(Handle, Said)> {
    (what == GIDC_REMOVAL).then_some((handle, Said::Lost))
}

/// Put it down where the drain reads it, and nothing where there is nothing to
/// put down.
fn put_down(arrived: &mut VecDeque<Arrived>, one: Option<(Handle, Said)>) {
    let _ = one.map(|(at, said)| arrived.push_back(Arrived { at, said }));
}

/// The machine's own name for whatever one arrival came from, and none for the
/// ones that came from no device.
///
/// A number and not the handle, because that is what crosses to say two
/// findings are the same device (`favjit_host::capture::Found`): the handle it
/// stands for stays here.
fn came_from(one: &Option<(Handle, Said)>) -> Option<Handle> {
    match one {
        Some((_, Said::Nothing(_))) | None => None,
        Some((at, _)) => Some(*at),
    }
}

/// Note the machine's name for whatever this came from, where the run has not
/// numbered it and has not been offered it yet.
fn note_the_device(pending: &mut Vec<Handle>, devices: &[Device], one: &Option<(Handle, Said)>) {
    let named =
        came_from(one).filter(|at| numbered(devices, *at).is_none() && !pending.contains(at));
    let _ = named.map(|at| pending.push(at));
}

/// The next device a report named and nothing has a number for, kept under the
/// number the run offers.
///
/// No path, because neither a hook nor a pointer report has one to give: what a
/// path is read for is a vendor and a product, and a rule that named this device
/// by either would be reading a name nothing said (ADR-0003).
fn found(
    pending: &mut Vec<Handle>,
    devices: &mut Vec<Device>,
    as_this: DeviceId,
) -> Option<favjit_host::capture::Found> {
    let at = (!pending.is_empty()).then(|| pending.remove(0))?;
    devices.push(Device { at, id: as_this });
    Some(favjit_host::capture::Found {
        named: at as u64,
        found: EventKind::PathDeviceFound {
            device: as_this,
            path: String::new(),
        },
    })
}

/// The number the run gave the device the machine names this, and none where it
/// has given none yet.
fn numbered(devices: &[Device], at: Handle) -> Option<DeviceId> {
    devices
        .iter()
        .find(|device| device.at == at)
        .map(|device| device.id)
}

/// The next thing the turn put down, as the event the run reads — and none once
/// it put down no more, or while what it put down names a device the run has not
/// numbered.
///
/// Left where it is rather than dropped in that second case: the number arrives
/// on the next drain of the devices, which is the same turn, and a report thrown
/// away for want of one is a keystroke nobody typed twice.
fn the_next(arrived: &mut VecDeque<Arrived>, devices: &[Device]) -> Option<EventKind> {
    let front = arrived.front()?;
    let device = match named_device(&front.said) {
        true => Some(numbered(devices, front.at)?),
        false => None,
    };
    let one = arrived.pop_front()?;
    Some(as_an_event(one.said, device))
}

/// Whether this came from a device at all, as against being something the
/// machine said that names none.
fn named_device(said: &Said) -> bool {
    !matches!(said, Said::Nothing(_))
}

/// What the machine said, under the number the run gave what it came from.
fn as_an_event(said: Said, device: Option<DeviceId>) -> EventKind {
    // The two are looked up together above, so what came from a device the run
    // has not numbered never reaches here and this stands for nothing a report
    // is ever built with.
    let device = device.unwrap_or(DeviceId(0));
    match said {
        Said::Key {
            make_code,
            vkey,
            flags,
        } => EventKind::HookedKey {
            device,
            make_code,
            vkey,
            flags,
        },
        Said::Pointer(raw) => EventKind::MouseReport {
            device,
            flags: raw.flags,
            button_flags: raw.button_flags,
            button_data: raw.button_data,
            dx: raw.x,
            dy: raw.y,
        },
        Said::Lost => EventKind::DeviceLost(device),
        Said::Nothing(kind) => kind,
    }
}

/// One structure, out of the front of a buffer Windows filled in.
///
/// Unaligned, because the buffer is bytes and a raw input's payload starts
/// wherever the header ends.
fn read_from<T: Copy>(bytes: &[u8]) -> Option<T> {
    (bytes.len() >= size_of::<T>())
        .then(|| unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) })
}

/// The input devices this machine has attached, as the run reads them.
///
/// The list is held here between the two calls that make it, and each path is
/// held here between the two that make one: this API answers with how much room
/// it needs before it will fill any in, so what crosses is a place in the list
/// and never the handle at it (ADR-0006).
#[derive(Default)]
pub struct Attached {
    list: Vec<RawInputDeviceList>,
}

/// The list of what is attached, before anything has been looked at.
pub fn what_is_attached() -> Attached {
    Attached::default()
}

impl favjit_host::source::Listing for Attached {
    fn how_many(&mut self) -> usize {
        as_many(device_count())
    }

    fn look(&mut self, how_many: usize) -> usize {
        self.list = room_for(how_many);
        as_many(read_device_list(&mut self.list))
    }

    fn at(&mut self, place: usize) -> Option<favjit_host::source::Kind> {
        self.list.get(place).map(|entry| a_kind(entry.kind))
    }

    fn how_long_its_path_is(&mut self, place: usize) -> usize {
        as_many(path_length(at(&self.list, place)))
    }

    fn its_path(&mut self, place: usize, room: usize) -> Option<String> {
        // One more than the room asked for, so a path written with a nul on the
        // end has somewhere to put it.
        let mut buffer = vec![0u16; room + 1];
        let mut size = buffer.len() as u32;
        let written = read_path(at(&self.list, place), &mut buffer, &mut size);
        the_text(written, &buffer)
    }
}

/// Room for that many entries, which is what the call that fills them in wants
/// before it will fill any.
fn room_for(how_many: usize) -> Vec<RawInputDeviceList> {
    vec![
        RawInputDeviceList {
            device: null_mut(),
            kind: 0,
        };
        how_many
    ]
}

/// A count a call answered with, and none where it answered none.
fn as_many(answered: Option<u32>) -> usize {
    answered.unwrap_or(0) as usize
}

/// The machine's own name for the device at that place, and a handle to nothing
/// where nothing was looked at there.
///
/// A handle either way, because the calls taking one are made unconditionally: a
/// call asked about first would be a turning beside it (ADR-0006), and this API
/// answers a handle it does not know with the failure those calls report.
fn at(list: &[RawInputDeviceList], place: usize) -> Handle {
    match list.get(place) {
        Some(entry) => entry.device,
        None => null_mut(),
    }
}

/// What this machine says one of its input devices is.
fn a_kind(kind: u32) -> favjit_host::source::Kind {
    match kind {
        RIM_TYPEKEYBOARD => favjit_host::source::Kind::Keyboard,
        RIM_TYPEMOUSE => favjit_host::source::Kind::Pointer,
        _ => favjit_host::source::Kind::Something,
    }
}

/// How many characters — not bytes — this device's path needs.
fn path_length(handle: Handle) -> Option<u32> {
    let mut size: u32 = 0;
    unsafe { GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, null_mut(), &mut size) };
    a_count(size)
}

/// Fill that buffer with it, and say how much was written.
fn read_path(handle: Handle, buffer: &mut [u16], size: &mut u32) -> Option<u32> {
    let written = unsafe {
        GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, buffer.as_mut_ptr().cast(), size)
    };
    what_was_written(written)
}

/// How many characters a fill wrote, and none for either way it answered
/// nothing: this API spells a failure as every bit set and an empty path as
/// zero, and a path of no characters is not one.
fn what_was_written(answered: u32) -> Option<u32> {
    filled_in(answered).and_then(a_count)
}

/// The path a fill wrote, cut to what it wrote and to the first nul in it.
fn the_text(written: Option<u32>, buffer: &[u16]) -> Option<String> {
    written.map(|written| {
        let end = (written as usize).min(buffer.len());
        String::from(String::from_utf16_lossy(&buffer[..end]).trim_end_matches('\0'))
    })
}

/// How many input devices there are to list.
fn device_count() -> Option<u32> {
    let mut count: u32 = 0;
    let size = size_of::<RawInputDeviceList>() as u32;
    unsafe { GetRawInputDeviceList(null_mut(), &mut count, size) };
    a_count(count)
}

/// Fill that list with them, and say how many were written.
fn read_device_list(list: &mut [RawInputDeviceList]) -> Option<u32> {
    let mut count = list.len() as u32;
    let size = size_of::<RawInputDeviceList>() as u32;
    let read = unsafe { GetRawInputDeviceList(list.as_mut_ptr(), &mut count, size) };
    filled_in(read)
}
