//! What crosses the link between the two machines (ADR-0012).
//!
//! The bytes' own shape — [`Message`], [`FRAME`] and the rest — is
//! `favjit-link-wire`'s, and the Noise construction they travel sealed under is
//! `favjit-noise`'s, both re-exported below (ADR-0005): stated once where both
//! machines' `engine`s and `host-sim` can reach them, rather than by each host.
//!
//! What stays here is who is allowed in, when the list is read, what order the
//! handshake happens in, what becomes of a record that will not open, and how a
//! device from the other machine is numbered on this one — the order over the
//! format, [`serve`]'s, which is what lets the end-to-end suite drive a source
//! into a sink with no network in the way (ADR-0006).
//!
//! Timestamps do not cross. The two machines have their own clocks, and a
//! difference between them is not something either can measure — so the sink
//! stamps a message with its own arrival time, and every rule that reads time
//! reads one clock (ADR-0010).

pub use favjit_host::link::{Accepted, Incoming, LinkHost};
pub use favjit_link_wire::{Attached, Message, Seen};
pub use favjit_noise::{Identity, ANSWER, FRAME, HANDSHAKE, PATTERN, SEALED};

use crate::noise::{Responder, Session};
use crate::pairing::Authorized;
use crate::{DeviceId, EventKind};

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
/// `pub(crate)`: which id a remote keyboard ends up under never crosses the
/// host boundary, so nothing outside `engine` has a device to ask this about.
pub(crate) fn from_source(device: DeviceId) -> DeviceId {
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
pub(crate) fn is_from_source(device: DeviceId) -> bool {
    device.0 & REMOTE != 0
}

/// What to send for an event the source's host produced, if anything.
///
/// The match is exhaustive on purpose: a new kind of event has to be decided
/// about here rather than falling through a catch-all into a link that quietly
/// does not carry it. A free function rather than a method on [`Message`]: that
/// type is `favjit-link-wire`'s, and which of `engine`'s own events cross the
/// link is this crate's decision over it, not part of the wire format
/// (ADR-0005).
pub(crate) fn message_of(kind: EventKind) -> Option<Message> {
    match kind {
        EventKind::DeviceAttached(info) => Some(Message::DeviceAttached(Attached {
            device: info.id,
            is_built_in: info.is_built_in,
            vendor_id: info.vendor_id,
            product_id: info.product_id,
        })),
        EventKind::DeviceDetached(id) => Some(Message::DeviceDetached(id)),
        EventKind::KeyDown { device, key } => Some(Message::KeyDown { device, key }),
        EventKind::KeyUp { device, key } => Some(Message::KeyUp { device, key }),
        EventKind::Pointer { device, report } => Some(Message::Pointer { device, report }),
        // The watchdog's question, this process's own wake-up and an ask for the
        // keyboard are about the machine they happened on. Relaying any of them would
        // be telling the other end about the state of this end.
        EventKind::Timer | EventKind::Probe | EventKind::Asked(_) => None,
    }
}

/// What the sink should treat a message as, once it has stamped it.
///
/// Called from the sink's own loop once a [`favjit_host::EventKind::Record`] comes
/// off the stream, which is where every other raw signal is resolved too — a frame
/// relayed from the other machine reaches [`crate::sink::Sink`] no differently
/// from one read here (ADR-0006).
/// `None` for a message that is not input. One of them is: the frame the source sends
/// while the keyboard is its own machine's, which exists so that a link is kept rather
/// than rebuilt for each trip of the keyboard, and which the sink has nothing to do
/// about but have received.
pub(crate) fn from_the_peer(message: Message) -> Option<EventKind> {
    Some(match message {
        Message::DeviceAttached(info) => EventKind::DeviceAttached(crate::DeviceInfo {
            id: from_source(info.device),
            is_built_in: info.is_built_in,
            vendor_id: info.vendor_id,
            product_id: info.product_id,
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
        // Neither is input, so neither becomes a keystroke: one says the far end
        // is still there and the other carries that machine's own recording,
        // which the loop serving the link puts on the stream itself.
        Message::StillHere | Message::Recorded(_) => return None,
    })
}

/// Drives the responder's whole half of the handshake, so a host answers by
/// reading and writing bytes alone: the construction is `engine`'s to hold
/// (ADR-0006), and this is the one place it is held from the sink's side.
fn shake_hands(
    identity: &Identity,
    host: &mut dyn LinkHost,
    open: &mut dyn favjit_host::link::Talking,
) -> Option<(Vec<u8>, Session)> {
    let mut first = [0u8; HANDSHAKE];
    if !open.take_handshake(&mut first) {
        return None;
    }
    let mut responder = Responder::new(identity, host).ok()?;
    let answer = responder.answer(&first).ok()?;
    if !open.send_answer(&answer) || !open.flush_answer() {
        return None;
    }
    responder.done().ok()
}

/// How many connections in a row a link will take and fail to use before it gives
/// up on the socket.
///
/// A count rather than a delay, because `engine` has no clock (ADR-0010) — and a
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
/// held-key reasoning assumes they do not (ADR-0012).
///
/// Returning means the socket is not being served any more, whichever way it
/// happened, and the machine is told so by the loop it handed over coming back.
/// Say why this connection is over, and let it go.
///
/// One function for the pair because the two go together and the words are the
/// same words: a connection dropped in silence is a source refused for a
/// reason nobody can see, and a line about a connection still open would be a
/// line about nothing (ADR-0006).
fn give_up(
    host: &mut dyn LinkHost,
    mut open: Box<dyn favjit_host::link::Talking>,
    reason: Refused,
) {
    // On the stream before the words, because the stream is what a trace is made
    // of: a reason only spoken lands in a log the other machine's recording
    // cannot be read beside, and which end let go first is the question two
    // recordings exist to answer (ADR-0009).
    host.deliver(favjit_host::EventKind::LinkEnded { why: reason as u32 });
    host.warn(format_args!("let the connection go: {}", reason.said()));
    // Said before it goes, because letting it go is what closes it and a socket
    // already closed is one this end can no longer say anything about.
    open.sending_it_away();
    drop(open);
}

/// Why this end let a connection go.
///
/// A number rather than the sentence, because it goes on the stream and into a
/// fixed-width record: what a person reads is composed from it where the run's
/// other words are, and a trace read afterwards holds the number (ADR-0009).
///
/// The numbers are the record's format, so an existing one keeps its value and a
/// new reason takes the next: a recording written by one build and read by
/// another would otherwise name the wrong reason rather than failing to name
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Refused {
    /// The source's session ended with nothing wrong: the peer closed, or went
    /// quiet for longer than the read allows. One value because this end cannot
    /// tell those apart.
    Ended = 0,
    CannotConfigure = 1,
    HandshakeIncomplete = 2,
    NotPaired = 3,
    RecordWillNotOpen = 4,
    FrameWillNotRead = 5,
    ConverterStopped = 6,
}

impl Refused {
    /// What a person reads about it.
    pub fn said(self) -> &'static str {
        match self {
            Self::Ended => "the source's session is over",
            Self::CannotConfigure => "the connection could not be configured",
            Self::HandshakeIncomplete => "the handshake did not complete",
            Self::NotPaired => "this machine has not paired that source",
            Self::RecordWillNotOpen => "a record this end cannot open",
            Self::FrameWillNotRead => "a frame this end cannot read",
            Self::ConverterStopped => "the converter has stopped",
        }
    }

    /// The reason a number stands for, and nothing for one this build does not
    /// name.
    ///
    /// Nothing rather than a guess: a recording from a build that had a reason
    /// this one does not is better read as unnamed than as whichever reason
    /// happens to sit at that number here.
    pub fn from_number(why: u32) -> Option<Self> {
        Some(match why {
            0 => Self::Ended,
            1 => Self::CannotConfigure,
            2 => Self::HandshakeIncomplete,
            3 => Self::NotPaired,
            4 => Self::RecordWillNotOpen,
            5 => Self::FrameWillNotRead,
            6 => Self::ConverterStopped,
            _ => return None,
        })
    }
}

/// Whether a connection this end could not take says anything about the next
/// one.
///
/// Named kinds only, and everything else counted as the socket being unusable: a
/// kind this end cannot name might be one that never clears, and [`FAILURES`]
/// bounds how many in a row are taken before giving up — so the cost of being
/// wrong here is a link that ends a little early, never one that spins on a
/// socket forever. A connection that went away between arriving and being taken,
/// and a call cut short by a signal, are both what a machine on the same desk
/// being switched off looks like, and what a port scan produces.
fn recoverable(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind::{ConnectionAborted, ConnectionReset, Interrupted, WouldBlock};
    matches!(
        kind,
        ConnectionAborted | ConnectionReset | Interrupted | WouldBlock
    )
}

pub(crate) fn serve(identity: &Identity, host: &mut dyn LinkHost) {
    // Before the first connection is waited for, and carried on with either failure:
    // a source that already knows the port can still get in, so a machine nobody can
    // find is worse than one nobody advertised only for the person setting it up.
    if let Some(port) = host.listener_port() {
        host.advertise(SERVICE, port);
    }

    // Consecutive, so a link that works between failures never reaches the bound:
    // what [`FAILURES`] is for is a socket that has stopped answering, not a run
    // that has been going for a week.
    let mut failures = 0;

    loop {
        let mut open = match host.accept() {
            Accepted::Yes(open) => open,
            Accepted::No(kind) if recoverable(kind) => {
                failures += 1;
                if failures >= FAILURES {
                    return;
                }
                continue;
            }
            Accepted::No(_) => return,
            // `Accepted` is `#[non_exhaustive]`: a kind this crate does not yet
            // name is nothing usable this time, the same as a connection that
            // went away, until it is given a meaning of its own.
            _ => {
                failures += 1;
                if failures >= FAILURES {
                    return;
                }
                continue;
            }
        };
        if !open.set_read_timeout(IDLE) || !open.set_nodelay(true) {
            give_up(host, open, Refused::CannotConfigure);
            failures += 1;
            if failures >= FAILURES {
                return;
            }
            continue;
        }
        failures = 0;

        // The source speaks first in this pattern, so every step waits on the one,
        // before it: an answer written before its own message was opened would be
        // an answer to nothing, and a key taken from a handshake the peer never
        // received would be a source this end believes is there.
        let Some((peer, mut session)) = shake_hands(identity, host, open.as_mut()) else {
            give_up(host, open, Refused::HandshakeIncomplete);
            continue;
        };

        let authorized = host
            .authorized()
            .map(|text| Authorized::parse(&text))
            .unwrap_or_default();
        if !authorized.holds(&peer) {
            // Before a single frame is read. Input that arrived and was then
            // discarded would already have been converted, and refusal is the
            // default ADR-0004 asks for rather than a filter applied afterwards.
            give_up(host, open, Refused::NotPaired);
            continue;
        }

        // The devices this session has mentioned, so the end of it can say they are
        // gone. A network that drops sends no detach, and the sink is what the OS
        // believes: a modifier that was down stays down in every application
        // otherwise, which is the failure ADR-0002 puts on the sink.
        let mut seen: Vec<DeviceId> = Vec::new();

        // What this end judged about the session, for the one place below that says
        // how it ended. Carried rather than said where it is found, so that a session
        // is said to be over exactly once: the reasons here are the ones this end
        // decided, and the ordinary end is the one it did not decide anything about.
        let mut refused: Option<Refused> = None;

        let mut sealed = [0u8; SEALED];
        while let Incoming::Record = open.take_record(&mut sealed) {
            // A record that will not open ends the session for the same reason a
            // frame that will not decode does: what follows it comes from a stream
            // whose meaning is already in doubt.
            let Some((at, frame)) = session.open(&sealed) else {
                refused = Some(Refused::RecordWillNotOpen);
                break;
            };
            let Some(seen_in) = Message::seen_in(&frame) else {
                refused = Some(Refused::FrameWillNotRead);
                break;
            };
            // A device the source says has gone is one the sink has already
            // released, so the end of the session has nothing left to say about
            // it — and saying it twice would be an event no hardware made.
            match seen_in {
                Seen::Detached(device) => {
                    let device = from_source(device);
                    seen.retain(|known| *known != device);
                }
                Seen::Device(device) => {
                    let device = from_source(device);
                    if !seen.contains(&device) {
                        seen.push(device);
                    }
                }
                // Nothing happened, so nothing is delivered and nothing is
                // remembered: what it did is keep this read from being the silence
                // that ends the session, and that has already happened by getting
                // here. The source sends one while the keyboard is its own machine's
                // (ADR-0013), which is a link kept rather than one rebuilt per trip.
                Seen::NoDevice => {
                    // The other machine's own recording, put in this one as it
                    // wrote it: one region holds both, so what a person reads
                    // needs no second file and no second machine (ADR-0009).
                    if let Some(Message::Recorded(record)) = Message::decode(&frame) {
                        host.deliver(favjit_host::EventKind::Recorded(record.to_vec()));
                    }
                    continue;
                }
            }

            if !host.deliver(favjit_host::EventKind::Record {
                at,
                frame: frame.to_vec(),
            }) {
                // The converter has stopped, so there is nowhere for the next
                // keystroke to go — including the releases below, which is why
                // this ends the whole link rather than the session.
                give_up(host, open, Refused::ConverterStopped);
                return;
            }
        }

        // Said whichever way it ended, because the source says the same moment from
        // its own side and the two logs read together are what say which end let go
        // first — a question neither end can answer alone. The ordinary end has no
        // reason to give: the peer closing and the peer going quiet for longer than
        // the read allows reach this end as one answer, and nothing here can tell
        // them apart.
        // The ordinary end goes on the stream too, and not only the refusals: a
        // recording that held a reason for every way but the usual one would leave
        // the reader of a link that simply ended with nothing to read, which is
        // the case a person looking at a link that keeps dropping starts from
        // (ADR-0009).
        let reason = refused.unwrap_or(Refused::Ended);
        host.deliver(favjit_host::EventKind::LinkEnded { why: reason as u32 });
        match refused {
            Some(reason) => host.warn(format_args!("let the connection go: {}", reason.said())),
            None => host.warn(format_args!(
                "the source's session is over; everything it had down is released"
            )),
        }
        // Said only where this end refused it: an ordinary session ending is the
        // peer going, not this machine sending it away, and the two are what a
        // reader of a link that keeps dropping has to tell apart.
        if refused.is_some() {
            open.sending_it_away();
        }
        drop(open);

        // In the order they were first heard of, which is the order a person
        // plugged them in; the sink releases each one's keys in reverse of how they
        // went down, which is the part that matters.
        for device in seen {
            if !host.deliver(favjit_host::EventKind::DeviceLost(device)) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::external;
    use crate::{Buttons, DeviceId, Key, PointerReport};

    #[test]
    fn a_connection_that_went_away_says_nothing_about_the_socket() {
        // The one that decides whether a port scan ends the link: each of these
        // is a connection that stopped existing, not a socket that stopped
        // working.
        use std::io::ErrorKind;

        assert!(recoverable(ErrorKind::ConnectionAborted));
        assert!(recoverable(ErrorKind::ConnectionReset));
        assert!(recoverable(ErrorKind::Interrupted));
        assert!(recoverable(ErrorKind::WouldBlock));

        // And anything this end cannot name is counted as the socket, because a
        // kind that never clears would otherwise be a loop that never stops
        // trying.
        assert!(!recoverable(ErrorKind::PermissionDenied));
        assert!(!recoverable(ErrorKind::Other));
    }

    fn round_trip(message: Message) {
        let mut bytes = [0u8; FRAME];
        message.encode(&mut bytes);
        assert_eq!(Message::decode(&bytes), Some(message));
    }

    /// The same keyboard as [`external`], in the shape it crosses the link in.
    fn attached(device: DeviceId, vendor_id: u16, product_id: u16) -> Attached {
        Attached {
            device,
            is_built_in: false,
            vendor_id: Some(vendor_id),
            product_id: Some(product_id),
        }
    }

    #[test]
    fn every_kind_survives_the_wire() {
        round_trip(Message::DeviceAttached(attached(
            DeviceId(9),
            0x046d,
            0xc52b,
        )));
        round_trip(Message::DeviceAttached(Attached {
            device: DeviceId(1),
            is_built_in: true,
            vendor_id: None,
            product_id: None,
        }));
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
    fn what_the_source_relays_is_input_and_only_input() {
        assert!(message_of(EventKind::Probe).is_none());
        assert!(message_of(EventKind::Timer).is_none());
        assert!(message_of(EventKind::KeyDown {
            device: DeviceId(1),
            key: Key::A
        })
        .is_some());
    }

    #[test]
    fn from_the_peer_undoes_message_of_but_renumbers_the_device() {
        // The relay is only faithful if these agree on everything but the device:
        // a message that came back naming a different key or report would convert
        // as something else, and one that kept the source's own numbering would
        // collide with a device this machine found for itself.
        let sent = DeviceId(1);
        let received = from_source(sent);
        for (kind, expected) in [
            (
                EventKind::DeviceAttached(external(sent, 1, 2)),
                EventKind::DeviceAttached(external(received, 1, 2)),
            ),
            (
                EventKind::DeviceDetached(sent),
                EventKind::DeviceDetached(received),
            ),
            (
                EventKind::KeyDown {
                    device: sent,
                    key: Key::Z,
                },
                EventKind::KeyDown {
                    device: received,
                    key: Key::Z,
                },
            ),
            (
                EventKind::KeyUp {
                    device: sent,
                    key: Key::Z,
                },
                EventKind::KeyUp {
                    device: received,
                    key: Key::Z,
                },
            ),
            (
                EventKind::Pointer {
                    device: sent,
                    report: PointerReport::moved(1, -1),
                },
                EventKind::Pointer {
                    device: received,
                    report: PointerReport::moved(1, -1),
                },
            ),
        ] {
            let message = message_of(kind).expect("every kind above relays");
            assert_eq!(from_the_peer(message), Some(expected));
        }
    }
}
