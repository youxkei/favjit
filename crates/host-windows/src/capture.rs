//! Reading the keyboards and the mice, with the device each event came from.
//!
//! Raw input, and **not** a low-level hook, which is the other way to see this
//! machine's input. A hook is handed a `KBDLLHOOKSTRUCT` with no device in it, so
//! every keyboard would arrive as one with no vendor and product for a rule to
//! match (ADR-0003); and a `MSLLHOOKSTRUCT` carries where the cursor now is rather
//! than how far the mouse moved, which is already accelerated and stops at the
//! edge of the screen, where ADR-0016 relays what the hardware said. Raw input
//! carries both — the device on every event, and `lLastX`/`lLastY`.
//!
//! It arrives as a message to a window, so there is a window and a message loop,
//! and both live on a thread of their own with everything they produce funnelled
//! into one channel. That is where ADR-0006 puts unavoidable concurrency: inside
//! the host, never in `core`.
//!
//! Refusing the input is [`crate::suppress`]'s, on this same thread, and the two
//! are not paired up: nothing here matches an event against what was refused, so
//! neither has to identify what the other saw. What that costs is stated where the
//! counts are reported — a run has to see input arrive here while the hooks are
//! refusing it, or the input has gone nowhere.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};

use core::mem::size_of;
use core::ptr::null_mut;

use favjit_core::{DeviceId, DeviceInfo, EventKind, HostEvent, Instant};
use log::{error, info, warn};

use crate::ffi::*;
use crate::pointer::Pointing;
use crate::scancode::Pressed;
use crate::supervisor::Supervisor;
use crate::{device, scancode, suppress};

/// How often the capture loop wakes to look for a probe.
///
/// The message loop otherwise sleeps until something is typed, and a watchdog's probe
/// is not a message. Well inside the silence a watchdog allows, so an idle machine
/// answers several times over before its bound is reached — and slow enough that an
/// idle machine is not being woken for nothing.
const PROBE_TICK_MS: u32 = 100;

/// What to watch, and whether to take it away from this machine.
///
/// There is no list of devices to leave alone, unlike the macOS side's. A hook is
/// not told which device it was called for, so a device left out of the relay
/// would still have its input refused — which is that device's keys doing
/// nothing at all, the outcome ADR-0008 rules out.
/// Whether the hooks may be installed is not here: `core` asks for that when it
/// asks to start reading, because when a run becomes able to refuse input is part
/// of the order it decides ([`favjit_core::source::SourceHost::take_input`]).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The keyboards are ANSI ones.
    ///
    /// One position depends on it — see [`crate::scancode::key`] — and the make
    /// code cannot say which keyboard sent it.
    pub ansi: bool,
}

/// A make code no key is named for.
///
/// Reported rather than dropped silently, because while suppression is on a key
/// that lands here is a key that has stopped working: the hook refuses it and
/// nothing relays it. Which ones those are is the gap, and it is invisible from a
/// run that does not say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownScanCode {
    pub device: DeviceId,
    pub extended: bool,
    pub code: u16,
}

/// What the capture thread sends up.
pub enum Captured {
    Event(HostEvent),
    Unknown(UnknownScanCode),
    /// A pointer that says where it is rather than how far it moved, which this
    /// relay has no way to carry.
    Absolute(DeviceId),
}

/// The host's clock, shared by everything that stamps an event.
///
/// One base copied to whoever needs it, so the capture thread's stamps and any
/// other are on one origin — `core` compares them against each other.
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
        Instant::from_nanos(u64::try_from(self.base.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }
}

/// The largest raw input this host asks for.
///
/// A mouse's is the bigger of the two at a header plus twenty-four bytes, and
/// only mice and keyboards are registered for. Fixed rather than asked for per
/// message: the two-call dance to learn a size that never changes would be two
/// system calls per keystroke.
const RAW_BYTES: usize = 128;

/// Start reading, and hand back the stream and the clock its stamps are on.
///
/// `None` when the thread cannot be started, which is the one failure visible from
/// here: everything the thread does after that — the window, the registration —
/// fails on the thread, and shows up as a stream that ended.
pub fn capture(config: Config, may_suppress: bool) -> Option<(Receiver<Captured>, Clock)> {
    let (events, captured) = mpsc::channel();
    let clock = Clock::start();
    let theirs = clock;
    if let Err(error) = std::thread::Builder::new()
        .name(String::from("favjit-capture"))
        .spawn(move || watch(config, may_suppress, theirs, events))
    {
        error!("cannot start the capture thread: {error}");
        return None;
    }
    Some((captured, clock))
}

/// Everything the capture thread does, start to finish.
fn watch(config: Config, may_suppress: bool, clock: Clock, events: Sender<Captured>) {
    let Some(window) = Window::open() else {
        return;
    };
    if !register(window.handle) {
        return;
    }
    // Said once the window and the registration are in place, because a run that
    // reports no input otherwise says nothing about which of the two it was: a
    // registration that failed, or a keyboard nobody touched.
    info!("reading raw input from the keyboards and mice");

    // Where the keyboard hook hands its keys, which is this thread: it is the one the
    // procedure is called on, and the one whose loop turns them into events.
    suppress::keys_go_to(window.handle);

    // Installed before the first event and left installed, whatever the link is
    // doing: what the mouse hook refuses is the state it reads. The keyboard has no
    // hook — refusing a key is the registration above.
    let hooks = match may_suppress {
        true => match suppress::Hooks::install() {
            Some(hooks) => Some(hooks),
            None => {
                // Ended rather than carried on: relaying without suppressing
                // types every keystroke on both machines, and a run asked to
                // suppress should not quietly become that run.
                error!("suppression was asked for and the mouse hook would not install; stopping");
                return;
            }
        },
        false => None,
    };

    // Read here rather than handed down, because this is the thread that has to look
    // at the probe pipe: the message loop below is the only thing that comes back
    // round often enough to answer one.
    let supervisor = Supervisor::from_env();
    if supervisor.watches_probes()
        && unsafe { SetTimer(window.handle, PROBE_TIMER, PROBE_TICK_MS, null_mut()) } == 0
    {
        // Carried on rather than ended: without the timer the probes go unanswered
        // and the watchdog ends this process, which is the safe direction — and
        // saying so is what tells that apart from a wedge.
        error!(
            "no timer to look for the watchdog's probes with ({}); it will end this process for a \
             silence that is this host's fault",
            last_error()
        );
    }

    let mut watching = Watching {
        config,
        clock,
        events,
        devices: HashMap::new(),
        keyboard: None,
        next: 1,
        gone: false,
        unreadable: false,
    };

    let mut message = Msg {
        window: null_mut(),
        message: 0,
        wparam: 0,
        lparam: 0,
        time: 0,
        point: Point::default(),
    };
    loop {
        // The hooks are called from inside this, which is why the thread that
        // installed them is this one.
        let got = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if got == -1 {
            error!(
                "the capture loop cannot read its messages: {}",
                last_error()
            );
            break;
        }
        if got == 0 {
            info!("the capture loop was asked to stop");
            break;
        }

        match message.message {
            WM_INPUT => watching.input(message.lparam as Handle),
            WM_INPUT_DEVICE_CHANGE => watching.changed(message.wparam, message.lparam as Handle),
            // One event per probe, so what the watchdog is owed and what the loop
            // answers are the same count: the ack is `core` calling back into this
            // host after handling it, which is the real path rather than a side
            // channel (ADR-0008).
            // A key, handed over by the hook that also decided whether this machine
            // gets it.
            WM_KEY => watching.keyboard(message.wparam, message.lparam),
            WM_TIMER if message.wparam == PROBE_TIMER => {
                for _ in 0..supervisor.take_probes() {
                    watching.send(EventKind::Probe);
                }
            }
            _ => {}
        }

        // Every message, raw input included: the window procedure is
        // `DefWindowProc`, and for raw input that is what releases what the
        // system allocated to deliver it.
        unsafe { DispatchMessageW(&message) };

        if watching.gone {
            info!("nothing is reading the captured input any more; stopping");
            break;
        }
    }

    // Both by going out of scope, in this order because the hooks are what is
    // taking input away: an unhook that waited for the window to close would
    // leave the keyboard refused for that long.
    drop(hooks);
    drop(window);
}

/// A window with no screen presence, for raw input to be delivered to.
struct Window {
    handle: Handle,
}

impl Window {
    fn open() -> Option<Self> {
        let class_name = wide("favjit-capture");
        let class = WindowClass {
            style: 0,
            procedure: Some(DefWindowProcW),
            class_extra: 0,
            window_extra: 0,
            instance: unsafe { GetModuleHandleW(null_mut()) },
            icon: null_mut(),
            cursor: null_mut(),
            background: null_mut(),
            menu_name: null_mut(),
            class_name: class_name.as_ptr(),
        };
        // The result is not read: a class that is already registered is the
        // ordinary case on a second call, and whether the class exists is what
        // creating the window answers.
        unsafe { RegisterClassW(&class) };

        let handle = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
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
        if handle.is_null() {
            error!("cannot make a window to receive input: {}", last_error());
            return None;
        }
        Some(Self { handle })
    }
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
/// its movement is a movement rather than a cursor position (ADR-0016).
fn register(window: Handle) -> bool {
    let mice = RawInputDevice {
        usage_page: USAGE_PAGE_GENERIC,
        usage: USAGE_MOUSE,
        flags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
        target: window,
    };
    let registered =
        unsafe { RegisterRawInputDevices(&mice, 1, size_of::<RawInputDevice>() as u32) };
    if registered == 0 {
        error!("cannot register for raw input: {}", last_error());
        return false;
    }
    true
}

/// One device this host is reading.
///
/// Only what Windows' own vocabulary needs remembered. What a device is holding
/// and whether the sink has heard of it are [`favjit_core::source`]'s, because
/// they are about what the sink is told rather than about what Windows said.
struct Device {
    id: DeviceId,
    pointing: Pointing,
}

/// The devices, and what has been said about them.
struct Watching {
    config: Config,
    clock: Clock,
    events: Sender<Captured>,
    /// The pointers, by the handle Windows names each with, as a number.
    devices: HashMap<usize, Device>,
    /// The one keyboard, once something has been typed on it.
    ///
    /// One for the machine rather than one per keyboard, because what reports a key here
    /// is a hook and a hook says which key and not which keyboard.
    keyboard: Option<DeviceId>,
    /// The next device number to hand out.
    ///
    /// From one, counting up. Nothing outside this host reads the value — and it
    /// must stay a small number, because the sink moves what arrives over the
    /// link into a range of its own by setting the top bit
    /// ([`favjit_core::link::from_source`]).
    next: u64,
    /// Whether the far end of the channel has gone.
    gone: bool,
    /// Whether a raw input that could not be read has already been reported.
    unreadable: bool,
}

impl Watching {
    fn send(&mut self, kind: EventKind) {
        let event = Captured::Event(HostEvent::new(self.clock.now(), kind));
        if self.events.send(event).is_err() {
            self.gone = true;
        }
    }

    /// One raw input message.
    fn input(&mut self, input: Handle) {
        let mut buffer = [0u8; RAW_BYTES];
        let mut size = RAW_BYTES as u32;
        let read = unsafe {
            GetRawInputData(
                input,
                RID_INPUT,
                buffer.as_mut_ptr().cast(),
                &mut size,
                size_of::<RawInputHeader>() as u32,
            )
        };
        if read == u32::MAX || (read as usize) < size_of::<RawInputHeader>() {
            // Once, not per event: whatever makes this fail is not going to stop,
            // and a line per keystroke would be the loudest thing in the log.
            if !self.unreadable {
                self.unreadable = true;
                warn!("cannot read a raw input: {}", last_error());
            }
            return;
        }

        let header: RawInputHeader = match read_from(&buffer) {
            Some(header) => header,
            None => return,
        };
        // Bounded by what was actually written rather than by the buffer, so that a
        // structure shorter than the one this expects is nothing rather than the
        // zeroes after it: a make code read out of untouched buffer is a key
        // nobody pressed.
        let end = (read as usize).min(buffer.len());
        let payload = &buffer[size_of::<RawInputHeader>()..end];
        // Only the mouse is registered for, so only the mouse arrives here. Matched
        // rather than assumed: a registration that grew a usage should not read one
        // structure as another.
        if header.kind == RIM_TYPEMOUSE {
            self.mouse(header.device, payload);
        }
    }

    /// One key, as the hook packed it into a message.
    ///
    /// From the hook and not from raw input, because refusing a key takes it away from
    /// raw input as well — so the only place a key can be both read and refused is the
    /// procedure that refuses it ([`crate::suppress`]).
    ///
    /// **One keyboard, unnamed.** A hook says which key and not which keyboard, so
    /// everything this machine forwards is attributed to one device with no vendor and
    /// no product. ADR-0003 allows for it, and `Config::ansi` is already a property of
    /// the machine rather than of a keyboard.
    fn keyboard(&mut self, packed: Wparam, flags: Lparam) {
        let make_code = (packed & 0xFFFF) as u16;
        let vkey = ((packed >> 16) & 0xFFFF) as u16;
        let read = scancode::pressed(
            scancode::from_a_hook(flags as u32, vkey),
            make_code,
            vkey,
            self.config.ansi,
        );
        if read == Pressed::NotAKey {
            // Before the keyboard is announced, so that a stream of filler events does
            // not announce one nothing was typed on.
            return;
        }

        let device = self.the_keyboard();
        match read {
            // Every press, the ones Windows makes up while a key is held
            // included: which of them is the key going down is
            // `favjit_core::source`'s, and so is a release with no press behind
            // it.
            Pressed::Down(key) => self.send(EventKind::KeyDown { device, key }),
            Pressed::Up(key) => self.send(EventKind::KeyUp { device, key }),
            Pressed::Unnamed { extended, code } => {
                let unknown = UnknownScanCode {
                    device,
                    extended,
                    code,
                };
                if self.events.send(Captured::Unknown(unknown)).is_err() {
                    self.gone = true;
                }
            }
            Pressed::NotAKey => {}
        }
    }

    fn mouse(&mut self, handle: Handle, payload: &[u8]) {
        let Some(raw) = read_from::<RawMouse>(payload) else {
            return;
        };
        let device = self.known(handle);
        let raw = crate::pointer::Raw {
            flags: raw.flags,
            button_flags: raw.button_flags,
            button_data: raw.button_data,
            dx: raw.x,
            dy: raw.y,
        };

        if raw.is_absolute() && self.events.send(Captured::Absolute(device)).is_err() {
            // Sent every time rather than once per device: saying it once is the
            // reader's to do, and it already does — a second record of which
            // devices have been mentioned would be the same list kept twice.
            self.gone = true;
        }

        let Some(watched) = self.devices.get_mut(&(handle as usize)) else {
            return;
        };
        // Every report, including one that says nothing new: which reports are
        // worth relaying is `favjit_core::source`'s.
        let report = watched.pointing.report(&raw);
        self.send(EventKind::Pointer { device, report });
    }

    /// This device's number, announcing it the first time it is seen.
    ///
    /// Devices are learned from their first event rather than from the arrival
    /// notification, because a keyboard plugged in mid-session is announced the
    /// moment it matters — when it is typed on — and one code path is easier to
    /// be sure of than two. The notification is still asked for, because the
    /// *removal* is not something an event can stand in for.
    /// The one keyboard this machine forwards, announced the first time it is typed on.
    ///
    /// One and not one per keyboard, because a hook says which key and not which
    /// keyboard. Announced from the first key rather than at startup so that a run
    /// nobody typed on tells the sink about nothing.
    fn the_keyboard(&mut self) -> DeviceId {
        if let Some(id) = self.keyboard {
            return id;
        }
        let id = DeviceId(self.next);
        self.next += 1;
        self.keyboard = Some(id);
        info!(
            "device {} is this machine's keyboards, with no vendor or product: a hook says \
             which key and not which keyboard, so no rule can single one of them out",
            id.0
        );
        self.send(EventKind::DeviceAttached(device::info(id, "")));
        id
    }

    /// This pointer's number, learned from its first report.
    ///
    /// Per device, unlike the keyboard: a pointer is read from raw input, which carries
    /// the device it came from.
    fn known(&mut self, handle: Handle) -> DeviceId {
        let key = handle as usize;
        if let Some(device) = self.devices.get(&key) {
            return device.id;
        }

        let named = path(handle);
        let id = DeviceId(self.next);
        self.next += 1;
        self.devices.insert(
            key,
            Device {
                id,
                pointing: Pointing::default(),
            },
        );

        match &named {
            Some(path) => info!("device {} is {path}", id.0),
            // The handle, because a device with no path is one the sink cannot
            // single out by a rule, and the handle is the only thing left that says
            // which device it was: zero is Windows saying the event came from none.
            None => info!("device {} has no path; its handle is {key:#x}", id.0),
        }
        // Not announced: the sink's device list is what its layout rules are read
        // against, and a pointer report carries no key for a rule to match.
        id
    }

    /// A device arriving or going away.
    ///
    /// Only the going away. Every departure is relayed, whether or not the sink
    /// has heard of the device: which of them the sink needs to be told about is
    /// `favjit_core::source`'s, which is the end that knows what it has said.
    fn changed(&mut self, what: Wparam, handle: Handle) {
        if what != GIDC_REMOVAL {
            return;
        }
        let Some(device) = self.devices.remove(&(handle as usize)) else {
            return;
        };
        // Without releasing its keys first, which is the shape of an unplugged
        // cable: the sink is what releases what it believes is held, because it is
        // the end that told the OS the key was down (ADR-0002).
        info!("device {} has gone", device.id.0);
        self.send(EventKind::DeviceDetached(device.id));
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

/// The device's interface path, which is where its vendor and product are.
fn path(handle: Handle) -> Option<String> {
    let mut size: u32 = 0;
    // With no buffer, this answers with how many characters — not bytes — the
    // path needs.
    unsafe { GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, null_mut(), &mut size) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize + 1];
    let written = unsafe {
        GetRawInputDeviceInfoW(
            handle,
            RIDI_DEVICENAME,
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if written == u32::MAX || written == 0 {
        return None;
    }
    let end = (written as usize).min(buffer.len());
    Some(String::from(
        String::from_utf16_lossy(&buffer[..end]).trim_end_matches('\0'),
    ))
}

/// Every keyboard and mouse attached, as `(is a keyboard, path)`.
///
/// For reporting rather than for capture: what a person needs in order to write
/// a vendor and product into a configuration is the list, and the list is a
/// separate question from what is being typed on.
pub fn attached() -> Vec<(bool, String)> {
    let mut count: u32 = 0;
    let size = size_of::<RawInputDeviceList>() as u32;
    unsafe { GetRawInputDeviceList(null_mut(), &mut count, size) };
    if count == 0 {
        return Vec::new();
    }
    let mut list = vec![
        RawInputDeviceList {
            device: null_mut(),
            kind: 0,
        };
        count as usize
    ];
    let read = unsafe { GetRawInputDeviceList(list.as_mut_ptr(), &mut count, size) };
    if read == u32::MAX {
        warn!("cannot list the input devices: {}", last_error());
        return Vec::new();
    }

    list.iter()
        .take(read as usize)
        .filter(|entry| entry.kind == RIM_TYPEKEYBOARD || entry.kind == RIM_TYPEMOUSE)
        .filter_map(|entry| path(entry.device).map(|path| (entry.kind == RIM_TYPEKEYBOARD, path)))
        .collect()
}

/// What a device at this path would be announced as.
///
/// Here so that the listing and the capture answer the same question with the
/// same code: a report that named a different vendor from the one a rule is
/// matched against would send somebody looking for a keyboard that is not there.
pub fn announced_as(path: &str) -> DeviceInfo {
    device::info(DeviceId(0), path)
}
