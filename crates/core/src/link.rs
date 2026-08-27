//! What crosses the link between the two machines (ADR-0017).
//!
//! What the bytes mean, and everything the two machines have to agree on about
//! them: the pattern their session runs in, the length of each message, the name
//! the source looks for. Stated once here rather than by each host, because a copy
//! that disagrees is not an error but a read waiting for bytes nobody will send
//! (ADR-0017).
//!
//! The calls that put those bytes on a socket are the hosts', and the order over
//! them is [`serve`]'s — which is what lets the end-to-end suite drive a source
//! into a sink with no network in the way (ADR-0006).
//!
//! Timestamps do not cross. The two machines have their own clocks, and a
//! difference between them is not something either can measure — so the sink
//! stamps a message with its own arrival time, and every rule that reads time
//! reads one clock (ADR-0010).

use crate::pairing::Authorized;
use crate::{DeviceId, DeviceInfo, EventKind, Key, PointerReport};

/// One message, as bytes on the wire.
///
/// Every message is exactly this long, and the length is not carried. A reader
/// takes a fixed number of bytes and decodes them, so a stream cannot desync into
/// reading a payload as a length — which is the failure a length prefix invites
/// and the one that is hardest to see afterwards, since the bytes still decode
/// into *something*.
pub const FRAME: usize = 32;

/// The Noise handshake and cipher suite both ends name.
///
/// Written out in full rather than assembled from parts: the two machines have to
/// say the same string, and a mismatch is a handshake that fails with nothing to
/// point at. Here rather than in a host for the same reason the frame is — it is
/// what the two ends agree on, and an agreement stated twice is two agreements.
pub const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// One frame on the wire, sealed.
///
/// The plaintext is a fixed [`FRAME`] and the cipher suite adds a 16-byte tag, so
/// every record is this long and none carries a length. Reading exactly this many
/// bytes is the whole of the framing.
pub const SEALED: usize = FRAME + 16;

/// The source's first handshake message, and the sink's answer to it.
///
/// Fixed for the same reason the records are: each is the same every time, so the
/// end reading one takes exactly this many bytes rather than whatever a single read
/// returned — which over a socket is not the message but a piece of it.
pub const HANDSHAKE: usize = 96;
pub const ANSWER: usize = 48;

/// The service the source looks for.
///
/// A name of favjit's own rather than something generic: what is on the other end
/// of this is a keyboard, and a source that connected to the wrong service would be
/// typing into it.
pub const SERVICE: &str = "_favjit._tcp";

/// The service a machine showing a pairing code advertises.
///
/// **A second name rather than the one above**, so that the two things a sink offers
/// cannot be mistaken for each other. They are up at the same time on a converting
/// machine and neither is at a port anybody configured, so a source looking for one
/// name would take whichever answer arrived first — and an offer spoken at a listener
/// waiting for a handshake fails as a pairing attempt that says nothing about why.
/// With a name each, a source asks for the thing it is doing.
pub const PAIRING: &str = "_favjit-pair._tcp";

/// How long to wait on a peer that has stopped talking.
///
/// A read with no limit is a thread parked forever on a machine that was unplugged,
/// holding the connection the next one needs.
pub const IDLE: core::time::Duration = core::time::Duration::from_secs(30);

/// A message whose length is not the one both ends read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrongLength {
    pub what: &'static str,
    pub wrote: usize,
    pub expected: usize,
}

impl core::fmt::Display for WrongLength {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} is {} bytes, not the {} both ends read",
            self.what, self.wrote, self.expected
        )
    }
}

/// Refuse a message that is not the length stated here.
///
/// Checked rather than trusted, because the reader takes the constant's worth of
/// bytes and nothing tells it otherwise: a message written at some other length is
/// a read that waits for bytes nobody will send, which is a link that hangs rather
/// than one that fails.
pub fn exactly(what: &'static str, wrote: usize, expected: usize) -> Result<(), WrongLength> {
    if wrote == expected {
        return Ok(());
    }
    Err(WrongLength {
        what,
        wrote,
        expected,
    })
}

const KIND_ATTACHED: u8 = 1;
const KIND_DETACHED: u8 = 2;
const KIND_KEY_DOWN: u8 = 3;
const KIND_KEY_UP: u8 = 4;
const KIND_POINTER: u8 = 5;

/// Input the source observed, for the sink to convert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    DeviceAttached(DeviceInfo),
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
}

impl Message {
    /// What to send for an event the source's host produced, if anything.
    ///
    /// The match is exhaustive on purpose: a new kind of event has to be decided
    /// about here rather than falling through a catch-all into a link that quietly
    /// does not carry it.
    pub fn of(kind: EventKind) -> Option<Self> {
        match kind {
            EventKind::DeviceAttached(info) => Some(Self::DeviceAttached(info)),
            EventKind::DeviceDetached(id) => Some(Self::DeviceDetached(id)),
            EventKind::KeyDown { device, key } => Some(Self::KeyDown { device, key }),
            EventKind::KeyUp { device, key } => Some(Self::KeyUp { device, key }),
            EventKind::Pointer { device, report } => Some(Self::Pointer { device, report }),
            // The watchdog's question and this process's own wake-up are about the
            // machine they happened on. Relaying either would be telling the other
            // end about the state of this end.
            EventKind::Timer | EventKind::Probe => None,
        }
    }

    /// What the sink should treat this as, once it has stamped it.
    pub fn kind(self) -> EventKind {
        match self {
            Self::DeviceAttached(info) => EventKind::DeviceAttached(info),
            Self::DeviceDetached(id) => EventKind::DeviceDetached(id),
            Self::KeyDown { device, key } => EventKind::KeyDown { device, key },
            Self::KeyUp { device, key } => EventKind::KeyUp { device, key },
            Self::Pointer { device, report } => EventKind::Pointer { device, report },
        }
    }

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
                out[1..9].copy_from_slice(&info.id.0.to_le_bytes());
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
            KIND_ATTACHED => Some(Self::DeviceAttached(DeviceInfo {
                id: device,
                is_built_in: bytes[9] != 0,
                vendor_id: (bytes[10] != 0)
                    .then(|| u16::from_le_bytes(bytes[11..13].try_into().ok().unwrap_or([0; 2]))),
                product_id: (bytes[13] != 0)
                    .then(|| u16::from_le_bytes(bytes[14..16].try_into().ok().unwrap_or([0; 2]))),
            })),
            KIND_DETACHED => Some(Self::DeviceDetached(device)),
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
                    buttons: crate::Buttons::from_bits(u32::from_le_bytes(
                        bytes[25..29].try_into().ok()?,
                    )),
                },
            }),
            _ => None,
        }
    }
}

/// The sink's end of the link, as everything impure about it.
///
/// The operations are the smallest the sequence below needs, and every one of them
/// is something only a platform can do: wait for a peer, read the list off disk,
/// take the next frame, hand an event to the converter. What order they happen in
/// — and in particular that nothing is read from a peer before it has been
/// authorised — is [`serve`]'s, so the suite can check it (ADR-0006).
pub trait LinkHost {
    /// Say on the local network that this machine is here, and can be reached at
    /// whatever the socket got.
    ///
    /// An operation of this object rather than of the machine, so that the
    /// advertisement is withdrawn with the link: one that outlived the socket would
    /// send the source to a port nothing is on, which looks from that end exactly
    /// like a machine refusing input.
    fn advertise(&mut self) -> bool;

    /// Wait for a connection.
    ///
    /// One operation, and its outcome reported rather than acted on. Every
    /// implementation of this trait is a call into the platform and the answer it
    /// gave: what to do about the answer is [`serve`]'s, because a decision inside
    /// a host is one nothing can check.
    fn accept(&mut self) -> Accepted;

    /// The source's first message, exactly [`HANDSHAKE`] bytes of it.
    fn take_handshake(&mut self) -> Option<[u8; HANDSHAKE]>;

    /// Open it, and produce what to send back.
    ///
    /// The handshake in progress stays on the platform's side of this, because what
    /// it is made of is that platform's cryptography. What crosses is bytes.
    fn answer(&mut self, first: &[u8; HANDSHAKE]) -> Option<[u8; ANSWER]>;

    /// Send it.
    fn send_answer(&mut self, answer: &[u8; ANSWER]) -> bool;

    /// Whose key the finished handshake presented, and a session to read through.
    fn peer(&mut self) -> Option<Vec<u8>>;

    /// The keys this machine accepts input from, as they are now.
    fn authorized(&mut self) -> Authorized;

    /// The next sealed record from the peer, or that the session is over.
    fn take_record(&mut self) -> Incoming;

    /// The frame inside one, or nothing when it will not open.
    ///
    /// Nothing rather than which way it failed: a record that will not open and one
    /// that opens to something other than a frame are the same answer here, and
    /// what to do about it is [`serve`]'s.
    fn open(&mut self, record: &[u8; SEALED]) -> Option<[u8; FRAME]>;

    /// Put an event into the stream the converter reads, stamped with this
    /// machine's clock — the source's is not comparable to it (ADR-0010).
    ///
    /// `false` when there is nothing on the other end of it any more.
    fn deliver(&mut self, kind: EventKind) -> bool;

    /// Let the current connection go, saying why for the log.
    fn close(&mut self, reason: &str);
}

/// What waiting for a connection produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    Connected,
    /// Nothing usable, this time.
    ///
    /// A kind of its own rather than an error, because it says nothing about
    /// whether the next connection will work. Everything a host cannot rule out
    /// recovering from belongs here — a connection that went away before it could be
    /// taken, a call that was interrupted — since [`serve`] gives up on its own after
    /// [`FAILURES`] of them and a host that decided instead would be deciding it
    /// where nothing can drive it.
    Failed,
    /// The socket itself is unusable, so nothing more is coming.
    Done,
}

/// What a device from the other machine is called on this one.
///
/// Each machine numbers its own devices from wherever it likes, and the rules read
/// the number: the MacBook's built-in keyboard takes Dudrack's layers where an
/// external one takes the raw-JIS remaps. Two machines both numbering from one
/// would put a Windows keyboard on the built-in rules, so what arrives is moved
/// into a range of its own — the top bit, which nothing counting up from zero will
/// reach.
///
/// One session's device keeps one name for the whole session, because the sink's
/// held-key bookkeeping is per device: a key that went down under one name and
/// came up under another would stay down.
const REMOTE: u64 = 1 << 63;

/// What this machine calls a device the source calls `device`.
///
/// Public because it is the vocabulary either end has to speak to say anything
/// about a remote keyboard — a caller working the mapping out for itself would be
/// a second copy of it, and the two could disagree.
pub fn from_source(device: DeviceId) -> DeviceId {
    DeviceId(device.0 | REMOTE)
}

/// Whether this is a device at the other end of the link.
///
/// What it is for is rules about the forwarded keyboard as opposed to the ones
/// attached here ([`crate::Scope::Forwarded`]): the two are different keyboards, one
/// at the other machine and one under the person's other hand, and a rule about one
/// is rarely a rule about the other.
///
/// Read off the number rather than from a flag on the device, because the number is
/// what every event carries and a flag would be a second thing that could disagree
/// with it.
pub fn is_from_source(device: DeviceId) -> bool {
    device.0 & REMOTE != 0
}

/// The same, for a whole message.
fn from_the_peer(message: Message) -> EventKind {
    match message {
        Message::DeviceAttached(info) => EventKind::DeviceAttached(DeviceInfo {
            id: from_source(info.id),
            ..info
        }),
        Message::DeviceDetached(id) => EventKind::DeviceDetached(from_source(id)),
        Message::KeyDown { device, key } => EventKind::KeyDown {
            device: from_source(device),
            key,
        },
        Message::KeyUp { device, key } => EventKind::KeyUp {
            device: from_source(device),
            key,
        },
        Message::Pointer { device, report } => EventKind::Pointer {
            device: from_source(device),
            report,
        },
    }
}

/// What reading from the peer produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incoming {
    Record([u8; SEALED]),
    /// The peer has gone.
    Ended,
}

/// The four steps of the handshake, in the only order they work in.
///
/// Each stops the sequence where it failed rather than carrying on: what a later
/// step would do with the result of one that did not happen is the failure this
/// order exists to make impossible.
fn shake_hands(host: &mut dyn LinkHost) -> Option<Vec<u8>> {
    let first = host.take_handshake()?;
    let answer = host.answer(&first)?;
    if !host.send_answer(&answer) {
        return None;
    }
    host.peer()
}

/// How many connections in a row a link will take and fail to use before it gives
/// up on the socket.
///
/// A count rather than a delay, because `core` has no clock (ADR-0010) — and a
/// bound of some kind there has to be: a link that carried on regardless would spin
/// at full speed on a socket that fails every call, holding the port with nothing
/// being served. One that gave up at the first failure would be a keyboard that
/// stops working because something scanned the machine, so a connection that becomes
/// a session sets the count back to zero.
pub const FAILURES: usize = 16;

/// Take input from one authorised source at a time, for as long as the host has
/// connections to give.
///
/// One source at a time because there is one set of hands: two feeding one
/// conversion pipeline would interleave their key state, and the pipeline's
/// held-key reasoning assumes they do not (ADR-0017).
///
/// Returning means the socket is not being served any more, whichever way it
/// happened, and the machine is told so by the loop it handed over coming back.
pub fn serve(host: &mut dyn LinkHost) {
    // Before the first connection is waited for, and carried on with either way: a
    // source that already knows the port can still get in, so a machine nobody can
    // find is worse than one nobody advertised only for the person setting it up.
    host.advertise();

    // Consecutive, so a link that works between failures never reaches the bound:
    // what [`FAILURES`] is for is a socket that has stopped answering, not a run
    // that has been going for a week.
    let mut failures = 0;

    loop {
        match host.accept() {
            Accepted::Connected => failures = 0,
            Accepted::Failed => {
                failures += 1;
                if failures >= FAILURES {
                    return;
                }
                continue;
            }
            Accepted::Done => return,
        }

        // The source speaks first in this pattern, so every step waits on the one
        // before it: an answer written before its own message was opened would be
        // an answer to nothing, and a key taken from a handshake the peer never
        // received would be a source this end believes is there.
        let Some(peer) = shake_hands(host) else {
            host.close("the handshake did not complete");
            continue;
        };

        if !host.authorized().holds(&peer) {
            // Before a single frame is read. Input that arrived and was then
            // discarded would already have been converted, and refusal is the
            // default ADR-0004 asks for rather than a filter applied afterwards.
            host.close("this machine has not paired that source");
            continue;
        }

        // The devices this session has mentioned, so the end of it can say they are
        // gone. A network that drops sends no detach, and the sink is what the OS
        // believes: a modifier that was down stays down in every application
        // otherwise, which is the failure ADR-0002 puts on the sink.
        let mut seen: Vec<DeviceId> = Vec::new();

        while let Incoming::Record(record) = host.take_record() {
            // A record that will not open ends the session for the same reason a
            // frame that will not decode does: what follows it comes from a stream
            // whose meaning is already in doubt.
            let Some(frame) = host.open(&record) else {
                host.close("a record this end cannot open");
                break;
            };
            let Some(message) = Message::decode(&frame) else {
                // Let go rather than skipped: the messages around one that could
                // not be read come from a stream whose meaning is already in
                // doubt, and a key down without its release is a held key.
                host.close("a frame this end cannot read");
                break;
            };

            let kind = from_the_peer(message);
            match kind {
                // A device the source says has gone is one the sink has already
                // released, so the end of the session has nothing left to say about
                // it — and saying it twice would be an event no hardware made.
                EventKind::DeviceDetached(id) => seen.retain(|known| *known != id),
                EventKind::DeviceAttached(DeviceInfo { id: device, .. })
                | EventKind::KeyDown { device, .. }
                | EventKind::KeyUp { device, .. }
                | EventKind::Pointer { device, .. } => {
                    if !seen.contains(&device) {
                        seen.push(device);
                    }
                }
                EventKind::Timer | EventKind::Probe => {}
            }

            if !host.deliver(kind) {
                // The converter has stopped, so there is nowhere for the next
                // keystroke to go — including the releases below, which is why
                // this ends the whole link rather than the session.
                host.close("the converter has stopped");
                return;
            }
        }

        // In the order they were first heard of, which is the order a person
        // plugged them in; the sink releases each one's keys in reverse of how they
        // went down, which is the part that matters.
        for device in seen {
            if !host.deliver(EventKind::DeviceDetached(device)) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Buttons;

    fn round_trip(message: Message) {
        let mut bytes = [0u8; FRAME];
        message.encode(&mut bytes);
        assert_eq!(Message::decode(&bytes), Some(message));
    }

    #[test]
    fn every_kind_survives_the_wire() {
        round_trip(Message::DeviceAttached(DeviceInfo::external(
            DeviceId(9),
            0x046d,
            0xc52b,
        )));
        round_trip(Message::DeviceAttached(DeviceInfo::built_in(DeviceId(1))));
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
        let zero = DeviceInfo {
            id: DeviceId(5),
            is_built_in: false,
            vendor_id: Some(0),
            product_id: Some(0),
        };
        round_trip(Message::DeviceAttached(zero));
        let absent = DeviceInfo {
            id: DeviceId(5),
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
    fn what_the_source_relays_is_input_and_only_input() {
        assert!(Message::of(EventKind::Probe).is_none());
        assert!(Message::of(EventKind::Timer).is_none());
        assert!(Message::of(EventKind::KeyDown {
            device: DeviceId(1),
            key: Key::A
        })
        .is_some());
    }

    #[test]
    fn a_message_and_the_event_it_becomes_say_the_same_thing() {
        // The relay is only faithful if these two are inverses: an event that
        // came back as a different kind would convert as something else.
        for kind in [
            EventKind::DeviceAttached(DeviceInfo::built_in(DeviceId(1))),
            EventKind::DeviceDetached(DeviceId(1)),
            EventKind::KeyDown {
                device: DeviceId(1),
                key: Key::Z,
            },
            EventKind::KeyUp {
                device: DeviceId(1),
                key: Key::Z,
            },
            EventKind::Pointer {
                device: DeviceId(1),
                report: PointerReport::moved(1, -1),
            },
        ] {
            assert_eq!(Message::of(kind).map(Message::kind), Some(kind));
        }
    }
}
