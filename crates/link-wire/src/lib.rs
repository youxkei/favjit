//! What crosses the link between the two machines, as bytes (ADR-0012).
//!
//! What the bytes mean and how long each message is — stated once here rather
//! than by each machine's `engine`, because a copy that disagrees is not an error
//! but a read waiting for bytes nobody will send. `host-sim` depends on this crate
//! directly, the same reason it depends on `favjit-pairing-exchange` and
//! `favjit-noise`: standing in for the machine on the other end of the link means
//! encoding and decoding the same frames a real one would (ADR-0005).
//!
//! What is not here is everything about *when* a message crosses, what a session
//! does with one, and how a device from the other machine is numbered on this
//! one: those are `engine::link`'s, over this format, reached from the entry
//! point the run came in through (ADR-0006).
//!
//! Timestamps do not cross. The two machines have their own clocks, and a
//! difference between them is not something either can measure — so the sink
//! stamps a message with its own arrival time, and every rule that reads time
//! reads one clock (ADR-0010).

use favjit_hid::{Buttons, Key, PointerReport};
use favjit_host::DeviceId;

pub use favjit_noise::FRAME;

/// One keyboard, as the machine relaying it read its own description of it.
///
/// Its own type here rather than the one a run holds: what a keyboard is to a
/// run is `engine`'s, and this crate sits beside `engine` rather than under it
/// (ADR-0005). What crosses is the fields, because both ends have to agree on
/// them byte for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attached {
    pub device: DeviceId,
    pub is_built_in: bool,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
}

const KIND_ATTACHED: u8 = 1;
const KIND_DETACHED: u8 = 2;
const KIND_KEY_DOWN: u8 = 3;
const KIND_KEY_UP: u8 = 4;
const KIND_POINTER: u8 = 5;

/// A frame that names no device and says nothing happened.
///
/// **The only frame that is not input**, and the link needs one: while the keyboard is
/// the source's own machine nothing crosses, and the sink drops a session it has heard
/// nothing on for [`IDLE`] — so a link kept across a trip of the keyboard has to be
/// heard from. What it costs is one frame every few seconds while nothing is being
/// typed at the other machine, and what it buys is the trip back being immediate: no
/// question asked of the network, no connection made, no handshake, and no keyboard
/// announced a second time.
const KIND_STILL_HERE: u8 = 6;

/// One of the forwarding machine's own trace records, on its way to the machine
/// that keeps the recording (ADR-0009).
const KIND_RECORDED: u8 = 7;

/// How wide one of those records is.
///
/// Stated here rather than where the records are made, because it is the frame
/// that has to be wide enough for one: a width named beside the recording and a
/// frame sized for it would be two copies of the same agreement, and the copy
/// that disagreed would be a record truncated on the wire rather than an error
/// anywhere.
pub const RECORDED: usize = 32;

/// Input the source observed, for the sink to convert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    DeviceAttached(Attached),
    DeviceDetached(DeviceId),
    KeyDown {
        device: DeviceId,
        key: Key,
    },
    KeyUp {
        device: DeviceId,
        key: Key,
    },
    Pointer {
        device: DeviceId,
        report: PointerReport,
    },
    /// Nothing happened, and this end is still here.
    ///
    /// Carries no device because there is none: it is not input, and the sink has
    /// nothing to do with it but count it as having been heard from. See
    /// [`KIND_STILL_HERE`] for why a link needs one at all.
    StillHere,
    /// One of the forwarding machine's own trace records, verbatim.
    ///
    /// Not input either, and the bytes are not read on the way: what a record
    /// means is the recording's business and this crate is the wire (ADR-0009).
    /// Every record that machine makes crosses, including the ones it made while
    /// there was no link to send them over — which is why they arrive in a run of
    /// them at the start of a session rather than one per keystroke.
    Recorded([u8; RECORDED]),
}

impl Message {
    /// Write it into a frame.
    ///
    /// Little-endian and byte by byte, rather than a derived serialisation: the
    /// layout is the interface between two machines, and a format nothing states
    /// outright is one that changes when a field is reordered.
    pub fn encode(self, out: &mut [u8; FRAME]) {
        *out = [0; FRAME];
        match self {
            Self::DeviceAttached(info) => {
                out[0] = KIND_ATTACHED;
                out[1..9].copy_from_slice(&info.device.0.to_le_bytes());
                out[9] = u8::from(info.is_built_in);
                // Absent is 0 and present is the number plus nothing, with a flag
                // byte of its own: a vendor id of zero is a real value on some
                // devices, so "zero means absent" would rename it.
                out[10] = u8::from(info.vendor_id.is_some());
                out[11..13].copy_from_slice(&info.vendor_id.unwrap_or(0).to_le_bytes());
                out[13] = u8::from(info.product_id.is_some());
                out[14..16].copy_from_slice(&info.product_id.unwrap_or(0).to_le_bytes());
            }
            Self::DeviceDetached(id) => {
                out[0] = KIND_DETACHED;
                out[1..9].copy_from_slice(&id.0.to_le_bytes());
            }
            Self::KeyDown { device, key } => {
                out[0] = KIND_KEY_DOWN;
                out[1..9].copy_from_slice(&device.0.to_le_bytes());
                out[9] = key.code();
            }
            Self::KeyUp { device, key } => {
                out[0] = KIND_KEY_UP;
                out[1..9].copy_from_slice(&device.0.to_le_bytes());
                out[9] = key.code();
            }
            Self::Pointer { device, report } => {
                out[0] = KIND_POINTER;
                out[1..9].copy_from_slice(&device.0.to_le_bytes());
                out[9..13].copy_from_slice(&report.dx.to_le_bytes());
                out[13..17].copy_from_slice(&report.dy.to_le_bytes());
                out[17..21].copy_from_slice(&report.vertical_wheel.to_le_bytes());
                out[21..25].copy_from_slice(&report.horizontal_wheel.to_le_bytes());
                out[25..29].copy_from_slice(&report.buttons.bits().to_le_bytes());
            }
            // The kind and nothing after it: the frame is a fixed length whatever it
            // carries, so this one is the byte that names it and zeroes.
            Self::StillHere => out[0] = KIND_STILL_HERE,
            Self::Recorded(record) => {
                out[0] = KIND_RECORDED;
                out[1..1 + RECORDED].copy_from_slice(&record);
            }
        }
    }

    /// Read one, or nothing if the frame says something this end does not know.
    ///
    /// `None` rather than a guess: the sink acts on what arrives, and a frame it
    /// cannot read is one it must not act on. A peer sending them is a peer to
    /// disconnect, which is the host's decision.
    pub fn decode(bytes: &[u8; FRAME]) -> Option<Self> {
        let device = DeviceId(u64::from_le_bytes(bytes[1..9].try_into().ok()?));
        match bytes[0] {
            KIND_ATTACHED => Some(Self::DeviceAttached(Attached {
                device,
                is_built_in: bytes[9] != 0,
                vendor_id: (bytes[10] != 0)
                    .then(|| u16::from_le_bytes(bytes[11..13].try_into().ok().unwrap_or([0; 2]))),
                product_id: (bytes[13] != 0)
                    .then(|| u16::from_le_bytes(bytes[14..16].try_into().ok().unwrap_or([0; 2]))),
            })),
            KIND_DETACHED => Some(Self::DeviceDetached(device)),
            KIND_STILL_HERE => Some(Self::StillHere),
            KIND_RECORDED => Some(Self::Recorded(bytes[1..1 + RECORDED].try_into().ok()?)),
            KIND_KEY_DOWN => Some(Self::KeyDown {
                device,
                key: Key::from_code(bytes[9])?,
            }),
            KIND_KEY_UP => Some(Self::KeyUp {
                device,
                key: Key::from_code(bytes[9])?,
            }),
            KIND_POINTER => Some(Self::Pointer {
                device,
                report: PointerReport {
                    dx: i32::from_le_bytes(bytes[9..13].try_into().ok()?),
                    dy: i32::from_le_bytes(bytes[13..17].try_into().ok()?),
                    vertical_wheel: i32::from_le_bytes(bytes[17..21].try_into().ok()?),
                    horizontal_wheel: i32::from_le_bytes(bytes[21..25].try_into().ok()?),
                    buttons: Buttons::from_bits(u32::from_le_bytes(bytes[25..29].try_into().ok()?)),
                },
            }),
            _ => None,
        }
    }

    /// Who this message is about, and whether it says that device is gone —
    /// without resolving what it says.
    ///
    /// Every message carries its device at the same offset, and resolving a key
    /// is a table `engine::link::serve` has no need of here: bookkeeping which
    /// devices a session has mentioned asks only who.
    ///
    /// Still `None` for a kind byte none of the kinds above declare: reading the
    /// device at a fixed offset regardless would answer for a frame that is not
    /// one of these messages at all, and the peer that sent it is the one
    /// `engine::link::serve` closes on rather than one this bookkeeping quietly
    /// went along with.
    pub fn seen_in(bytes: &[u8; FRAME]) -> Option<Seen> {
        match bytes[0] {
            KIND_ATTACHED | KIND_DETACHED | KIND_KEY_DOWN | KIND_KEY_UP | KIND_POINTER => {}
            // Read and named, so the session goes on: a frame this end could not read
            // ends it, and one that says nothing happened is not that. Nor is one
            // carrying the other machine's own recording, which names no device of
            // this session's — the ids inside it are that machine's own numbering.
            KIND_STILL_HERE | KIND_RECORDED => return Some(Seen::NoDevice),
            _ => return None,
        }
        let device = DeviceId(u64::from_le_bytes(bytes.get(1..9)?.try_into().ok()?));
        Some(match bytes[0] == KIND_DETACHED {
            true => Seen::Detached(device),
            false => Seen::Device(device),
        })
    }
}

/// What a frame says about the devices a session has mentioned.
///
/// Three answers and not a device with a flag, because the third is a frame that names
/// no device at all: the one saying nothing happened. Read as a device it would be
/// device zero, which the sink would then say had gone at the end of the session — a
/// keyboard nobody ever attached being released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// This device, which the session is now known to have mentioned.
    Device(DeviceId),
    /// This device, which the source says has gone.
    Detached(DeviceId),
    /// No device: the frame is not input.
    NoDevice,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: Message) {
        let mut bytes = [0u8; FRAME];
        message.encode(&mut bytes);
        assert_eq!(Message::decode(&bytes), Some(message));
    }

    fn external(device: DeviceId, vendor_id: u16, product_id: u16) -> Attached {
        Attached {
            device,
            is_built_in: false,
            vendor_id: Some(vendor_id),
            product_id: Some(product_id),
        }
    }

    fn built_in(device: DeviceId) -> Attached {
        Attached {
            device,
            is_built_in: true,
            vendor_id: None,
            product_id: None,
        }
    }

    #[test]
    fn every_kind_survives_the_wire() {
        round_trip(Message::DeviceAttached(external(
            DeviceId(9),
            0x046d,
            0xc52b,
        )));
        round_trip(Message::DeviceAttached(built_in(DeviceId(1))));
        round_trip(Message::DeviceDetached(DeviceId(4)));
        round_trip(Message::KeyDown {
            device: DeviceId(2),
            key: Key::International1,
        });
        round_trip(Message::KeyUp {
            device: DeviceId(2),
            key: Key::A,
        });
        round_trip(Message::Pointer {
            device: DeviceId(3),
            report: PointerReport {
                dx: -30000,
                dy: 30000,
                vertical_wheel: -3,
                horizontal_wheel: 2,
                buttons: Buttons::NONE.with(1).with(3),
            },
        });
    }

    #[test]
    fn a_vendor_id_of_zero_is_not_the_same_as_no_vendor_id() {
        // Some devices report zero, and a format that used zero for absent would
        // turn one into the other — which the sink's rules read.
        let zero = Attached {
            device: DeviceId(5),
            is_built_in: false,
            vendor_id: Some(0),
            product_id: Some(0),
        };
        round_trip(Message::DeviceAttached(zero));
        let absent = Attached {
            device: DeviceId(5),
            is_built_in: false,
            vendor_id: None,
            product_id: None,
        };
        let mut bytes = [0u8; FRAME];
        Message::DeviceAttached(zero).encode(&mut bytes);
        let mut other = [0u8; FRAME];
        Message::DeviceAttached(absent).encode(&mut other);
        assert_ne!(bytes, other);
    }

    #[test]
    fn a_frame_this_end_cannot_read_decodes_to_nothing() {
        // A zeroed frame is what a truncated read leaves behind, and an unknown
        // kind is what a newer peer sends. Neither may become an event.
        assert_eq!(Message::decode(&[0; FRAME]), None);
        let mut unknown = [0u8; FRAME];
        unknown[0] = 200;
        assert_eq!(Message::decode(&unknown), None);
        let mut no_such_key = [0u8; FRAME];
        no_such_key[0] = KIND_KEY_DOWN;
        no_such_key[9] = 250;
        assert_eq!(Message::decode(&no_such_key), None);
    }

    #[test]
    fn a_frame_this_end_cannot_read_is_not_seen_in_either() {
        // `seen_in` reads the same frame `decode` would refuse, ahead of
        // resolving what it says: the two have to agree on which frames are
        // nothing, or a peer sending an unknown kind would be bookkept as a
        // device this end never actually heard from.
        let mut unknown = [0u8; FRAME];
        unknown[0] = 200;
        assert_eq!(Message::seen_in(&unknown), None);
    }
}
