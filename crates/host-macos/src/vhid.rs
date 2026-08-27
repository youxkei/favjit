//! The virtual HID device service, spoken directly (ADR-0011).
//!
//! Two framings, one inside the other, and they disagree about byte order: the
//! transport's length and request id are big-endian, while the service's payloads
//! are C++ objects handed to `memcpy` and so are native-endian with their padding
//! part of the format. The sizes are `sizeof` on those types rather than a reading
//! of their fields — see `docs/platform/macos/virtual-hid-device.md`.
//!
//! Written here rather than bound to the driver's own header-only C++ client,
//! which would bring `asio`, `nod`, `type_safe` and `spdlog` into the build and a
//! C++ toolchain with them. What that client does that matters is in this file:
//! frame, heartbeat, answer a health check, and know when the device is ready.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use favjit_core::PointerReport;

use favjit_core::hid::report::{self, ControlPage, Report, Sent};

const SOCKET: &str = "/Library/Application Support/org.pqrs/tmp/rootonly/\
                      karabiner_virtual_hid_device_service.sock";

/// The version the daemon on this machine expects.
///
/// Pinned, and the pin is the whole of the version handling: a mismatch is not
/// refused and not reported — the connection stays open, status responses keep
/// arriving, and no virtual keyboard ever appears. So there is no error to check,
/// only readiness that never comes.
const CLIENT_PROTOCOL_VERSION: u16 = 7;

const HEARTBEAT: u8 = 0;
const HEALTH_CHECK: u8 = 2;
const HEALTH_CHECK_RESPONSE: u8 = 3;
const REQUEST: u8 = 4;
const RESPONSE: u8 = 5;

const VIRTUAL_HID_KEYBOARD_INITIALIZE: u8 = 0;
const VIRTUAL_HID_POINTING_INITIALIZE: u8 = 3;
const POST_KEYBOARD_INPUT_REPORT: u8 = 6;
const POST_CONSUMER_INPUT_REPORT: u8 = 7;
const POST_APPLE_VENDOR_KEYBOARD_INPUT_REPORT: u8 = 8;
const POST_APPLE_VENDOR_TOP_CASE_INPUT_REPORT: u8 = 9;
const POST_GENERIC_DESKTOP_INPUT_REPORT: u8 = 10;
const POST_POINTING_INPUT_REPORT: u8 = 11;

/// `us`, from HID's country code table.
const COUNTRY_CODE_US: u64 = 33;

/// The identity favjit gives the virtual keyboard it sends through.
///
/// Karabiner's defaults, and deliberately so: the keyboard type the OS gives the
/// resulting device is what a chord-resolving consumer reads back, and it does not
/// follow the country code, so there is nothing to gain by differing here.
///
/// **The device enumerates with these**, which is what lets capture leave it
/// alone: whoever initialises the virtual keyboard sets its vendor and product,
/// so ignoring these two numbers is ignoring exactly the device this process
/// sends through. Matching on the numbers someone else's client happened to use
/// would be matching an observation instead.
pub const VIRTUAL_KEYBOARD_VENDOR: u16 = 0x16c0;
pub const VIRTUAL_KEYBOARD_PRODUCT: u16 = 0x27db;

/// A write that cannot complete this quickly is a write that would stall the one
/// loop, and a stalled loop holding a seize is what ADR-0008 exists to prevent.
/// Losing a report is the lesser failure and it is reported.
const WRITE_TIMEOUT: Duration = Duration::from_millis(50);

/// What the service has told us about itself, kept as flags because that is all
/// it sends: pairs of (kind, value) bytes.
#[derive(Debug, Default)]
struct Status {
    driver_activated: AtomicBool,
    driver_connected: AtomicBool,
    version_mismatched: AtomicBool,
    keyboard_ready: AtomicBool,
    pointing_ready: AtomicBool,
}

/// A live connection to the service, with a virtual keyboard and pointing device.
pub struct VirtualDevice {
    writer: Arc<Mutex<UnixStream>>,
    status: Arc<Status>,
    connected: Arc<AtomicBool>,
    next_request_id: u64,
}

/// Why a connection could not be made.
#[derive(Debug)]
pub enum Unavailable {
    /// The socket refused a connection. Root-only by directory permission, so
    /// this is either the privilege or the package not being installed.
    Socket(std::io::Error),
    /// Connected, but no virtual keyboard arrived within the wait.
    NotReady {
        driver_activated: bool,
        driver_connected: bool,
        version_mismatched: bool,
    },
}

impl core::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Unavailable::Socket(error) => write!(
                f,
                "cannot reach the virtual HID device service ({error}); it needs root, and the \
                 Karabiner-DriverKit-VirtualHIDDevice package installed"
            ),
            Unavailable::NotReady {
                driver_activated,
                driver_connected,
                version_mismatched,
            } => write!(
                f,
                "the virtual HID device service never reported a ready keyboard \
                 (driver_activated={driver_activated} driver_connected={driver_connected} \
                 version_mismatched={version_mismatched})"
            ),
        }
    }
}

fn frame(message_type: u8, payload: &[u8]) -> Vec<u8> {
    let body_size = 1 + payload.len();
    let mut out = Vec::with_capacity(4 + body_size);
    out.extend_from_slice(&(body_size as u32).to_be_bytes());
    out.push(message_type);
    out.extend_from_slice(payload);
    out
}

impl VirtualDevice {
    /// Connect, ask for a keyboard and a pointing device, and wait for both.
    pub fn open(wait: Duration) -> Result<Self, Unavailable> {
        let stream = UnixStream::connect(SOCKET).map_err(Unavailable::Socket)?;
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

        let writer = Arc::new(Mutex::new(stream.try_clone().map_err(Unavailable::Socket)?));
        let status = Arc::new(Status::default());
        let connected = Arc::new(AtomicBool::new(true));

        Self::spawn_reader(&stream, &writer, &status, &connected).map_err(Unavailable::Socket)?;
        Self::spawn_heartbeat(&writer, &connected);

        let mut device = Self {
            writer,
            status,
            connected,
            next_request_id: 1,
        };

        let mut parameters = Vec::with_capacity(24);
        parameters.extend_from_slice(&u64::from(VIRTUAL_KEYBOARD_VENDOR).to_ne_bytes());
        parameters.extend_from_slice(&u64::from(VIRTUAL_KEYBOARD_PRODUCT).to_ne_bytes());
        parameters.extend_from_slice(&COUNTRY_CODE_US.to_ne_bytes());
        device.request(VIRTUAL_HID_KEYBOARD_INITIALIZE, &parameters);
        device.request(VIRTUAL_HID_POINTING_INITIALIZE, &[]);

        let deadline = std::time::Instant::now() + wait;
        while std::time::Instant::now() < deadline {
            if device.status.keyboard_ready.load(Ordering::SeqCst)
                && device.status.pointing_ready.load(Ordering::SeqCst)
            {
                return Ok(device);
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        Err(Unavailable::NotReady {
            driver_activated: device.status.driver_activated.load(Ordering::SeqCst),
            driver_connected: device.status.driver_connected.load(Ordering::SeqCst),
            version_mismatched: device.status.version_mismatched.load(Ordering::SeqCst),
        })
    }

    /// A thread of its own, because the service speaks out of band as well as in
    /// band: readiness arrives both as the response to our request and as a
    /// request it makes of us, and a health check has to be answered whatever
    /// else the loop is doing.
    fn spawn_reader(
        stream: &UnixStream,
        writer: &Arc<Mutex<UnixStream>>,
        status: &Arc<Status>,
        connected: &Arc<AtomicBool>,
    ) -> std::io::Result<()> {
        let mut reader = stream.try_clone()?;
        let writer = Arc::clone(writer);
        let status = Arc::clone(status);
        let connected = Arc::clone(connected);

        std::thread::spawn(move || {
            loop {
                let mut header = [0u8; 4];
                if reader.read_exact(&mut header).is_err() {
                    break;
                }
                let body_size = u32::from_be_bytes(header) as usize;
                let mut body = vec![0u8; body_size];
                if body_size > 0 && reader.read_exact(&mut body).is_err() {
                    break;
                }
                let Some(&message_type) = body.first() else {
                    continue;
                };

                match message_type {
                    HEALTH_CHECK => {
                        let reply = frame(HEALTH_CHECK_RESPONSE, &[]);
                        let _ = writer.lock().unwrap().write_all(&reply);
                    }
                    REQUEST | RESPONSE if body.len() >= 9 => {
                        for pair in body[9..].chunks(2) {
                            let [kind, value] = pair else { break };
                            let value = *value != 0;
                            let flag = match kind {
                                1 => &status.driver_activated,
                                2 => &status.driver_connected,
                                3 => &status.version_mismatched,
                                4 => &status.keyboard_ready,
                                5 => &status.pointing_ready,
                                _ => continue,
                            };
                            flag.store(value, Ordering::SeqCst);
                        }
                        // A request expects a response even when there is nothing
                        // to say: the transport pairs them by id, and one left
                        // unanswered is one the service waits on.
                        if message_type == REQUEST {
                            let mut out = Vec::with_capacity(13);
                            out.extend_from_slice(&9u32.to_be_bytes());
                            out.push(RESPONSE);
                            out.extend_from_slice(&body[1..9]);
                            let _ = writer.lock().unwrap().write_all(&out);
                        }
                    }
                    _ => {}
                }
            }
            connected.store(false, Ordering::SeqCst);
        });
        Ok(())
    }

    /// Every three seconds, as the library's own peers do. Posting reports is no
    /// substitute: a session where nothing is typed would go quiet and be torn
    /// down for it.
    fn spawn_heartbeat(writer: &Arc<Mutex<UnixStream>>, connected: &Arc<AtomicBool>) {
        let writer = Arc::clone(writer);
        let connected = Arc::clone(connected);
        std::thread::spawn(move || {
            while connected.load(Ordering::SeqCst) {
                let beat = frame(HEARTBEAT, &[]);
                if writer.lock().unwrap().write_all(&beat).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    }

    fn request(&mut self, request: u8, payload: &[u8]) -> bool {
        let mut body = Vec::with_capacity(3 + payload.len());
        body.extend_from_slice(&CLIENT_PROTOCOL_VERSION.to_ne_bytes());
        body.push(request);
        body.extend_from_slice(payload);

        let body_size = 1 + 8 + body.len();
        let mut out = Vec::with_capacity(4 + body_size);
        out.extend_from_slice(&(body_size as u32).to_be_bytes());
        out.push(REQUEST);
        out.extend_from_slice(&self.next_request_id.to_be_bytes());
        out.extend_from_slice(&body);
        self.next_request_id += 1;

        self.writer.lock().unwrap().write_all(&out).is_ok()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn send_keyboard(&mut self, report: Report) -> bool {
        self.request(POST_KEYBOARD_INPUT_REPORT, &report.bytes())
    }

    /// One report, on the request that belongs to its page.
    ///
    /// The page decides the request as well as the report id, and the two orders
    /// are not the same — the top case's report is posted by the later request and
    /// carries the lower id — so this pairing cannot be inferred from either
    /// number (`docs/platform/macos/virtual-hid-device.md`).
    pub fn send(&mut self, report: Sent) -> bool {
        match report {
            Sent::Keyboard(report) => self.send_keyboard(report),
            Sent::Control(report) => {
                let request = match report.page {
                    ControlPage::Consumer => POST_CONSUMER_INPUT_REPORT,
                    ControlPage::AppleVendorTopCase => POST_APPLE_VENDOR_TOP_CASE_INPUT_REPORT,
                    ControlPage::AppleVendorKeyboard => POST_APPLE_VENDOR_KEYBOARD_INPUT_REPORT,
                    ControlPage::GenericDesktop => POST_GENERIC_DESKTOP_INPUT_REPORT,
                };
                self.request(request, &report.bytes())
            }
        }
    }

    /// One pointing report: buttons as a native-endian `u32`, then x, y and the
    /// two wheels as signed bytes.
    ///
    /// The deltas saturate rather than wrap, because a byte is the whole of the
    /// field: a movement of 200 points arriving as -56 would send the cursor the
    /// other way.
    pub fn send_pointer(&mut self, report: PointerReport) -> bool {
        self.request(POST_POINTING_INPUT_REPORT, &report::pointing(report))
    }
}

impl Drop for VirtualDevice {
    /// Say that nothing is down, on the way out.
    ///
    /// The device outlives this process — it belongs to the driver — so a modifier
    /// left down in the last report stays down for whatever runs next. Terminating
    /// the device instead would take it away from any other client also using it.
    fn drop(&mut self) {
        self.send_keyboard(Report::default());
        self.send_pointer(PointerReport::default());
    }
}
