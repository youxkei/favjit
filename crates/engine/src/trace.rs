//! The bounded recording a run can be replayed from (ADR-0009).
//!
//! Every inbound event, every outbound call with its result, and periodic
//! checkpoints of the sink's whole state. The vocabulary is the same
//! [`crate::HostEvent`] the simulated host is scripted with, so a recording loads
//! as a script and a field incident becomes a regression test.
//!
//! **The memory is the host's** ([`Trace::new`] takes a slice), not allocated
//! here: a shared region is an OS operation and `engine` reaching for one is what
//! ADR-0006 rules out. On a real host that region is one the watchdog can read, so
//! a trace survives the kill that ADR-0008 uses to give the keyboard back. Under
//! the simulator it is ordinary process memory, which is what keeps shared memory
//! out of the test path entirely.
//!
//! Records are one fixed size, and a checkpoint is a run of them rather than one
//! big one. That is what makes eviction index arithmetic and the watchdog's job a
//! copy: a variable-length encoding would need framing that something has to
//! parse, and the component that must stay trivial is the one reading it.
//!
//! **A trace of keystrokes is a keylog.** It holds whatever was typed in the
//! window it retains, passwords included. That is inherent — replaying a
//! conversion bug needs the actual keys — so nothing here writes to a file or
//! sends anything anywhere, and the region is not file-backed.

use crate::clock::Clock;
use crate::{
    Buttons, DeviceId, DeviceInfo, EventKind, HostEvent, Injected, Instant, Key, ModifierKeys,
    PointerReport,
};

/// One record's size.
///
/// Every record is this long whatever it carries, so the ring is an index and a
/// count. The largest payload is a held key mid-decision, which needs 22 bytes;
/// the rest is room for a record kind that has not been written yet.
///
/// Taken from the wire rather than stated here, because it is a frame that has to
/// be wide enough for one: the forwarding machine's records reach the converting
/// one over the link, and a width named here with a frame sized for it elsewhere
/// would be two copies of one agreement (ADR-0009).
pub const RECORD: usize = favjit_link_wire::RECORDED;

/// Bytes of header before the first record.
///
/// The count of evicted records lives here rather than being derivable, because a
/// reader has to be able to say "the start is missing" — a trace that looks
/// complete when it is not is worse than one that says so.
pub const HEADER: usize = 16;

const MAGIC: u32 = 0x66_61_76_6a; // "favj"

// Record kinds. Numbered rather than derived, so a reader written against one
// build can say what it does not understand instead of misreading it.
const KIND_CHECKPOINT_BEGIN: u8 = 1;
const KIND_CHECKPOINT_DEVICE: u8 = 2;
const KIND_CHECKPOINT_HELD: u8 = 3;
const KIND_EVENT_ATTACHED: u8 = 10;
const KIND_EVENT_DETACHED: u8 = 11;
const KIND_EVENT_KEY_DOWN: u8 = 12;
const KIND_EVENT_KEY_UP: u8 = 13;
const KIND_EVENT_POINTER: u8 = 14;
const KIND_EVENT_TIMER: u8 = 15;
const KIND_EVENT_PROBE: u8 = 16;
const KIND_EVENT_ASKED: u8 = 17;
const KIND_INJECT_KEY_DOWN: u8 = 20;
const KIND_INJECT_KEY_UP: u8 = 21;
const KIND_INJECT_POINTER: u8 = 22;
const KIND_INJECT_MODIFIERS: u8 = 24;
const KIND_SENT: u8 = 30;
const KIND_RECEIVED: u8 = 31;
const KIND_LINK_ENDED: u8 = 32;
const KIND_SERVED: u8 = 40;
const KIND_OUTPUT_ENDED: u8 = 41;

/// One frame a run put on the output device's connection of its own accord.
///
/// Named rather than left as the call that wrote it: the three are
/// indistinguishable in a recording otherwise, and telling a run that beat
/// steadily from one that only ever answered what it was asked is what says
/// whether the connection was being held up at all — the second holds it up for
/// no longer than the service keeps asking (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
#[non_exhaustive]
pub enum Served {
    Beat = 0,
    HealthCheckAnswer = 1,
    Acknowledgement = 2,
}

impl Served {
    /// What a person reads for it.
    pub fn said(self) -> &'static str {
        match self {
            Self::Beat => "a beat",
            Self::HealthCheckAnswer => "an answer to a health check",
            Self::Acknowledgement => "an acknowledgement",
        }
    }

    pub fn from_number(frame: u32) -> Option<Self> {
        match frame {
            0 => Some(Self::Beat),
            1 => Some(Self::HealthCheckAnswer),
            2 => Some(Self::Acknowledgement),
            _ => None,
        }
    }
}

/// Why the loop serving that connection came back.
///
/// Numbered for the reason [`crate::link::Refused`] is: what a person reads is
/// composed from the number, and the number is what survives into a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
#[non_exhaustive]
pub enum Lost {
    ReadsWillNotBound = 0,
    ServiceClosedIt = 1,
    BeatWillNotGo = 2,
    HealthCheckAnswerWillNotGo = 3,
    AcknowledgementWillNotGo = 4,
    FrameArrivedInPieces = 5,
}

impl Lost {
    pub fn said(self) -> &'static str {
        match self {
            Self::ReadsWillNotBound => "a read of it cannot be given a bound",
            Self::ServiceClosedIt => "the service closed it",
            Self::BeatWillNotGo => "a beat would not go",
            Self::HealthCheckAnswerWillNotGo => "an answer to a health check would not go",
            Self::AcknowledgementWillNotGo => "an acknowledgement would not go",
            Self::FrameArrivedInPieces => {
                "part of a frame arrived and the rest did not, so where the next one \
                 starts is no longer known"
            }
        }
    }

    pub fn from_number(why: u32) -> Option<Self> {
        match why {
            0 => Some(Self::ReadsWillNotBound),
            1 => Some(Self::ServiceClosedIt),
            2 => Some(Self::BeatWillNotGo),
            3 => Some(Self::HealthCheckAnswerWillNotGo),
            4 => Some(Self::AcknowledgementWillNotGo),
            5 => Some(Self::FrameArrivedInPieces),
            _ => None,
        }
    }
}

/// The bit a kind byte carries when the record came off the other machine.
///
/// A bit on the kind rather than a field of its own, because every record has a
/// kind and none of them needs 128 of them: a field would cost every record four
/// bytes to say something one bit says, on a format where the width is what
/// decides how much history a region holds.
const FROM_THE_SOURCE: u8 = 0x80;

/// What a record says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Record {
    /// The start of a segment: the sink's state, as the records that follow.
    CheckpointBegin {
        pointer_buttons: Buttons,
        repeating: Option<(DeviceId, Key, Instant)>,
    },
    /// One keyboard the sink knew about.
    CheckpointDevice(DeviceInfo),
    /// One key the sink had down, and what became of it.
    CheckpointHeld {
        device: DeviceId,
        key: Key,
        state: HeldRecord,
    },
    /// Something that happened outside.
    Event(HostEvent),
    /// Something the sink asked the host to do, and what became of it.
    ///
    /// Two results and not one: `rendered` is whether the run could turn it into
    /// a report at all, and `code` is what the machine said when that report was
    /// written — `0` for the write that went. One field holding both would show a
    /// keystroke as delivered on a machine that took none of it, which is the
    /// reading a trace exists for (ADR-0009).
    Injected {
        injected: Injected,
        rendered: bool,
        code: i32,
    },
    /// One sealed record the source handed its link, and what the machine said.
    ///
    /// `code` is `0` for the write that went, and the number rather than whether
    /// it went: a link that drops is looked for again from the start, and what
    /// wants explaining afterwards is which failure it was.
    ///
    /// `at` is the number the session sent it under, which is what the receiving
    /// end's [`Record::Received`] carries for the same record: the transport's own
    /// count rather than a field favjit put on the wire, so neither end can
    /// disagree about it without the record failing to open at all. It is what
    /// names one record in two places, on a reading that holds both machines'
    /// (ADR-0009).
    Sent { at: u64, code: i32 },
    /// The loop serving the link let the connection go, for this reason
    /// ([`crate::link::Refused`] numbers them).
    ///
    /// The one record that says this end let go: on a reading that holds both
    /// machines', a link that dropped either has this or has a [`Record::Sent`]
    /// carrying a write that failed on the other one (ADR-0009).
    LinkEnded { why: u32 },
    /// One frame the loop serving the output device put on its connection, and
    /// what the machine said — `0` for the write that went.
    ///
    /// Which frame it was and not only that one went: a run that beat steadily
    /// and a run that only ever answered what it was asked are the same count of
    /// frames and different states, and the second one holds the connection up
    /// for no longer than the service keeps asking (ADR-0009).
    Served { frame: Served, code: i32 },
    /// The loop serving the output device came back, for this reason
    /// ([`crate::output::Lost`] numbers them).
    ///
    /// The record that separates them: from the injections alone every one of
    /// those reasons is a run writing reports successfully until it stops.
    OutputEnded { why: u32 },
    /// One sealed record taken off the link, at the number it came in under.
    ///
    /// The counterpart of [`Record::Sent`]: what arrived is already on the stream
    /// as an event, and what this adds is which of the sender's records it was.
    Received { at: u64 },
}

/// What a held key had resolved to, in the shape a checkpoint stores.
///
/// Its own type rather than the sink's, because the sink's is private and this one
/// is a wire format: the two are allowed to drift only through this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldRecord {
    Down {
        key: Key,
        modifiers: ModifierKeys,
    },
    Swallowed,
    Henkan,
    Undecided {
        hold: Key,
        tap: Key,
        deadline: Instant,
        sent: Option<ModifierKeys>,
        alone: bool,
    },
}

/// Where a record carries a set of modifier keys.
///
/// The last eight bytes, which only an event's timestamp uses — and no record
/// carries both. The two bytes at 4 are the modifier byte's obvious home and are a
/// button mask on a pointer record, so a set that outgrew one byte cannot live
/// there without one kind of record reading another's field.
const MODIFIER_KEYS: core::ops::Range<usize> = 24..26;

/// A recording, in memory the host owns.
pub struct Trace<'a> {
    memory: &'a mut [u8],
    /// Where the next record goes, counted in records from the first.
    next: usize,
    /// How many records have been dropped from the front.
    evicted: u64,
    /// Where the oldest retained record is, counted in records.
    first: usize,
    /// How many records are in use.
    used: usize,
    /// How many of the records written here have been handed to the link.
    ///
    /// Counted in records ever written rather than in slots, so an eviction
    /// cannot make it point at something else: a run whose link was down long
    /// enough loses the oldest of what it was holding, and what it then sends is
    /// what is left rather than a slot's worth of whatever overwrote it.
    sent: u64,
}

impl<'a> Trace<'a> {
    /// Take over a region and start a recording in it.
    ///
    /// `pub(crate)` and not `pub`: [`sink::run`](crate::sink::run) is what wraps a
    /// host's region in one of these, from the slice `run` itself is handed —
    /// nothing outside `sink` constructs a `Trace` to write, and a caller that did
    /// would be a second place deciding when a segment is checkpointed.
    pub(crate) fn new(memory: &'a mut [u8]) -> Self {
        let capacity = capacity_of(memory.len());
        let header = HEADER.min(memory.len());
        memory[..header].fill(0);
        if memory.len() >= HEADER {
            memory[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        }
        let mut trace = Self {
            memory,
            next: 0,
            evicted: 0,
            first: 0,
            used: 0,
            sent: 0,
        };
        debug_assert!(capacity > 0, "a trace needs room for at least one record");
        trace.write_header();
        trace
    }

    /// How many records fit.
    pub(crate) fn capacity(&self) -> usize {
        capacity_of(self.memory.len())
    }

    /// Append one record, evicting whole segments if there is no room.
    pub(crate) fn push(&mut self, record: Record) {
        let capacity = self.capacity();
        if capacity == 0 {
            return;
        }
        if self.used == capacity {
            self.evict_a_segment();
        }
        let slot = self.next % capacity;
        let at = HEADER + slot * RECORD;
        encode(record, &mut self.memory[at..at + RECORD]);
        self.next = (self.next + 1) % capacity;
        if self.used < capacity {
            self.used += 1;
        }
        self.write_header();
    }

    /// Put one of the other machine's records in, as it wrote it.
    ///
    /// The bytes are not decoded on the way through: what this end would make of
    /// them is the reader's business, and a recording that re-encoded them could
    /// only differ from what the machine that made them said (ADR-0009). The bit
    /// that says whose they are is the one thing added.
    pub(crate) fn push_from_the_source(&mut self, record: [u8; RECORD]) {
        let capacity = self.capacity();
        if capacity == 0 {
            return;
        }
        if self.used == capacity {
            self.evict_a_segment();
        }
        let slot = self.next % capacity;
        let at = HEADER + slot * RECORD;
        self.memory[at..at + RECORD].copy_from_slice(&record);
        self.memory[at] |= FROM_THE_SOURCE;
        self.next = (self.next + 1) % capacity;
        if self.used < capacity {
            self.used += 1;
        }
        self.write_header();
    }

    /// The next record this recording has not yet handed to the link, as the
    /// bytes a frame carries.
    ///
    /// One at a time rather than all of them at once, because what sends them is
    /// a loop with a keyboard to serve: a session that came up with a long
    /// backlog would otherwise spend the whole of one wait writing history while
    /// the person types into nothing. The run takes one per way round, and the
    /// backlog drains behind the keystrokes rather than in front of them.
    ///
    /// Records evicted before they were sent are gone: this skips to the oldest
    /// one still held rather than counting them again, since a link down long
    /// enough to overflow the ring has lost that history either way.
    pub(crate) fn unsent(&mut self) -> Option<[u8; RECORD]> {
        let capacity = self.capacity();
        if capacity == 0 {
            return None;
        }
        let written = self.evicted + self.used as u64;
        if self.sent >= written {
            return None;
        }
        self.sent = self.sent.max(self.evicted);
        let ahead = (self.sent - self.evicted) as usize;
        let slot = (self.first + ahead) % capacity;
        let at = HEADER + slot * RECORD;
        let mut out = [0u8; RECORD];
        out.copy_from_slice(&self.memory[at..at + RECORD]);
        self.sent += 1;
        Some(out)
    }

    /// Drop from the oldest record up to the next checkpoint.
    ///
    /// By segment and never by single record: dropping one at a time eventually
    /// drops the checkpoint the remaining records describe changes to, leaving a
    /// tail from which no state can be computed. Dropping to the *next* checkpoint
    /// costs history and keeps what is left replayable.
    fn evict_a_segment(&mut self) {
        let capacity = self.capacity();
        let mut dropped = 0;
        while dropped < self.used {
            let index = (self.first + dropped) % capacity;
            let at = HEADER + index * RECORD;
            let kind = self.memory[at];
            // Stop *before* the next checkpoint, so the trace starts at one.
            if dropped > 0 && kind == KIND_CHECKPOINT_BEGIN {
                break;
            }
            dropped += 1;
        }
        // A whole buffer with no later checkpoint: everything goes, and the next
        // record written will be the checkpoint the writer takes for the new
        // segment.
        self.first = (self.first + dropped) % capacity;
        self.used -= dropped;
        self.evicted += dropped as u64;
    }

    fn write_header(&mut self) {
        if self.memory.len() < HEADER {
            return;
        }
        let first = self.first as u32;
        let used = self.used as u32;
        self.memory[4..8].copy_from_slice(&first.to_le_bytes());
        self.memory[8..12].copy_from_slice(&used.to_le_bytes());
        self.memory[12..16].copy_from_slice(&(self.evicted as u32).to_le_bytes());
    }

    /// Read a recording back, without needing the writer.
    ///
    /// A separate borrow so the watchdog's copy of a region can be read exactly as
    /// the region itself would be.
    pub fn read(memory: &[u8]) -> Reader<'_> {
        Reader::new(memory)
    }
}

/// Which machine wrote a record.
///
/// One recording holds both: the forwarding machine's records cross the link to
/// the converting one, which is the machine that keeps the region (ADR-0009). So
/// there is nothing to merge afterwards — the order is the order they were
/// written, and this says whose each one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The machine the keyboards are read on, which sends.
    Source,
    /// The machine they are converted on, which receives, and whose recording
    /// this is.
    Sink,
}

/// A recording being read.
pub struct Reader<'a> {
    memory: &'a [u8],
    first: usize,
    used: usize,
    evicted: u64,
}

impl<'a> Reader<'a> {
    fn new(memory: &'a [u8]) -> Self {
        if memory.len() < HEADER || u32::from_le_bytes(head(memory, 0)) != MAGIC {
            return Self {
                memory,
                first: 0,
                used: 0,
                evicted: 0,
            };
        }
        let capacity = capacity_of(memory.len());
        let first = u32::from_le_bytes(head(memory, 4)) as usize;
        let used = u32::from_le_bytes(head(memory, 8)) as usize;
        Self {
            memory,
            first: if capacity == 0 { 0 } else { first % capacity },
            used: used.min(capacity),
            evicted: u32::from_le_bytes(head(memory, 12)) as u64,
        }
    }

    /// How many records were dropped from the front.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Whether the first record retained is a checkpoint, which is what makes the
    /// rest replayable.
    pub fn begins_at_a_checkpoint(&self) -> bool {
        matches!(
            self.own_records().next(),
            Some(Record::CheckpointBegin { .. })
        )
    }

    /// This machine's own records, without the other machine's.
    ///
    /// What a replay reproduces is this machine's run, and the records that
    /// arrived over the link are the other one's: a checkpoint's devices read out
    /// of a reading that held both would take whatever the other machine happened
    /// to have written next as this one's state (ADR-0009).
    fn own_records(&self) -> impl Iterator<Item = Record> + '_ {
        self.records()
            .filter(|(side, _)| *side == Side::Sink)
            .map(|(_, record)| record)
    }

    /// Every record retained, oldest first.
    pub fn records(&self) -> impl Iterator<Item = (Side, Record)> + '_ {
        let capacity = capacity_of(self.memory.len());
        (0..self.used).filter_map(move |offset| {
            let index = (self.first + offset) % capacity;
            let at = HEADER + index * RECORD;
            let bytes = &self.memory[at..at + RECORD];
            let side = match bytes[0] & FROM_THE_SOURCE == 0 {
                true => Side::Sink,
                false => Side::Source,
            };
            decode(bytes).map(|record| (side, record))
        })
    }

    /// How many checkpoints the retained window holds.
    ///
    /// What a trace can be replayed from, and there is one per segment rather
    /// than one per trace: a window that reaches back a minute holds several, and
    /// which of them a replay starts at decides how much of the run it reproduces
    /// (ADR-0009).
    pub fn checkpoints(&self) -> usize {
        self.own_records()
            .filter(|record| matches!(record, Record::CheckpointBegin { .. }))
            .count()
    }

    /// The state one checkpoint recorded, and the events that follow it — the
    /// script a replay from there runs.
    ///
    /// `from` numbers them oldest first. `None` where the trace holds no such
    /// checkpoint, which is what a replay of a window that has since been evicted
    /// past looks like.
    ///
    /// `pub` beside [`crate::sink::replay`], which runs it: what a binary writing
    /// the same segment out as a script needs is the segment, and working out
    /// which records belong to one is this format's own business — a second walk
    /// of the ring elsewhere would be a second answer to that.
    pub fn checkpoint(&self, from: usize) -> Option<(Checkpoint, Vec<HostEvent>)> {
        let mut records = self.own_records().skip_while({
            // Skipped by counting the ones that go past, rather than by asking
            // each record whether it belongs to a later segment: what separates
            // two of them is a `CheckpointBegin` and nothing else marks it.
            let mut seen = 0;
            move |record| match record {
                Record::CheckpointBegin { .. } => {
                    seen += 1;
                    seen <= from
                }
                _ => true,
            }
        });
        let Some(Record::CheckpointBegin {
            pointer_buttons,
            repeating,
        }) = records.next()
        else {
            return None;
        };
        let mut checkpoint = Checkpoint {
            pointer_buttons,
            repeating,
            devices: Vec::new(),
            held: Vec::new(),
        };
        let mut events = Vec::new();
        // Which checkpoint's state is being read, rather than whether any event
        // has arrived yet: a later segment's own device and held records sit
        // further down the same stream, and taking those would describe a machine
        // this checkpoint never saw.
        let mut theirs = true;
        for record in records {
            match record {
                Record::CheckpointDevice(info) if theirs => checkpoint.devices.push(info),
                Record::CheckpointHeld { device, key, state } if theirs => {
                    checkpoint.held.push((device, key, state))
                }
                Record::Event(event) => {
                    theirs = false;
                    events.push(event);
                }
                Record::CheckpointBegin { .. } => theirs = false,
                // An injection: what a replay produces is read off the host it is
                // given, never off what the run decided it would send (ADR-0009).
                _ => {}
            }
        }
        Some((checkpoint, events))
    }
}

/// The sink's state, as a checkpoint recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Checkpoint {
    pub pointer_buttons: Buttons,
    pub repeating: Option<(DeviceId, Key, Instant)>,
    pub devices: Vec<DeviceInfo>,
    pub held: Vec<(DeviceId, Key, HeldRecord)>,
}

fn head(memory: &[u8], at: usize) -> [u8; 4] {
    let mut out = [0u8; 4];
    out.copy_from_slice(&memory[at..at + 4]);
    out
}

fn capacity_of(bytes: usize) -> usize {
    bytes.saturating_sub(HEADER) / RECORD
}

/// A record's bytes: the kind, then whatever that kind carries.
///
/// Little-endian throughout and byte-for-byte explicit, rather than a `repr(C)`
/// struct cast: the watchdog reads this region, and a layout the compiler chose
/// would be a layout two builds could disagree about.
///
/// The matches are exhaustive with no catch-all, which is the only guard against
/// a vocabulary that grows past the format: a kind added to [`EventKind`] or
/// [`Record`] stops this compiling instead of being recorded as a record that
/// says nothing.
fn encode(record: Record, into: &mut [u8]) {
    into.fill(0);
    match record {
        Record::CheckpointBegin {
            pointer_buttons,
            repeating,
        } => {
            into[0] = KIND_CHECKPOINT_BEGIN;
            into[4..8].copy_from_slice(&pointer_buttons.bits().to_le_bytes());
            if let Some((device, key, at)) = repeating {
                into[1] = 1;
                into[2] = code(key);
                into[8..16].copy_from_slice(&device.0.to_le_bytes());
                into[16..24].copy_from_slice(&at.as_nanos().to_le_bytes());
            }
        }
        Record::CheckpointDevice(info) => {
            into[0] = KIND_CHECKPOINT_DEVICE;
            into[1] = u8::from(info.is_built_in);
            into[8..16].copy_from_slice(&info.id.0.to_le_bytes());
            if let Some(vendor) = info.vendor_id {
                into[2] = 1;
                into[16..18].copy_from_slice(&vendor.to_le_bytes());
            }
            if let Some(product) = info.product_id {
                into[3] = 1;
                into[18..20].copy_from_slice(&product.to_le_bytes());
            }
        }
        Record::CheckpointHeld { device, key, state } => {
            into[0] = KIND_CHECKPOINT_HELD;
            into[2] = code(key);
            into[8..16].copy_from_slice(&device.0.to_le_bytes());
            match state {
                HeldRecord::Down { key, modifiers } => {
                    into[1] = 1;
                    into[3] = code(key);
                    into[MODIFIER_KEYS].copy_from_slice(&modifiers.bits().to_le_bytes());
                }
                HeldRecord::Swallowed => into[1] = 2,
                HeldRecord::Henkan => into[1] = 3,
                HeldRecord::Undecided {
                    hold,
                    tap,
                    deadline,
                    sent,
                    alone,
                } => {
                    into[1] = 4;
                    into[3] = code(hold);
                    into[5] = code(tap);
                    into[6] = u8::from(alone);
                    if let Some(modifiers) = sent {
                        into[7] = 1;
                        into[MODIFIER_KEYS].copy_from_slice(&modifiers.bits().to_le_bytes());
                    }
                    into[16..24].copy_from_slice(&deadline.as_nanos().to_le_bytes());
                }
            }
        }
        Record::Event(event) => {
            into[24..32].copy_from_slice(&event.at.as_nanos().to_le_bytes());
            match event.kind {
                EventKind::DeviceAttached(info) => {
                    into[0] = KIND_EVENT_ATTACHED;
                    into[1] = u8::from(info.is_built_in);
                    into[8..16].copy_from_slice(&info.id.0.to_le_bytes());
                    if let Some(vendor) = info.vendor_id {
                        into[2] = 1;
                        into[16..18].copy_from_slice(&vendor.to_le_bytes());
                    }
                    if let Some(product) = info.product_id {
                        into[3] = 1;
                        into[18..20].copy_from_slice(&product.to_le_bytes());
                    }
                }
                EventKind::DeviceDetached(id) => {
                    into[0] = KIND_EVENT_DETACHED;
                    into[8..16].copy_from_slice(&id.0.to_le_bytes());
                }
                EventKind::KeyDown { device, key } => {
                    into[0] = KIND_EVENT_KEY_DOWN;
                    into[2] = code(key);
                    into[8..16].copy_from_slice(&device.0.to_le_bytes());
                }
                EventKind::KeyUp { device, key } => {
                    into[0] = KIND_EVENT_KEY_UP;
                    into[2] = code(key);
                    into[8..16].copy_from_slice(&device.0.to_le_bytes());
                }
                EventKind::Pointer { device, report } => {
                    into[0] = KIND_EVENT_POINTER;
                    into[8..16].copy_from_slice(&device.0.to_le_bytes());
                    encode_pointer(report, &mut into[16..24]);
                    into[4..8].copy_from_slice(&report.buttons.bits().to_le_bytes());
                }
                EventKind::Timer => into[0] = KIND_EVENT_TIMER,
                EventKind::Probe => into[0] = KIND_EVENT_PROBE,
                EventKind::Asked(driving) => {
                    into[0] = KIND_EVENT_ASKED;
                    into[1] = u8::from(driving == favjit_host::source::Driving::TheSink);
                }
            }
        }
        Record::Sent { at, code } => {
            into[0] = KIND_SENT;
            into[8..12].copy_from_slice(&code.to_le_bytes());
            into[16..24].copy_from_slice(&at.to_le_bytes());
        }
        Record::Received { at } => {
            into[0] = KIND_RECEIVED;
            into[16..24].copy_from_slice(&at.to_le_bytes());
        }
        Record::LinkEnded { why } => {
            into[0] = KIND_LINK_ENDED;
            into[4..8].copy_from_slice(&why.to_le_bytes());
        }
        Record::Served { frame, code } => {
            into[0] = KIND_SERVED;
            into[4..8].copy_from_slice(&(frame as u32).to_le_bytes());
            into[8..12].copy_from_slice(&code.to_le_bytes());
        }
        Record::OutputEnded { why } => {
            into[0] = KIND_OUTPUT_ENDED;
            into[4..8].copy_from_slice(&why.to_le_bytes());
        }
        Record::Injected {
            injected,
            rendered,
            // Bound under another name because `code` is this file's own function
            // for a key's byte, and the arm below calls it.
            code: answered,
        } => {
            into[1] = u8::from(rendered);
            // Where a device id sits on the records that carry one, which an
            // injection does not: what it went out through is the one output
            // device, so there is nothing to name.
            into[8..12].copy_from_slice(&answered.to_le_bytes());
            match injected {
                Injected::KeyDown { key, modifiers } => {
                    into[0] = KIND_INJECT_KEY_DOWN;
                    into[2] = code(key);
                    into[MODIFIER_KEYS].copy_from_slice(&modifiers.bits().to_le_bytes());
                }
                Injected::KeyUp { key, modifiers } => {
                    into[0] = KIND_INJECT_KEY_UP;
                    into[2] = code(key);
                    into[MODIFIER_KEYS].copy_from_slice(&modifiers.bits().to_le_bytes());
                }
                Injected::Modifiers(modifiers) => {
                    into[0] = KIND_INJECT_MODIFIERS;
                    into[MODIFIER_KEYS].copy_from_slice(&modifiers.bits().to_le_bytes());
                }
                Injected::Pointer(report) => {
                    into[0] = KIND_INJECT_POINTER;
                    encode_pointer(report, &mut into[16..24]);
                    into[4..8].copy_from_slice(&report.buttons.bits().to_le_bytes());
                }
            }
        }
    }
}

/// The deltas, as four `i16`.
///
/// Wider than the byte a HID report carries, because what is recorded is what the
/// hardware said and not what a report can hold — a relay that had to saturate a
/// delta is a thing a trace should show, not hide.
fn encode_pointer(report: PointerReport, into: &mut [u8]) {
    for (slot, value) in [
        report.dx,
        report.dy,
        report.vertical_wheel,
        report.horizontal_wheel,
    ]
    .into_iter()
    .enumerate()
    {
        let clamped = value.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        into[slot * 2..slot * 2 + 2].copy_from_slice(&clamped.to_le_bytes());
    }
}

fn decode_pointer(from: &[u8], buttons: Buttons) -> PointerReport {
    let axis = |slot: usize| i16::from_le_bytes([from[slot * 2], from[slot * 2 + 1]]) as i32;
    PointerReport {
        dx: axis(0),
        dy: axis(1),
        vertical_wheel: axis(2),
        horizontal_wheel: axis(3),
        buttons,
    }
}

fn decode(from: &[u8]) -> Option<Record> {
    // Without the bit that says which machine wrote it: what a record means is
    // the same either way, and which end it came off is the reader's to report
    // beside it.
    let kind = from[0] & !FROM_THE_SOURCE;
    let flag = from[1] != 0;
    let code = i32::from_le_bytes([from[8], from[9], from[10], from[11]]);
    // The eight bytes an event's timestamp uses, which the records carrying a
    // sequence number do not — and no record carries both.
    let sequence = u64::from_le_bytes([
        from[16], from[17], from[18], from[19], from[20], from[21], from[22], from[23],
    ]);
    let key_a = decode_key(from[2]);
    let key_b = decode_key(from[3]);
    let modifiers = ModifierKeys::from_bits(u16::from_le_bytes([
        from[MODIFIER_KEYS.start],
        from[MODIFIER_KEYS.start + 1],
    ]));
    let buttons = Buttons::from_bits(u32::from_le_bytes([from[4], from[5], from[6], from[7]]));
    let device = DeviceId(u64::from_le_bytes([
        from[8], from[9], from[10], from[11], from[12], from[13], from[14], from[15],
    ]));
    let payload_at = Instant::from_nanos(u64::from_le_bytes([
        from[16], from[17], from[18], from[19], from[20], from[21], from[22], from[23],
    ]));
    let at = Instant::from_nanos(u64::from_le_bytes([
        from[24], from[25], from[26], from[27], from[28], from[29], from[30], from[31],
    ]));
    let info = DeviceInfo {
        id: device,
        is_built_in: flag,
        vendor_id: (from[2] != 0).then(|| u16::from_le_bytes([from[16], from[17]])),
        product_id: (from[3] != 0).then(|| u16::from_le_bytes([from[18], from[19]])),
    };

    Some(match kind {
        KIND_CHECKPOINT_BEGIN => Record::CheckpointBegin {
            pointer_buttons: buttons,
            repeating: match (flag, key_a) {
                (true, Some(key)) => Some((device, key, payload_at)),
                // A flag set with no key behind it is a record half written, which
                // is what a torn read of the region looks like. Dropping the whole
                // record is right: a checkpoint claiming a repeat of no key would
                // replay a keystroke that never happened.
                (true, None) => return None,
                (false, _) => None,
            },
        },
        KIND_CHECKPOINT_DEVICE => Record::CheckpointDevice(info),
        KIND_CHECKPOINT_HELD => Record::CheckpointHeld {
            device,
            key: key_a?,
            state: match from[1] {
                1 => HeldRecord::Down {
                    key: key_b?,
                    modifiers,
                },
                2 => HeldRecord::Swallowed,
                3 => HeldRecord::Henkan,
                4 => HeldRecord::Undecided {
                    hold: key_b?,
                    tap: decode_key(from[5])?,
                    deadline: payload_at,
                    sent: (from[7] != 0).then_some(modifiers),
                    alone: from[6] != 0,
                },
                _ => return None,
            },
        },
        KIND_EVENT_ATTACHED => Record::Event(HostEvent::new(at, EventKind::DeviceAttached(info))),
        KIND_EVENT_DETACHED => Record::Event(HostEvent::new(at, EventKind::DeviceDetached(device))),
        KIND_EVENT_KEY_DOWN => Record::Event(HostEvent::new(
            at,
            EventKind::KeyDown {
                device,
                key: key_a?,
            },
        )),
        KIND_EVENT_KEY_UP => Record::Event(HostEvent::new(
            at,
            EventKind::KeyUp {
                device,
                key: key_a?,
            },
        )),
        KIND_EVENT_POINTER => Record::Event(HostEvent::new(
            at,
            EventKind::Pointer {
                device,
                report: decode_pointer(&from[16..24], buttons),
            },
        )),
        KIND_EVENT_TIMER => Record::Event(HostEvent::new(at, EventKind::Timer)),
        KIND_EVENT_PROBE => Record::Event(HostEvent::new(at, EventKind::Probe)),
        KIND_EVENT_ASKED => Record::Event(HostEvent::new(
            at,
            EventKind::Asked(match flag {
                true => favjit_host::source::Driving::TheSink,
                false => favjit_host::source::Driving::ThisMachine,
            }),
        )),
        KIND_INJECT_KEY_DOWN => Record::Injected {
            injected: Injected::KeyDown {
                key: key_a?,
                modifiers,
            },
            rendered: flag,
            code,
        },
        KIND_INJECT_KEY_UP => Record::Injected {
            injected: Injected::KeyUp {
                key: key_a?,
                modifiers,
            },
            rendered: flag,
            code,
        },
        KIND_INJECT_POINTER => Record::Injected {
            injected: Injected::Pointer(decode_pointer(&from[16..24], buttons)),
            rendered: flag,
            code,
        },
        KIND_INJECT_MODIFIERS => Record::Injected {
            injected: Injected::Modifiers(modifiers),
            rendered: flag,
            code,
        },
        KIND_SENT => Record::Sent { at: sequence, code },
        KIND_RECEIVED => Record::Received { at: sequence },
        // Where a pointer's buttons sit on the records that carry them, which
        // this does not.
        KIND_LINK_ENDED => Record::LinkEnded {
            why: u32::from_le_bytes([from[4], from[5], from[6], from[7]]),
        },
        // A frame number this crate has no name for is skipped rather than
        // guessed at, for the reason an unknown kind is: a record read as the
        // wrong frame would say the connection was being held up by something
        // that never went out.
        KIND_SERVED => Record::Served {
            frame: Served::from_number(u32::from_le_bytes([from[4], from[5], from[6], from[7]]))?,
            code,
        },
        KIND_OUTPUT_ENDED => Record::OutputEnded {
            why: u32::from_le_bytes([from[4], from[5], from[6], from[7]]),
        },
        // Zero is what an unwritten slot and an unknown kind both look like, and
        // skipping is right for both: a reader that guessed would put a record
        // into a replay that never happened.
        _ => return None,
    })
}

/// The number this format carries a key as.
///
/// [`Key::code`] rather than a numbering of its own, so that a trace and the link
/// cannot come to disagree about which key a byte means.
fn code(key: Key) -> u8 {
    key.code()
}

fn decode_key(code: u8) -> Option<Key> {
    Key::from_code(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{built_in, external};

    fn round_trip(record: Record) {
        let mut bytes = [0u8; RECORD];
        encode(record, &mut bytes);
        assert_eq!(decode(&bytes), Some(record), "did not survive the encoding");
    }

    #[test]
    fn every_key_has_a_number_of_its_own() {
        // A wire format's numbering has to be a function, and the only thing that
        // makes it one is this: two keys sharing a number would replay as each
        // other.
        let mut seen = std::collections::BTreeSet::new();
        for key in Key::ALL {
            let number = code(*key);
            assert_ne!(number, 0, "{key:?} has no number");
            assert!(seen.insert(number), "{key:?} shares a number");
            assert_eq!(decode_key(number), Some(*key));
        }
    }

    #[test]
    fn events_survive_the_encoding() {
        let at = Instant::from_nanos(1_234_567_890);
        round_trip(Record::Event(HostEvent::new(
            at,
            EventKind::DeviceAttached(external(DeviceId(7), 6127, 24801)),
        )));
        round_trip(Record::Event(HostEvent::new(
            at,
            EventKind::DeviceAttached(built_in(DeviceId(1))),
        )));
        round_trip(Record::Event(HostEvent::new(
            at,
            EventKind::DeviceDetached(DeviceId(9)),
        )));
        round_trip(Record::Event(HostEvent::new(
            at,
            EventKind::KeyDown {
                device: DeviceId(1),
                key: Key::Q,
            },
        )));
        round_trip(Record::Event(HostEvent::new(
            at,
            EventKind::KeyUp {
                device: DeviceId(1),
                key: Key::International3,
            },
        )));
        round_trip(Record::Event(HostEvent::new(at, EventKind::Timer)));
        round_trip(Record::Event(HostEvent::new(at, EventKind::Probe)));
    }

    #[test]
    fn a_pointer_report_survives_the_encoding_including_its_signs() {
        round_trip(Record::Event(HostEvent::new(
            Instant::from_nanos(5),
            EventKind::Pointer {
                device: DeviceId(2),
                report: PointerReport {
                    dx: -40,
                    dy: 13,
                    vertical_wheel: -1,
                    horizontal_wheel: 2,
                    buttons: Buttons::NONE.with(1).with(3),
                },
            },
        )));
    }

    #[test]
    fn outbound_calls_survive_the_encoding_with_their_results() {
        round_trip(Record::Injected {
            injected: Injected::KeyDown {
                key: Key::Semicolon,
                modifiers: ModifierKeys::of(&[Key::LeftShift]),
            },
            rendered: true,
            code: 0,
        });
        // Rendered and refused, which is the pair a run cannot report as one
        // answer: the report was built and the machine would not take it.
        round_trip(Record::Injected {
            injected: Injected::KeyUp {
                key: Key::Fn,
                modifiers: ModifierKeys::NONE,
            },
            rendered: true,
            code: 32,
        });
        round_trip(Record::Injected {
            injected: Injected::KeyUp {
                key: Key::Fn,
                modifiers: ModifierKeys::NONE,
            },
            rendered: false,
            code: 0,
        });
        round_trip(Record::Injected {
            injected: Injected::Pointer(PointerReport::moved(3, -3)),
            rendered: true,
            code: 0,
        });
        // Both sides of one modifier at once, which is what a set can say and a
        // single modifier cannot.
        round_trip(Record::Injected {
            injected: Injected::Modifiers(ModifierKeys::of(&[
                Key::LeftShift,
                Key::RightShift,
                Key::RightCommand,
            ])),
            rendered: true,
            code: 0,
        });
        round_trip(Record::Injected {
            injected: Injected::Modifiers(ModifierKeys::NONE),
            rendered: true,
            code: 0,
        });
        // Negative as well as positive: `-1` is what a call nobody made answers
        // with, and a field read as unsigned would turn it into a large errno.
        round_trip(Record::Sent { at: 0, code: 0 });
        round_trip(Record::Sent { at: 1, code: 10060 });
        round_trip(Record::Sent { at: 7, code: -1 });
        // The number a session runs to, so a record late in a long link is not
        // one whose count was truncated on the way into the ring.
        round_trip(Record::Sent {
            at: u64::MAX,
            code: 0,
        });
        round_trip(Record::Received { at: 0 });
        round_trip(Record::Received { at: u64::MAX });
    }

    #[test]
    fn a_checkpoint_survives_the_encoding() {
        round_trip(Record::CheckpointBegin {
            pointer_buttons: Buttons::NONE.with(2),
            repeating: Some((DeviceId(1), Key::A, Instant::from_nanos(7))),
        });
        round_trip(Record::CheckpointBegin {
            pointer_buttons: Buttons::NONE,
            repeating: None,
        });
        round_trip(Record::CheckpointDevice(external(DeviceId(3), 1, 2)));
        round_trip(Record::CheckpointHeld {
            device: DeviceId(1),
            key: Key::Spacebar,
            state: HeldRecord::Undecided {
                hold: Key::LeftShift,
                tap: Key::Spacebar,
                deadline: Instant::from_nanos(1000),
                sent: Some(ModifierKeys::of(&[Key::LeftControl])),
                alone: true,
            },
        });
        round_trip(Record::CheckpointHeld {
            device: DeviceId(1),
            key: Key::Q,
            state: HeldRecord::Down {
                key: Key::Digit1,
                modifiers: ModifierKeys::NONE,
            },
        });
        round_trip(Record::CheckpointHeld {
            device: DeviceId(1),
            key: Key::Q,
            state: HeldRecord::Swallowed,
        });
        round_trip(Record::CheckpointHeld {
            device: DeviceId(1),
            key: Key::RightCommand,
            state: HeldRecord::Henkan,
        });
    }

    #[test]
    fn a_record_is_read_back_in_the_order_it_was_written() {
        let mut memory = vec![0u8; HEADER + RECORD * 8];
        let mut trace = Trace::new(&mut memory);
        for nanos in 1..=5u64 {
            trace.push(Record::Event(HostEvent::new(
                Instant::from_nanos(nanos),
                EventKind::Timer,
            )));
        }
        // The writer's borrow has to end before the region can be read, which is
        // the same shape the real thing has: the watchdog reads a copy, never the
        // buffer a running favjit is writing into.
        let memory = &memory[..];
        let reader = Trace::read(memory);
        let times: Vec<u64> = reader
            .records()
            .filter_map(|(_, record)| match record {
                Record::Event(event) => Some(event.at.as_nanos()),
                _ => None,
            })
            .collect();
        assert_eq!(times, vec![1, 2, 3, 4, 5]);
        assert_eq!(reader.evicted(), 0);
    }

    #[test]
    fn a_full_buffer_drops_to_the_next_checkpoint() {
        // Two segments and no room for a third: the first goes whole, and what is
        // left starts at the second's checkpoint.
        let mut memory = vec![0u8; HEADER + RECORD * 4];
        let mut trace = Trace::new(&mut memory);
        let checkpoint = Record::CheckpointBegin {
            pointer_buttons: Buttons::NONE,
            repeating: None,
        };
        let event =
            |nanos| Record::Event(HostEvent::new(Instant::from_nanos(nanos), EventKind::Timer));

        trace.push(checkpoint);
        trace.push(event(1));
        trace.push(checkpoint);
        trace.push(event(2));
        // Full. This one evicts the first segment, which is two records.
        trace.push(event(3));
        // The writer's borrow has to end before the region can be read, which is
        // the same shape the real thing has: the watchdog reads a copy, never the
        // buffer a running favjit is writing into.
        let memory = &memory[..];
        let reader = Trace::read(memory);
        assert_eq!(reader.evicted(), 2);
        assert!(reader.begins_at_a_checkpoint());
        let (_, events) = reader.checkpoint(0).expect("a checkpoint to replay from");
        let times: Vec<u64> = events.iter().map(|e| e.at.as_nanos()).collect();
        assert_eq!(times, vec![2, 3]);
    }

    #[test]
    fn a_region_that_was_never_written_reads_as_empty() {
        // What the watchdog will hand over when it never got a trace: zeroes. A
        // reader that took them for records would replay a run that never
        // happened.
        let memory = vec![0u8; HEADER + RECORD * 4];
        let reader = Trace::read(&memory);
        assert_eq!(reader.records().count(), 0);
        assert!(!reader.begins_at_a_checkpoint());
    }
}
