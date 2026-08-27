//! A run recorded on one machine, replayed as a script on another (ADR-0009).
//!
//! What makes this possible is that `engine` is a function of its event stream, and
//! that the simulated host drives it through the same stream — so a recording is
//! the same shape as a script. These pin the property the whole design rests on:
//! **the same trace produces the same keystrokes.** A trace that did not would be
//! a debugging tool that lies about what happened.
//!
//! The buffer is plain process memory here, handed in by the caller, because
//! ADR-0006 has the host provide it and `host-sim`'s host is this process.
//!
//! Checked in the bytes each run actually wrote, decoded back with
//! `favjit-hid`'s inverse of the encoding, and never against what a trace says
//! `engine` decided: a recording that agreed with the decision and disagreed with
//! the device would be a recording of the wrong thing. What is read out of a
//! trace here is what a person reading one gets — how many records survived,
//! whether it begins at a checkpoint, which events it holds — and the keystrokes
//! a replay of it produces.

use core::time::Duration;

use favjit_engine::pointer;
use favjit_engine::sink::{self, InputConfig, Repeat, Replayed, Request};
use favjit_engine::trace::{Reader, Record, Trace};
use favjit_engine::{Buttons, DeviceId, HostEvent, Key, Layout, PointerReport};
use favjit_hid::report::{pointing_from_bytes, Report as KeyboardReport};
use favjit_host::OutputReport;
use favjit_host_sim::{identity, Did, SimHost, SimLink, SimPointer, OUTPUT_VENDOR};

const BUILT_IN: DeviceId = DeviceId(1);
const TRACKPOINT: DeviceId = DeviceId(2);

fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// Enough of everything to be worth replaying: a layer, a tap-hold decided both
/// ways, a shifted character, a pointer, and a keyboard torn away while it holds
/// something.
fn script(host: &mut SimHost) {
    host.attach_built_in(BUILT_IN);
    host.attach_external(TRACKPOINT, 6127, 24801);

    host.tap(BUILT_IN, Key::S);
    host.tap(TRACKPOINT, Key::Q);

    // Space held with another key: shift. Then space alone: a space.
    host.press(BUILT_IN, Key::Spacebar).advance(ms(20));
    host.tap(BUILT_IN, Key::A);
    host.advance(ms(20)).release(BUILT_IN, Key::Spacebar);
    host.hold(BUILT_IN, Key::Spacebar, ms(60));

    // The Henkan layer.
    host.press(BUILT_IN, Key::RightCommand);
    host.tap(BUILT_IN, Key::Q);
    host.release(BUILT_IN, Key::RightCommand);

    host.pointer(TRACKPOINT, PointerReport::moved(4, -2));
    host.pointer(
        TRACKPOINT,
        PointerReport {
            buttons: Buttons::NONE.with(1),
            ..PointerReport::default()
        },
    );
    host.pointer(TRACKPOINT, PointerReport::default());

    // Torn away holding a modifier, which is the case that strands one.
    host.press(TRACKPOINT, Key::LeftShift);
    host.detach(TRACKPOINT);

    host.probe();
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The file `favjit --pair` leaves behind, holding the one key the listening run
/// below is paired with.
///
/// Written here rather than built with the simulator's own writer: a script that
/// started from the same function the simulator writes that file with would be
/// checking that function against itself.
fn paired_list() -> String {
    format!("{}\n", hex(identity(0xaa).public()))
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// What a run wrote, on whichever page each report landed — a keyboard report
/// and a pointer report both cross this boundary here.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Written {
    Keyboard(KeyboardReport),
    Pointer(PointerReport),
}

/// Every report a host wrote, decoded onto whichever variant its own page names.
fn decode(host: &SimHost) -> Vec<Written> {
    host.reports()
        .iter()
        .map(|(_, report, bytes)| match *report {
            OutputReport::Keyboard => {
                Written::Keyboard(KeyboardReport::from_bytes(bytes).expect("a keyboard report"))
            }
            OutputReport::Pointing => {
                Written::Pointer(pointing_from_bytes(bytes).expect("a pointing report"))
            }
            // `OutputReport` is `#[non_exhaustive]`, and nothing here ever
            // asks for a control.
            other => panic!("not a keyboard or pointing report: {other:?}"),
        })
        .collect()
}

/// The events a recording holds, in the order they were handled.
fn events(trace: &Reader<'_>) -> Vec<HostEvent> {
    trace
        .records()
        .filter_map(|(_, record)| match record {
            Record::Event(event) => Some(event),
            _ => None,
        })
        .collect()
}

/// How many injections it recorded alongside them.
fn injections(trace: &Reader<'_>) -> usize {
    trace
        .records()
        .filter(|(_, record)| matches!(record, Record::Injected { .. }))
        .count()
}

/// Run the script, recording into a buffer of this size, and return what reached
/// applications alongside the recording.
fn record(bytes: usize) -> (Vec<Written>, Vec<u8>) {
    recording(bytes, None, script)
}

/// A keyboard on the machine besides the built-in one, for the keys the script
/// below holds on a keyboard that keeps its JIS labels.
const EXTERNAL: DeviceId = DeviceId(3);

/// A script that is still holding keys when a checkpoint is taken, in every state
/// a held key can be in: a plain key down, the Henkan layer, an undecided
/// tap-hold, and a press the layout swallowed.
///
/// One script for all four rather than one each, because they are recorded as one
/// record apiece and what a checkpoint has to carry is whatever is down *at once*.
fn held_across_a_checkpoint(host: &mut SimHost) {
    host.attach_built_in(BUILT_IN);
    host.attach_external(EXTERNAL, 1, 2);

    // Command first and then `h` on a keyboard that keeps its JIS labels, which is
    // the press the hide guard swallows. Before the tap-hold below, because the
    // shift that one lends is not a modifier the guard tolerates.
    host.press(EXTERNAL, Key::LeftCommand);
    host.press(EXTERNAL, Key::H);

    host.press(BUILT_IN, Key::RightCommand);
    host.press(BUILT_IN, Key::Spacebar);

    // The key the repeat runs on, and long enough for its records to push the
    // start of the run out of a small buffer.
    host.press(EXTERNAL, Key::K);
    host.advance(Duration::from_secs(11));
    host.release(EXTERNAL, Key::K);
}

/// The same for a run given repeat rates and a script of its own.
fn recording(
    bytes: usize,
    repeat: Option<Repeat>,
    script: impl FnOnce(&mut SimHost),
) -> (Vec<Written>, Vec<u8>) {
    let mut memory = vec![0u8; bytes];
    // A machine with an output pointer to tune, because a recorded run reaches
    // its machine through the recording: a run that tuned nothing would leave
    // that half of the wrapping unexercised, and a recording that saw only half
    // of what crossed the boundary is the one thing a trace must not be
    // (ADR-0009).
    let mut host = SimHost::new()
        .told_to_tune(Some(80.0), Some(0.8))
        .with_pointers(vec![
            SimPointer::new(OUTPUT_VENDOR).with_fixed(pointer::RESOLUTION, 400.0)
        ]);
    script(&mut host);
    sink::run(
        &converting(),
        Layout::dudrack(),
        repeat,
        InputConfig::default(),
        &mut host,
        Some(&mut memory),
    );
    (decode(&host), memory)
}

/// Replay a recording from its oldest checkpoint through a fresh simulated host.
fn replay(memory: &[u8]) -> Vec<Written> {
    replaying(memory, None, 0).0
}

/// The same from any checkpoint, for a recording of a run that produced its own
/// repeats: the rates are configuration a replay is given rather than anything
/// the trace carries (ADR-0009), so a replay of such a run has to be given them
/// too.
fn replaying(memory: &[u8], repeat: Option<Repeat>, from: usize) -> (Vec<Written>, Replayed) {
    let trace = Trace::read(memory);
    let mut host = SimHost::new();
    // The same place a device's reports would go, which is what the reproduction
    // is compared against: a replay written somewhere else would be checked
    // against a list nothing filled.
    let writing = favjit_host::sink::SinkHost::instead_of_the_output(&mut host);
    let replayed = sink::replay(Layout::dudrack(), repeat, &trace, from, &mut host, writing);
    (decode(&host), replayed)
}

#[test]
fn a_trace_replays_to_the_same_keystrokes() {
    let (written, memory) = record(64 * 1024);
    assert!(!written.is_empty(), "the script has to produce something");
    assert_eq!(replay(&memory), written);
}

#[test]
fn a_trace_records_what_was_sent_as_well_as_what_arrived() {
    // Outbound calls with their results, not only the inbound events: the script
    // has a keyboard torn away while it holds a key, so one of the injections
    // found no report to ride — and what a run does after a refused injection
    // depends on knowing it was refused. A recording of the events alone would
    // replay that stretch differently, which is what the equality above would
    // then fail on.
    let (written, memory) = record(64 * 1024);
    let trace = Trace::read(&memory);

    assert_eq!(replay(&memory), written);
    assert!(
        injections(&trace) >= written.len(),
        "every report written has to have an injection behind it in the recording"
    );
}

#[test]
fn what_survives_eviction_still_begins_at_a_checkpoint() {
    // Small enough that the run cannot fit, so the oldest segments go. Whatever
    // is left has to start at a checkpoint, or the events that remain describe
    // changes to a state nothing knows.
    // Room for fifteen records, against a script that writes far more.
    let (_, memory) = record(512);
    let trace = Trace::read(&memory);

    assert!(trace.evicted() > 0, "the buffer has to have overflowed");
    assert!(
        trace.begins_at_a_checkpoint(),
        "a trace that begins mid-stream cannot be replayed from"
    );
}

#[test]
fn replaying_what_survived_eviction_reproduces_the_end_of_the_run() {
    // The checkpoint is what makes this possible: replay starts from the state
    // `engine` was in when the surviving segment began, so the keystrokes from that
    // point on come out the same.
    let (written, memory) = record(512);
    let tail = replay(&memory);

    assert!(
        written.ends_with(&tail),
        "replaying from the checkpoint should reproduce the end of the run;\n  \
         whole: {written:?}\n  tail: {tail:?}"
    );
    assert!(
        !tail.is_empty(),
        "something has to survive, or the buffer is too small to be useful"
    );
    assert!(
        tail.len() < written.len(),
        "this test is only meaningful while the trace really did lose the start"
    );
}

#[test]
fn a_replay_resumes_with_the_keys_that_were_still_held() {
    // A key still down is state the surviving events describe changes to: its
    // release has to reach the OS as a release of what it typed, the layer it
    // was under has to still be in force, and the repeat that was running has
    // to carry on. A checkpoint that recorded only the devices would replay the
    // tail against a machine on which nothing was pressed, and the difference
    // shows up as the keystrokes the tail produces.
    //
    // Every held state a key can be in at once, because they are recorded as one
    // record each and a state left out is one no replay would resume from.
    let repeat = Some(Repeat {
        initial: ms(250),
        interval: ms(100),
    });
    let (written, memory) = recording(4096, repeat, held_across_a_checkpoint);

    let trace = Trace::read(&memory);
    assert!(trace.evicted() > 0, "the buffer has to have overflowed");
    assert!(
        trace.begins_at_a_checkpoint(),
        "a trace that begins mid-stream cannot be replayed from"
    );

    let (tail, _) = replaying(&memory, repeat, 0);
    assert!(
        written.ends_with(&tail),
        "the replay has to reproduce the end of the run;\n  \
         whole: {written:?}\n  tail: {tail:?}"
    );
    assert!(
        tail.len() < written.len(),
        "this test is only meaningful while the trace really did lose the start"
    );
}

#[test]
fn the_trace_holds_the_event_the_run_stopped_on() {
    // What a wedged converter's recording has to contain. A loop that stops after
    // handling an event — because a supervisor is about to kill it, or because it
    // wedged there on purpose — leaves a trace that has to say what favjit was
    // given last: the key that was down when it stopped is the whole of why a
    // stuck key happened, and a recording missing it explains nothing.
    //
    // Pinned here rather than around the wedge itself, which lives in the binary
    // and wraps a real host: this suite cannot reach one without becoming
    // platform-specific (ADR-0005), so what it holds is the half `engine` owns —
    // that the event is recorded before it is handled, so anything stopping after
    // that keeps it.
    //
    // Stops after the second raw event rather than after decoding a `KeyDown`:
    // recognising one at this level would mean naming its usage here too, a
    // second copy of the table `engine` itself reads it with. The script below is
    // built to make the second event the key going down, so counting is enough.
    struct StopsAfterAnEvent<'a> {
        inner: &'a mut SimHost,
        seen: usize,
        stop_after: usize,
        stop: bool,
    }

    impl favjit_host::Host for StopsAfterAnEvent<'_> {
        fn now(&mut self) -> favjit_host::Instant {
            self.inner.now()
        }

        fn next_event(&mut self, deadline: favjit_host::Instant) -> Option<favjit_host::HostEvent> {
            if self.stop {
                return None;
            }
            let event = self.inner.next_event(deadline)?;
            self.seen += 1;
            Some(event)
        }

        fn is_supervised(&mut self) -> bool {
            self.inner.is_supervised()
        }

        fn warn(&mut self, message: core::fmt::Arguments) {
            self.inner.warn(message);
        }

        /// Where the loop gives up, which is where a wedge stops it: after the
        /// event is through.
        fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
            let beat = self.inner.heartbeat();
            if self.seen >= self.stop_after {
                self.stop = true;
            }
            beat
        }
    }

    impl favjit_host::sink::SinkInputHost for StopsAfterAnEvent<'_> {
        fn switched_on(&mut self) -> bool {
            self.inner.switched_on()
        }

        fn may_read_input(&mut self) -> bool {
            self.inner.may_read_input()
        }

        fn request_input_permission(&mut self) -> bool {
            self.inner.request_input_permission()
        }

        fn output_connected(&mut self) -> bool {
            self.inner.output_connected()
        }

        // `stop` alone would leave `next_event` returning `None` forever
        // with nothing telling the loop that is the run ending rather than
        // an ordinary empty wait — `idle_ended` reads this, not `stop`.
        fn stop_requested(&mut self) -> bool {
            self.stop || self.inner.stop_requested()
        }

        fn look_for_devices(
            &mut self,
            work: favjit_host::capture::SinkLoop,
        ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
            self.inner.look_for_devices(work)
        }
    }

    impl favjit_host::IdentityStore for StopsAfterAnEvent<'_> {
        fn read(&mut self) -> Option<Vec<u8>> {
            self.inner.read()
        }

        fn make_directory(&mut self) -> Result<(), favjit_host::Trouble> {
            self.inner.make_directory()
        }

        fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, favjit_host::Trouble> {
            self.inner.open()
        }
    }

    impl favjit_host::Entropy for StopsAfterAnEvent<'_> {
        fn fill(&mut self, into: &mut [u8]) -> bool {
            self.inner.fill(into)
        }
    }

    impl favjit_host::PointerHost for StopsAfterAnEvent<'_> {
        fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
            self.inner.wanted_pointer_feel()
        }

        fn output_vendor(&mut self) -> i64 {
            self.inner.output_vendor()
        }

        fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
            self.inner.open_event_system()
        }

        fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
            self.inner.open_simple_event_system()
        }
    }

    impl favjit_host::sink::SinkHost for StopsAfterAnEvent<'_> {
        fn reach_the_output(
            &mut self,
        ) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
            self.inner.reach_the_output()
        }

        fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
            self.inner.run_output_alongside(work)
        }

        fn bind_link(&mut self) -> Option<Box<dyn favjit_host::link::LinkHost + Send>> {
            self.inner.bind_link()
        }

        fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
            self.inner.run_alongside(work)
        }

        fn instead_of_the_output(&mut self) -> Box<dyn favjit_host::sink::Injecting> {
            self.inner.instead_of_the_output()
        }
    }

    let mut memory = vec![0u8; 64 * 1024];
    let mut sim = SimHost::new();
    sim.attach_built_in(BUILT_IN);
    sim.press(BUILT_IN, Key::S);
    sim.release(BUILT_IN, Key::S);
    {
        // The device's own announcement is the first raw event this stream
        // carries, so the key going down is the second.
        let mut host = StopsAfterAnEvent {
            inner: &mut sim,
            seen: 0,
            stop_after: 2,
            stop: false,
        };
        sink::run(
            &converting(),
            Layout::dudrack(),
            None,
            InputConfig::default(),
            &mut host,
            Some(&mut memory),
        );
    }

    let trace = Trace::read(&memory);
    let handled = events(&trace);
    let last = handled.last().expect("the run handled something");
    assert!(
        matches!(
            last.kind,
            favjit_engine::EventKind::KeyDown { key: Key::S, .. }
        ),
        "the last event recorded should be the one it stopped on; got {:?}",
        last.kind
    );
    assert!(
        injections(&trace) > 0,
        "and what that event produced has to be there too, or the recording says \
         a key arrived and nothing came of it"
    );
}

#[test]
fn recording_changes_nothing_about_the_run_it_records() {
    // The recording sits between the loop and the machine for the whole run,
    // bring-up included, so every outbound call there is goes through it. What
    // it must not do is change any of them: a trace is worth having because it
    // is a record of *this* run, and a run that asked its machine for something
    // different for being recorded would be a record of a run nobody had.
    //
    // A run that does everything a bring-up can: it is asked for permission it
    // does not have yet, opens and tunes the output, establishes an identity,
    // binds the link and hands over the loop that serves it — and warns about
    // what it is taking on the way past.
    fn once(mut mac: SimHost, trace: Option<&mut [u8]>) -> (Vec<Did>, Vec<String>, Vec<Written>) {
        mac.attach_built_in(BUILT_IN);
        mac.tap(BUILT_IN, Key::S);
        sink::run(
            &Request::Injecting { listen: true },
            Layout::dudrack(),
            None,
            InputConfig::default(),
            &mut mac,
            trace,
        );
        (mac.did(), mac.warnings().to_vec(), decode(&mac))
    }

    /// The same machine twice, recorded and not, and what it was asked for.
    fn recorded_and_not(make: impl Fn() -> SimHost) -> Vec<Did> {
        let mut memory = vec![0u8; 64 * 1024];
        let recorded = once(make(), Some(&mut memory));
        assert_eq!(
            recorded,
            once(make(), None),
            "the recording changed the run"
        );
        recorded.0
    }

    let served = recorded_and_not(|| SimHost::new().with_link(SimLink::new(paired_list())));
    assert!(
        served.contains(&Did::BoundLink) && served.contains(&Did::OpenedOutput),
        "this only says anything while the run really did all of it: {served:?}"
    );

    // A machine that has to be asked for the permission it does not have: the one
    // bring-up call a run that already has it never makes.
    let asked = recorded_and_not(|| SimHost::new().with_no_permission());
    assert!(
        asked.contains(&Did::RequestedPermission),
        "the permission has to have been asked for: {asked:?}"
    );

    // And a machine with nothing watching it, which is the run that warns
    // (ADR-0008): the line has to reach the log of a recorded run as well, since
    // a person reading it is being told the keyboards are at risk *now*.
    let mut memory = vec![0u8; 64 * 1024];
    let (_, said, _) = once(SimHost::new().with_no_watchdog(), Some(&mut memory));
    assert_eq!(
        said.iter().filter(|line| line.contains("watchdog")).count(),
        1,
        "a recorded run has to warn the same way: {said:?}"
    );
}

#[test]
fn a_long_stretch_between_checkpoints_is_bounded_by_the_records_as_well() {
    // Two bounds, because they bound different things: a minute of typing is a
    // few hundred records and a minute of pointer movement is tens of thousands,
    // so a window fixed by the clock alone would reach back further for one
    // person than another. The count is what holds in a region large enough that
    // a quarter of it is more records than the count allows.
    let mut memory = vec![0u8; 512 * 1024];
    let mut host = SimHost::new();
    host.attach_external(TRACKPOINT, 6127, 24801);
    for n in 0..600 {
        host.pointer(TRACKPOINT, PointerReport::moved(n % 3, 1));
    }
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        Some(&mut memory),
    );

    let trace = Trace::read(&memory);
    assert_eq!(
        trace.evicted(),
        0,
        "the region is big enough to hold it all"
    );
    assert!(
        trace.checkpoints() > 1,
        "so what put a second checkpoint in is the record count: {}",
        trace.checkpoints()
    );
}

#[test]
fn every_checkpoint_a_trace_holds_is_a_case_of_its_own() {
    // What makes a field trace usable as a case: a person given one asks it to
    // reproduce the incident, and the incident is at the *end* of the window.
    // Starting at the oldest checkpoint replays everything that survived;
    // starting at a later one reproduces the same ending from a state closer to
    // it, which is the difference between a case that takes the whole window and
    // one that takes a moment.
    let (written, memory) = record(4096);
    let trace = Trace::read(&memory);
    let checkpoints = trace.checkpoints();
    assert!(
        checkpoints > 1,
        "this only says anything while the run really left several: {checkpoints}"
    );

    let mut lengths = Vec::new();
    for from in 0..checkpoints {
        let (tail, replayed) = replaying(&memory, None, from);
        assert!(
            matches!(replayed, Replayed::Events(events) if events > 0),
            "checkpoint {from} has to carry the events after it; got {replayed:?}"
        );
        assert!(
            written.ends_with(&tail),
            "a replay from checkpoint {from} has to reproduce the end of the run;\n  \
             whole: {written:?}\n  tail: {tail:?}"
        );
        lengths.push(tail.len());
    }

    assert!(
        lengths.windows(2).all(|pair| pair[0] >= pair[1]),
        "a later checkpoint cannot reproduce more than an earlier one: {lengths:?}"
    );
    assert!(
        lengths.first() > lengths.last(),
        "and the last one has to be the shorter case, or there is nothing to \
         choose between them: {lengths:?}"
    );
}

#[test]
fn a_checkpoint_the_trace_does_not_hold_replays_nothing() {
    // A person naming a checkpoint that has since been evicted past, or one a
    // shorter run never reached. Typing nothing is the only safe answer: a
    // replay that fell back to whatever checkpoint *is* there would reproduce a
    // stretch of the run nobody asked about and say it was the one they named.
    let (_, memory) = record(64 * 1024);
    let trace = Trace::read(&memory);
    let (written, replayed) = replaying(&memory, None, trace.checkpoints());

    assert_eq!(replayed, Replayed::NoSuchCheckpoint);
    assert_eq!(written, Vec::new());
}

#[test]
fn a_region_that_holds_no_trace_replays_nothing() {
    // What a replay is handed is a region of memory a run may or may not have
    // written: a file from a machine that never recorded one, or a page the
    // supervisor reserved and the run died before touching. Typing nothing is
    // the only safe answer — bytes read as records anyway would be keystrokes
    // nobody pressed.
    assert_eq!(replay(&vec![0u8; 4096]), Vec::new());
}

#[test]
fn a_replay_drops_the_records_it_cannot_read() {
    // The region is written by a live run and read by whoever comes after it, so
    // what a reader finds can be a record half written or bytes something else
    // left there. Dropping those is what keeps a replay honest: a reader that
    // guessed at a kind it does not know would type a keystroke that never
    // happened.
    let (written, mut memory) = record(64 * 1024);
    assert!(!written.is_empty(), "the script has to produce something");
    memory[favjit_engine::trace::HEADER..].fill(0xFF);

    assert_eq!(replay(&memory), Vec::new());
}

#[test]
fn a_replay_drops_a_record_that_was_only_half_written() {
    // The watchdog reads the region while the run is still writing into it, so a
    // record it finds can be the first bytes of one and the last bytes of
    // whatever was there before. The kinds that carry a flag are where that shows
    // up as something readable and wrong: a checkpoint claiming a repeat with no
    // key behind it, or a held key in a state this format has no name for. Both
    // have to go — replaying a repeat of no key would type a keystroke nobody
    // pressed.
    //
    // Written by hand into the region because that is what a torn write leaves:
    // the flag byte of every record set, over a recording that was whole.
    // The script that leaves keys held at a checkpoint, so the region holds the
    // records a held key is written as: those are half of what a torn flag byte
    // makes unreadable, and a recording without them would only show the other.
    let (written, mut memory) = recording(4096, None, held_across_a_checkpoint);
    assert!(!written.is_empty(), "the script has to produce something");
    for record in memory[favjit_engine::trace::HEADER..]
        .chunks_mut(favjit_engine::trace::RECORD)
        .filter(|record| record[0] != 0)
    {
        record[1] = 0xFF;
    }

    assert_eq!(
        replay(&memory),
        Vec::new(),
        "a checkpoint that cannot be read is not a checkpoint to start from"
    );
}

#[test]
fn a_trace_of_a_quiet_run_is_replayable_too() {
    // Nothing typed, one keyboard attached. The empty case is where an
    // off-by-one in the ring shows up as a trace that cannot be read at all.
    let mut memory = vec![0u8; 4 * 1024];
    let mut host = SimHost::new();
    host.attach_built_in(BUILT_IN);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        Some(&mut memory),
    );

    assert_eq!(replay(&memory), Vec::new());
}
