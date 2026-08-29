//! How the forwarding machine finds the machine it relays to (ADR-0017).
//!
//! The sink binds whatever port it is given and says where it is, so this is not a
//! name lookup: what comes back has to carry a port, and an answer without one is
//! not somewhere input can be sent. Which answers are the sink, how many are read
//! before giving up, and what a malformed one means are all here, where the suite
//! drives them; the socket is the host's ([`Discovery`], ADR-0006).
//!
//! The format is here for the same reason [`crate::link`]'s is: it is what the two
//! machines agree on, one advertising and one asking, and an agreement stated twice
//! is two agreements. Nothing in here touches the network — a datagram arrives as
//! bytes and an address leaves as four of them.

/// A service name in the domain mDNS puts it in.
///
/// The `.local` is this lookup's own, since a domain is a property of how a name is
/// resolved rather than of what the two machines agree on. Which name is asked for is
/// the caller's: a sink offers the link and pairing under one each
/// ([`crate::link::SERVICE`], [`crate::link::PAIRING`]).
pub fn service(name: &str) -> String {
    format!("{name}.local")
}

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_SRV: u16 = 33;

/// `IN`, with the bit that asks for the answer to come back to the asking socket
/// rather than to the group.
const CLASS_IN_UNICAST: u16 = 0x8001;

/// A name is at most this long, and a message can hold at most this many
/// compression pointers before it is one that points at itself.
const NAME_LIMIT: usize = 255;
const JUMP_LIMIT: usize = 32;

/// Where the sink said it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The machine's own name, as the service record gives it.
    pub host: String,
    pub port: u16,
    /// The address, when the responder sent it along with the rest.
    ///
    /// Four bytes rather than an address type, so that nothing here reaches into
    /// the network's vocabulary: turning them into something to connect to is the
    /// host's. Optional because a responder is only obliged to answer what was
    /// asked, and what was asked is the service.
    pub address: Option<[u8; 4]>,
}

/// The calls a machine makes to look for the sink.
///
/// Two, and each is one call into the platform and the answer it gave: put a
/// question on the network, and take the next thing that came back. What the
/// answers mean and when to stop reading them is [`look`]'s.
pub trait Discovery {
    /// Put the question on the local network.
    ///
    /// `false` when it could not be sent, which ends the search: an answer to a
    /// question nobody asked is whatever was already passing.
    fn ask(&mut self, question: &[u8]) -> bool;

    /// The next answer, or nothing once no more are coming.
    ///
    /// How long to wait for one is the host's, because waiting needs a clock and
    /// `core` has none.
    fn next_answer(&mut self) -> Option<Vec<u8>>;
}

/// Look for a machine offering `name`, and say where it is.
///
/// `None` when nothing usable answered, which is the ordinary state of a machine
/// that has not been switched on yet rather than a failure.
///
/// Every answer is read until one is the service asked for, rather than the first
/// being taken as the answer: a desk on an office network hears printers and AirPlay
/// receivers too, and giving up on the first of those would be giving up on the sink.
/// The name is the caller's, since the sink offers the link and pairing under one
/// each and what is being looked for is what the caller is about to do.
pub fn look(discovery: &mut dyn Discovery, name: &str) -> Option<Found> {
    let looking_for = service(name);
    if !discovery.ask(&question(&looking_for)) {
        return None;
    }
    while let Some(message) = discovery.next_answer() {
        if let Some(found) = answer(&message, &looking_for) {
            return Some(found);
        }
    }
    None
}

/// A query for every instance of `service`.
pub fn question(service: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    // The identifier is nothing: a one-shot query has one answer outstanding and
    // the socket it came back on is what matches them up.
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    write_name(&mut out, service);
    out.extend_from_slice(&TYPE_PTR.to_be_bytes());
    out.extend_from_slice(&CLASS_IN_UNICAST.to_be_bytes());
    out
}

/// Write a name as the labels a message carries it as.
///
/// Public because a machine standing in for a responder has to write the same
/// names this reads, and two spellings of the format would be two formats.
pub fn write_name(out: &mut Vec<u8>, name: &str) {
    for label in name.trim_end_matches('.').split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// What one answer says about `service`, if it says anything.
///
/// Every section is read, not only the answers: a responder that has the service
/// puts the service record and the address in the additional records, which is
/// what makes one question enough.
pub fn answer(message: &[u8], service: &str) -> Option<Found> {
    let records = read(message)?;
    let service = service.trim_end_matches('.').to_ascii_lowercase();

    // The instance named by a pointer to the service, and failing that any service
    // record inside it: a responder answering a question it was not asked is
    // within its rights, and a service record with no pointer beside it still says
    // where favjit is.
    let instance = records
        .iter()
        .find_map(|record| match &record.data {
            Data::Pointer(target) if record.name == service => Some(target.clone()),
            _ => None,
        })
        .or_else(|| {
            records.iter().find_map(|record| match record.data {
                Data::Service { .. } if record.name.ends_with(&service) => {
                    Some(record.name.clone())
                }
                _ => None,
            })
        })?;

    // Nothing without this: an instance named and no port is a machine to connect
    // to on a port nobody said.
    let (port, host) = records.iter().find_map(|record| match &record.data {
        Data::Service { port, target } if record.name == instance => Some((*port, target.clone())),
        _ => None,
    })?;

    let address = records.iter().find_map(|record| match record.data {
        Data::Address(address) if record.name == host => Some(address),
        _ => None,
    });

    Some(Found {
        host,
        port,
        address,
    })
}

/// One record, as much of it as this end reads.
#[derive(Debug)]
struct Record {
    /// Lower-cased, because names are compared that way and a responder chooses
    /// its own capitalisation.
    name: String,
    data: Data,
}

#[derive(Debug)]
enum Data {
    Pointer(String),
    Service {
        port: u16,
        target: String,
    },
    Address([u8; 4]),
    /// A type this end has no use for. Kept as a record rather than skipped, so
    /// that a walk over the sections stays a walk over all of them.
    Other,
}

/// Every record in a message, or nothing if it is not one.
///
/// `None` rather than what could be salvaged: a message that stops making sense
/// part way through is one whose earlier records may have been read against the
/// wrong offsets, and this is where a connection that carries a keyboard comes
/// from.
fn read(message: &[u8]) -> Option<Vec<Record>> {
    // The header's counts: questions at the third word, then the three sections of
    // records, which are read as one because what section a record arrived in does
    // not change what it says.
    let questions = word(message, 4)?;
    let answers =
        u32::from(word(message, 6)?) + u32::from(word(message, 8)?) + u32::from(word(message, 10)?);

    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(message, at)?;
        // Type and class, which a question has and nothing here reads.
        at = at.checked_add(4)?;
    }

    let mut records = Vec::new();
    for _ in 0..answers {
        let (name, next) = name_at(message, at)?;
        at = next;
        let kind = word(message, at)?;
        // Class, time to live, then the length of what follows.
        let length = word(message, at + 8)? as usize;
        let data_at = at + 10;
        let data = message.get(data_at..data_at.checked_add(length)?)?;
        records.push(Record {
            name: name.to_ascii_lowercase(),
            data: match kind {
                TYPE_PTR => Data::Pointer(name_at(message, data_at)?.0.to_ascii_lowercase()),
                TYPE_SRV => Data::Service {
                    // Priority and weight come first, and with one instance of one
                    // service there is nothing to choose between.
                    port: word(data, 4)?,
                    target: name_at(message, data_at + 6)?.0.to_ascii_lowercase(),
                },
                TYPE_A => Data::Address(<[u8; 4]>::try_from(data.get(0..4)?).ok()?),
                _ => Data::Other,
            },
        });
        at = data_at + length;
    }
    Some(records)
}

/// The two bytes at `at`, as the big-endian number every field in a message is
/// written as.
fn word(message: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        message.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

/// The name at `at`, and where the thing after it starts.
///
/// A name in a message may be a pointer back to one written earlier, so reading it
/// and stepping over it are two different answers: the step is over the pointer,
/// not over what it points at.
fn name_at(message: &[u8], at: usize) -> Option<(String, usize)> {
    let after = skip_name(message, at)?;
    let mut name = String::new();
    let mut at = at;
    let mut jumps = 0;
    loop {
        let length = *message.get(at)? as usize;
        if length == 0 {
            return Some((name, after));
        }
        if length & 0xC0 == 0xC0 {
            // A pointer, and the only place a loop can come from. Bounded rather
            // than tracked: a message from the network decides how many of these
            // there are.
            jumps += 1;
            if jumps > JUMP_LIMIT {
                return None;
            }
            let low = *message.get(at + 1)? as usize;
            at = ((length & 0x3F) << 8) | low;
            continue;
        }
        let label = message.get(at + 1..at + 1 + length)?;
        if name.len() + label.len() + 1 > NAME_LIMIT {
            return None;
        }
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(core::str::from_utf8(label).ok()?);
        at += 1 + length;
    }
}

/// Where the field after the name at `at` starts.
fn skip_name(message: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let length = *message.get(at)? as usize;
        if length & 0xC0 == 0xC0 {
            // A pointer is the whole of the rest of the name, and it is two bytes
            // long however far away it points.
            return Some(at + 2);
        }
        if length == 0 {
            return Some(at + 1);
        }
        at = at.checked_add(1 + length)?;
        if at > message.len() {
            return None;
        }
    }
}
