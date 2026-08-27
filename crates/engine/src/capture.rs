//! The loops that read each machine's own keyboards, which its host turns for it
//! (ADR-0006).
//!
//! Here rather than beside the platform calls for the reason the link's loop and
//! the output device's are: what a loop does is a sequence, and a sequence
//! written where no test can reach it is a sequence held by whoever last read
//! it. What is left in a host is the calls — a registry entry, a window, a turn,
//! the probes — and this is the order over them.
//!
//! **Two loops and not one.** The forwarding machine's keyboards are watched
//! being typed on and refused with a hook; the converting machine's are taken
//! away and read off a queue. Nothing about those is the same sequence, so a
//! loop written to be either would be a loop that asked each machine about the
//! other's calls — and each host would answer half of them with nothing. What
//! *is* the same is the turn, the probes, the stream and whether the run is over
//! ([`favjit_host::capture::Reading`]), and that is what the two share here.

use core::time::Duration;

use favjit_host::capture::{Ask, Found, Reading, SinkCapture, SourceCapture};
use favjit_host::{DeviceId, EventKind, Handed};

/// Every device this loop has been told about, by the platform's own name for
/// it.
///
/// The platform's name and not the number `engine` gave it, because that is what
/// says two findings are the same device: a keyboard that arrives while the
/// devices already here are being listed is come across twice, and the number is
/// what this loop hands out rather than what it recognises by.
#[derive(Default)]
struct Known {
    named: Vec<u64>,
    next: u64,
}

impl Known {
    /// The number to offer the next device, which is one nothing holds.
    fn offer(&self) -> DeviceId {
        DeviceId(self.next)
    }

    /// Keep this one under the name the platform gave it, and say whether it is
    /// new.
    fn keep(&mut self, named: u64) -> bool {
        let new = !self.named.contains(&named);
        self.named.extend(new.then_some(named));
        self.next += u64::from(new);
        new
    }
}

/// Read the converting machine's own keyboards until the run is over.
///
/// **Asking to be told about arrivals comes before listing what is here**, which
/// is why they are separate calls. The other order has a gap in it: a keyboard
/// plugged in between the listing and the asking is in neither, so it is never
/// read and never converted. This order's cost is the opposite and it is not a
/// cost — a device that arrives in between is come across twice, and the second
/// finding is recognised by the registry's own name for it and dropped.
///
/// The port before the notification and the source before either is used,
/// because that is the order IOKit takes them in: a notification asked for on no
/// port is asked of nothing, and a port with no source on this loop delivers
/// nowhere. Stopped at the first that would not go, since each is what the next
/// is made on.
///
/// The loop itself is a turn and then the drains. The turn is where IOKit
/// delivers what it has, so everything else comes after it: the probes first,
/// because a supervisor with an unanswered probe ends this process and the
/// keyboards go with it (ADR-0008); then what the run has asked of this loop,
/// because a run waiting on an answer is a run that has not taken a keyboard
/// yet; then the devices, because a device found is worth nothing until
/// something asks to read it; and the values and the removals last, named by the
/// numbers that drain handed out.
pub fn convert(host: &mut dyn SinkCapture, poll: Duration) {
    let told = host.open_a_notification_port() && host.ask_to_be_told_about_devices();
    let _ = told.then(|| host.put_that_port_on_this_loop());
    let listed = told && host.look_at_the_devices_here();
    let mut known = Known::default();
    let _ = listed.then(|| {
        // Once before the first turn, so a run whose keyboards are all already
        // attached does not wait a poll interval to hear about them.
        let mut reading = drain_devices(host, &mut known);
        while reading && !host.over() {
            host.turn_the_loop(poll);
            reading = answer_probes(host);
            answer_what_was_asked(host);
            reading &= drain_devices(host, &mut known);
            reading &= drain_values(host);
            reading &= drain_gone(host);
        }
    });
}

/// Read the forwarding machine's own keyboards until the run is over.
///
/// The class before the window, the window before the registration, and where
/// the keys go before any of them can arrive: that is the order Win32 takes them
/// in, and each is what the next is asked of. The chord and the probe timer come
/// after, because both are read by things that only run once input is arriving.
///
/// The hooks last, and only where the run asked to refuse anything: a hook is
/// what takes the keyboard away, so a run asked to refuse nothing must not
/// install one. Both or neither — the keyboard's is let go of by its own `Drop`
/// when the pointer's stops the run, so a run that got one of the two forwards
/// nothing rather than half of what was typed.
///
/// The loop itself is a turn and then the drains, in the order the converting
/// loop takes them, with the message's payload read out of the turn before
/// anything is drained and given back after: it is memory the system allocated
/// to deliver with, so reading it once it has been given back is reading freed
/// memory, and never giving it back is that memory held for the life of the run.
pub fn relay(host: &mut dyn SourceCapture, poll: Duration, refusing: bool) {
    host.register_the_window_class();
    let watching = host.make_a_window() && host.ask_for_the_mice();
    let _ = watching.then(|| {
        host.send_the_keys_to_that_window();
        host.state_the_chord();
        host.start_the_probe_timer();
    });
    let ready = watching && (!refusing || (host.hook_the_keyboard() && host.hook_the_pointer()));
    let mut known = Known::default();
    let _ = ready.then(|| {
        let mut reading = true;
        while reading && !host.over() {
            host.turn_the_loop(poll);
            // Whether that turn produced anything at all, which is what says
            // there is a payload to take out of it and give back afterwards: a
            // wait that came back with nothing handed nothing over.
            let turned = !host.over();
            let _ = turned.then(|| host.take_what_the_turn_produced());
            reading = answer_probes(host);
            reading &= relay_devices(host, &mut known);
            reading &= drain_arrivals(host);
            let _ = turned.then(|| host.let_go_of_the_turn());
        }
    });
}

/// One event per probe, so what the supervisor is owed and what this loop
/// answers are the same count (ADR-0008).
///
/// Answers whether anything is still reading, for the reason every drain here
/// does: a loop that went on turning for a run that has gone is a thread
/// holding this machine's hooks with nobody to hand a keystroke to.
fn answer_probes(host: &mut dyn Reading) -> bool {
    (0..host.take_probes()).fold(true, |reading, _| reading & host.deliver(EventKind::Probe))
}

/// Put everything the message carried on the run's stream, in the order it came.
///
/// After the devices, because a report is what says there is a device: a drain
/// that ran first would name one by a number nothing had handed out.
fn drain_arrivals(host: &mut dyn SourceCapture) -> bool {
    let mut reading = true;
    while let Some(arrived) = host.next_arrival() {
        reading &= host.deliver(arrived);
    }
    reading
}

/// Every value off every queue IOKit said had some.
///
/// Read here rather than where IOKit said so, because that is a procedure it
/// calls from inside the turn: a drain written there is every keystroke read and
/// delivered in the one place no test reaches (ADR-0006). It is the same turn
/// either way, so nothing waits longer for it.
fn drain_values(host: &mut dyn SinkCapture) -> bool {
    let mut reading = true;
    while let Some(from) = host.next_thing_with_values() {
        while let Some(value) = host.next_value_of(from) {
            // Before the value and not after: what a reading of a trace is
            // built on is the order, and the measurement is about the value
            // behind it.
            let measured = stamp_of(&value).map(|stamp| host.how_long_ago(stamp));
            if let Some(delay) = measured {
                reading &= host.deliver(EventKind::Delay(delay));
            }
            reading &= host.deliver(value);
        }
        let done = host.nothing_more_of(from);
        reading &= host.deliver(done);
        host.done_with(from);
    }
    reading
}

/// IOKit's own stamp on a value, where the event carries one.
fn stamp_of(event: &EventKind) -> Option<u64> {
    match event {
        EventKind::HidValue { stamp, .. } => Some(*stamp),
        _ => None,
    }
}

/// Every device IOKit said had gone, given back in the order it takes.
///
/// Its queue stopped before it is let go of, because letting go of a running one
/// leaves the run loop holding a source over memory nothing owns; and the device
/// given back before it is let go of, for the same reason. Reported last, so
/// that what the run reads about a keyboard going is a keyboard that has already
/// been given back.
fn drain_gone(host: &mut dyn SinkCapture) -> bool {
    let mut reading = true;
    while let Some(device) = host.next_gone() {
        if let Some(queue) = host.its_queue(device) {
            host.stop_it_delivering(queue);
            host.let_go_of_the_queue(queue);
        }
        host.give_it_back(device);
        let was = host.forget_it(device);
        host.let_go_of_the_device(device);
        if let Some(device) = was {
            reading &= host.deliver(EventKind::DeviceLost(device));
        }
    }
    reading
}

/// Take every registry entry IOKit has waiting, keeping the devices this loop
/// has not seen.
///
/// Drained to the end rather than one per turn: IOKit re-arms its telling only
/// once its iterator is empty, so a drain that stopped early would be the last
/// arrival this loop ever heard about.
fn drain_devices(host: &mut dyn SinkCapture, known: &mut Known) -> bool {
    let mut reading = true;
    while let Some(found) = one_device(host, known.offer()) {
        let new = known.keep(found.named);
        // Let go of the second finding of one device rather than keeping it: the
        // handle behind it is one more open on a device already open, and a
        // seize on that one takes hold of nothing.
        let _ = (!new).then(|| host.forget(known.offer()));
        if new {
            reading &= host.deliver(found.found);
        }
    }
    reading
}

/// Take every device a report has named that nothing has numbered.
///
/// One call each rather than the sequence the converting loop drives: a device
/// here is not opened at all, because the report that named it is already
/// arriving. Numbered under the name the machine gave the handle, so that a
/// device this loop has already numbered is recognised rather than numbered
/// twice.
fn relay_devices(host: &mut dyn SourceCapture, known: &mut Known) -> bool {
    let mut reading = true;
    while let Some(found) = host.next_device(known.offer()) {
        let new = known.keep(found.named);
        if new {
            reading &= host.deliver(found.found);
        }
    }
    reading
}

/// Answer everything the run has asked of this loop, each ask as the calls it
/// is made of.
///
/// Drained to the end rather than one per turn: what the run asks it then waits
/// on, so an ask left unanswered is a run that has taken nothing until the next
/// turn comes round.
fn answer_what_was_asked(host: &mut dyn SinkCapture) {
    while let Some(ask) = host.next_ask() {
        let code = match ask {
            Ask::Take { device, exclusive } => seize(host, device, exclusive),
            Ask::Read { device } => start_reading(host, device),
            // `Ask` is `#[non_exhaustive]`: one this crate does not yet name is
            // answered as a call that could not be made, which is the number
            // every other unmade call answers with.
            _ => NOTHING_WAS_ASKED,
        };
        host.answer_the_ask(code);
    }
}

/// What a call that was never made answers with.
///
/// The platform's own numbers are what every other answer here is, so this is
/// one no platform uses: `-1` is what a call that could not be made stands for
/// wherever a host answers a code at all.
const NOTHING_WAS_ASKED: i32 = -1;

/// Take one keyboard away from everything else, and keep it among the ones this
/// run has to give back.
///
/// The keeping is its own call and after the open, because what has to be given
/// back is what the open says it took: a run that wrote it down first would have
/// a keyboard to release that it never held.
fn seize(host: &mut dyn SinkCapture, device: Handed, exclusive: bool) -> i32 {
    let code = host.seize(device, exclusive);
    host.keep_what_was_seized(device, code, exclusive);
    code
}

/// Start one keyboard delivering everything it reports, one call at a time.
///
/// The elements before the queue, because a queue with nothing on it wakes for
/// nothing; then the callback, the schedule and the start, which is the order
/// the API takes — a queue started before it is scheduled delivers nothing, and
/// one scheduled with no callback registered wakes this loop for nobody.
fn start_reading(host: &mut dyn SinkCapture, device: Handed) -> i32 {
    let Some(elements) = host.every_element_of(device) else {
        return NOTHING_WAS_ASKED;
    };
    let queue = host.make_a_queue_over(device);
    let filled = match queue {
        Some(queue) => put_the_elements_on(host, queue, elements),
        None => false,
    };
    // The elements are given back whichever way it went: they are a copy this
    // asked for, and the queue holds what it took from them.
    host.let_go_of_the_elements(elements);
    let Some(queue) = queue else {
        return NOTHING_WAS_ASKED;
    };
    // A queue nothing was put on wakes for nothing, so it is given back rather
    // than kept as a device that will never report.
    if !filled {
        host.let_go_of_the_queue(queue);
        return NOTHING_WAS_ASKED;
    }
    host.watch_for_its_values(queue);
    host.put_it_where_values_arrive(queue);
    host.start_it_delivering(queue);
    host.keep_the_queue(queue, device);
    0
}

/// Every element of that array onto that queue, and whether any went on.
///
/// Every one of them, and one that would not go is passed over rather than
/// ending it: a queue with some of a device's elements on it reads some of its
/// keys, and a queue with none of them wakes for nothing.
fn put_the_elements_on(host: &mut dyn SinkCapture, queue: Handed, elements: Handed) -> bool {
    let how_many = host.how_many_elements_of(elements);
    (0..how_many).fold(false, |on, at| {
        // Asked about before it is put on, because a place the array holds
        // nothing at is not an element.
        on | (host.has_an_element_at(elements, at) && host.put_one_element_on(queue, elements, at))
    })
}

/// One device the platform came across, taken through every call it takes to
/// become one the run can ask for.
///
/// The order is here and each step is one call, because each is something the
/// next is made on: the finding, the name it goes under, the device made from
/// it, and only then what it says about itself. A sequence written behind those
/// calls is a sequence held by whoever last read it (ADR-0006).
///
/// `None` where the platform has none waiting *and* where the one it had could
/// not be made into a device — both are this drain having nothing to deliver
/// this time round, and a drain that carried on past the second would ask the
/// machine about a device that was never made.
fn one_device(host: &mut dyn SinkCapture, as_this: DeviceId) -> Option<Found> {
    let found = host.next_device()?;
    let named = host.name_of(found);
    let device = host.open_it(found);
    // The finding is given back either way and before anything is asked of the
    // device: it is a reference this run took, and the device made from it holds
    // whatever it needed of it.
    host.let_go_of_the_finding(found);
    let device = device?;
    // Let go of a device the platform would not name: without a name there is no
    // telling it from one already being read, and reading one twice converts
    // every keystroke twice.
    let Some(named) = named else {
        host.let_go_of_the_device(device);
        return None;
    };
    // The callback before the schedule, because that order is what the API takes
    // and not a choice: a device scheduled with nothing registered delivers its
    // removal to nobody.
    host.watch_for_its_removal(device);
    host.put_it_where_reports_arrive(device);
    let told = host.describe(device, as_this);
    host.keep(device, as_this);
    Some(Found { named, found: told })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What this platform's stand-in numbers an elements array and a queue for
    /// a device by, so that a test can tell the three apart in what it asserts.
    const ELEMENTS: u64 = 100;
    const QUEUES: u64 = 200;

    /// The calls every converting loop makes before it can read anything.
    ///
    /// Named so that a test about one of the drains can leave them out: what the
    /// setup is has a test of its own, and restating it in every other
    /// assertion is the same order written down several times.
    const SETTING_UP: [&str; 4] = [
        "open a notification port",
        "ask to be told about devices",
        "put that port on this loop",
        "look at the devices here",
    ];

    /// What a loop did once it was set up.
    fn after_the_setup(did: &[&'static str]) -> Vec<&'static str> {
        did.iter()
            .skip_while(|one| SETTING_UP.contains(one))
            .copied()
            .collect()
    }

    /// The converting machine's stand-in, with these devices to be found, in
    /// this order.
    struct Platform {
        /// What each of the three calls that has to go before a device can be
        /// found answers with.
        opens_a_port: bool,
        takes_the_notification: bool,
        lists: bool,
        found: Vec<u64>,
        /// The findings it will not name, and the ones it will not make a
        /// device out of.
        nameless: Vec<u64>,
        unopenable: Vec<u64>,
        /// What was given back, watched, scheduled and kept, in order.
        findings_let_go: Vec<u64>,
        devices_let_go: Vec<u64>,
        watched: Vec<u64>,
        scheduled: Vec<u64>,
        kept: Vec<u64>,
        forgotten: Vec<DeviceId>,
        delivered: Vec<EventKind>,
        probes: Vec<usize>,
        answered: usize,
        /// What the run has asked of it, and what it answered each with.
        asks: Vec<Ask>,
        answers: Vec<i32>,
        /// The findings it will not seize, will not name elements of, or will
        /// not make a queue over — and whether nothing goes on a queue at all.
        will_not_seize: Vec<u64>,
        elementless: Vec<u64>,
        queueless: Vec<u64>,
        /// How many elements a device declares, and whether any of them goes on
        /// a queue.
        elements: usize,
        holds_nothing_at: Vec<usize>,
        nothing_goes_on: bool,
        /// The queues that have said they have values, what is on one, and the
        /// devices that have said they have gone.
        with_values: Vec<u64>,
        values: Vec<u32>,
        gone: Vec<u64>,
        /// Every call one of the sequences made, in order.
        did: Vec<&'static str>,
        turns: usize,
        turns_left: usize,
        /// Whether anything is reading what this loop delivers.
        reading: bool,
    }

    impl Platform {
        fn with(found: Vec<u64>) -> Self {
            Self {
                opens_a_port: true,
                takes_the_notification: true,
                lists: true,
                found,
                nameless: Vec::new(),
                unopenable: Vec::new(),
                findings_let_go: Vec::new(),
                devices_let_go: Vec::new(),
                watched: Vec::new(),
                scheduled: Vec::new(),
                kept: Vec::new(),
                forgotten: Vec::new(),
                delivered: Vec::new(),
                probes: Vec::new(),
                answered: 0,
                asks: Vec::new(),
                answers: Vec::new(),
                will_not_seize: Vec::new(),
                elementless: Vec::new(),
                queueless: Vec::new(),
                elements: 1,
                holds_nothing_at: Vec::new(),
                nothing_goes_on: false,
                with_values: Vec::new(),
                values: Vec::new(),
                gone: Vec::new(),
                did: Vec::new(),
                turns: 0,
                turns_left: 1,
                reading: true,
            }
        }
    }

    impl Reading for Platform {
        fn turn_the_loop(&mut self, _poll: Duration) {
            self.turns += 1;
            self.turns_left = self.turns_left.saturating_sub(1);
        }

        fn take_probes(&mut self) -> usize {
            self.probes.pop().unwrap_or(0)
        }

        fn deliver(&mut self, event: EventKind) -> bool {
            self.delivered.push(event);
            self.reading
        }

        fn over(&mut self) -> bool {
            self.turns_left == 0
        }
    }

    impl SinkCapture for Platform {
        fn open_a_notification_port(&mut self) -> bool {
            self.did.push("open a notification port");
            self.opens_a_port
        }

        fn ask_to_be_told_about_devices(&mut self) -> bool {
            self.did.push("ask to be told about devices");
            self.takes_the_notification
        }

        fn put_that_port_on_this_loop(&mut self) {
            self.did.push("put that port on this loop");
        }

        fn look_at_the_devices_here(&mut self) -> bool {
            self.did.push("look at the devices here");
            self.lists
        }

        fn next_device(&mut self) -> Option<favjit_host::Handed> {
            let named = (!self.found.is_empty()).then(|| self.found.remove(0));
            named.map(favjit_host::Handed)
        }

        fn name_of(&mut self, found: favjit_host::Handed) -> Option<u64> {
            (!self.nameless.contains(&found.0)).then_some(found.0)
        }

        fn open_it(&mut self, found: favjit_host::Handed) -> Option<favjit_host::Handed> {
            (!self.unopenable.contains(&found.0)).then_some(found)
        }

        fn let_go_of_the_finding(&mut self, found: favjit_host::Handed) {
            self.findings_let_go.push(found.0);
        }

        fn watch_for_its_removal(&mut self, device: favjit_host::Handed) {
            self.watched.push(device.0);
        }

        fn put_it_where_reports_arrive(&mut self, device: favjit_host::Handed) {
            self.scheduled.push(device.0);
        }

        fn describe(&mut self, _device: favjit_host::Handed, as_this: DeviceId) -> EventKind {
            EventKind::DeviceLost(as_this)
        }

        fn keep(&mut self, device: favjit_host::Handed, _as_this: DeviceId) {
            self.kept.push(device.0);
        }

        fn let_go_of_the_device(&mut self, device: favjit_host::Handed) {
            self.devices_let_go.push(device.0);
        }

        fn forget(&mut self, device: DeviceId) {
            self.forgotten.push(device);
        }

        fn next_ask(&mut self) -> Option<Ask> {
            self.answered += 1;
            (!self.asks.is_empty()).then(|| self.asks.remove(0))
        }

        fn seize(&mut self, device: Handed, exclusive: bool) -> i32 {
            self.did.push("seize");
            match exclusive && self.will_not_seize.contains(&device.0) {
                true => -1,
                false => 0,
            }
        }

        fn keep_what_was_seized(&mut self, _device: Handed, _code: i32, _exclusive: bool) {
            self.did.push("keep what was seized");
        }

        fn every_element_of(&mut self, device: Handed) -> Option<Handed> {
            self.did.push("every element of");
            (!self.elementless.contains(&device.0)).then_some(Handed(device.0 + ELEMENTS))
        }

        fn make_a_queue_over(&mut self, device: Handed) -> Option<Handed> {
            self.did.push("make a queue over");
            (!self.queueless.contains(&device.0)).then_some(Handed(device.0 + QUEUES))
        }

        fn how_many_elements_of(&mut self, _elements: Handed) -> usize {
            self.did.push("how many elements of");
            self.elements
        }

        fn has_an_element_at(&mut self, _elements: Handed, at: usize) -> bool {
            !self.holds_nothing_at.contains(&at)
        }

        fn put_one_element_on(&mut self, _queue: Handed, _elements: Handed, _at: usize) -> bool {
            self.did.push("put one element on");
            !self.nothing_goes_on
        }

        fn let_go_of_the_elements(&mut self, _elements: Handed) {
            self.did.push("let go of the elements");
        }

        fn watch_for_its_values(&mut self, _queue: Handed) {
            self.did.push("watch for its values");
        }

        fn put_it_where_values_arrive(&mut self, _queue: Handed) {
            self.did.push("put it where values arrive");
        }

        fn start_it_delivering(&mut self, _queue: Handed) {
            self.did.push("start it delivering");
        }

        fn keep_the_queue(&mut self, _queue: Handed, _device: Handed) {
            self.did.push("keep the queue");
        }

        fn let_go_of_the_queue(&mut self, _queue: Handed) {
            self.did.push("let go of the queue");
        }

        fn answer_the_ask(&mut self, code: i32) {
            self.answers.push(code);
        }

        fn next_thing_with_values(&mut self) -> Option<Handed> {
            self.with_values.first().copied().map(Handed)
        }

        fn next_value_of(&mut self, from: Handed) -> Option<EventKind> {
            (!self.values.is_empty()).then(|| EventKind::HidValue {
                device: DeviceId(from.0),
                page: 0,
                usage: self.values.remove(0),
                stamp: 0,
                value: 1,
            })
        }

        fn how_long_ago(&mut self, _stamp: u64) -> u64 {
            0
        }

        fn nothing_more_of(&mut self, from: Handed) -> EventKind {
            EventKind::HidValuesDone {
                device: DeviceId(from.0),
            }
        }

        fn done_with(&mut self, from: Handed) {
            self.with_values.retain(|one| *one != from.0);
        }

        fn next_gone(&mut self) -> Option<Handed> {
            self.gone.first().copied().map(Handed)
        }

        fn its_queue(&mut self, device: Handed) -> Option<Handed> {
            (!self.queueless.contains(&device.0)).then_some(Handed(device.0 + QUEUES))
        }

        fn stop_it_delivering(&mut self, _queue: Handed) {
            self.did.push("stop it delivering");
        }

        fn give_it_back(&mut self, _device: Handed) -> i32 {
            self.did.push("give it back");
            0
        }

        fn forget_it(&mut self, device: Handed) -> Option<DeviceId> {
            self.did.push("forget it");
            self.gone.retain(|one| *one != device.0);
            Some(DeviceId(device.0))
        }
    }

    /// The forwarding machine's stand-in, with these devices to be named by the
    /// reports that arrive.
    struct Watching {
        makes_a_window: bool,
        takes_the_mice: bool,
        hooks: bool,
        found: Vec<u64>,
        delivered: Vec<EventKind>,
        /// What a turn produced and could not name, oldest first.
        waiting: Vec<EventKind>,
        /// Every call the setup made, in order, and which turns had their
        /// payload taken out of them and given back.
        did: Vec<&'static str>,
        took: Vec<usize>,
        let_go: Vec<usize>,
        turns: usize,
        turns_left: usize,
        reading: bool,
    }

    impl Watching {
        fn with(found: Vec<u64>) -> Self {
            Self {
                makes_a_window: true,
                takes_the_mice: true,
                hooks: true,
                found,
                delivered: Vec::new(),
                waiting: Vec::new(),
                did: Vec::new(),
                took: Vec::new(),
                let_go: Vec::new(),
                turns: 0,
                turns_left: 1,
                reading: true,
            }
        }
    }

    impl Reading for Watching {
        fn turn_the_loop(&mut self, _poll: Duration) {
            self.turns += 1;
            self.turns_left = self.turns_left.saturating_sub(1);
        }

        fn take_probes(&mut self) -> usize {
            0
        }

        fn deliver(&mut self, event: EventKind) -> bool {
            self.delivered.push(event);
            self.reading
        }

        fn over(&mut self) -> bool {
            self.turns_left == 0
        }
    }

    impl SourceCapture for Watching {
        fn register_the_window_class(&mut self) {
            self.did.push("register the window class");
        }

        fn make_a_window(&mut self) -> bool {
            self.did.push("make a window");
            self.makes_a_window
        }

        fn ask_for_the_mice(&mut self) -> bool {
            self.did.push("ask for the mice");
            self.takes_the_mice
        }

        fn send_the_keys_to_that_window(&mut self) {
            self.did.push("send the keys to that window");
        }

        fn state_the_chord(&mut self) {
            self.did.push("state the chord");
        }

        fn start_the_probe_timer(&mut self) {
            self.did.push("start the probe timer");
        }

        fn hook_the_keyboard(&mut self) -> bool {
            self.did.push("hook the keyboard");
            self.hooks
        }

        fn hook_the_pointer(&mut self) -> bool {
            self.did.push("hook the pointer");
            self.hooks
        }

        fn next_device(&mut self, as_this: DeviceId) -> Option<Found> {
            let named = (!self.found.is_empty()).then(|| self.found.remove(0));
            named.map(|named| Found {
                named,
                found: EventKind::DeviceLost(as_this),
            })
        }

        fn take_what_the_turn_produced(&mut self) {
            self.took.push(self.turns);
        }

        fn let_go_of_the_turn(&mut self) {
            self.let_go.push(self.turns);
        }

        fn next_arrival(&mut self) -> Option<EventKind> {
            (!self.waiting.is_empty()).then(|| self.waiting.remove(0))
        }
    }

    #[test]
    fn a_device_come_across_twice_is_kept_once() {
        // The order this loop asks in produces the second finding on purpose: a
        // keyboard that arrives while the ones already here are being listed is
        // in both, and reading one device twice converts every keystroke twice.
        let mut platform = Platform::with(vec![7, 7, 9]);

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(
            platform.delivered,
            vec![
                EventKind::DeviceLost(DeviceId(0)),
                EventKind::DeviceLost(DeviceId(1)),
            ],
            "one for each device, numbered in the order they were found"
        );
        assert_eq!(
            platform.forgotten,
            vec![DeviceId(1)],
            "the second finding is let go of under the number it was offered"
        );
    }

    #[test]
    fn nothing_is_read_where_no_port_can_be_opened_to_hear_about_devices_on() {
        // Turning a loop that will never hear about a device is a loop that
        // holds a thread and reads nothing, which reads as a wedge from outside.
        let mut platform = Platform::with(vec![7]);
        platform.opens_a_port = false;

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(platform.turns, 0);
        assert_eq!(platform.delivered, Vec::new());
    }

    #[test]
    fn the_converting_loop_asks_in_the_order_iokit_takes() {
        // Each is what the next is made on: a notification asked for on no port
        // is asked of nothing, a port with no source on this loop delivers
        // nowhere, and listing what is already here comes after the asking so
        // that a keyboard plugged in between the two is in one of them.
        let mut platform = Platform::with(Vec::new());

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(
            platform.did,
            vec![
                "open a notification port",
                "ask to be told about devices",
                "put that port on this loop",
                "look at the devices here",
            ]
        );
    }

    #[test]
    fn nothing_is_asked_of_a_port_the_machine_would_not_take_a_notification_on() {
        // The source that port carries is what delivers the notification, so a
        // loop that put one on anyway would be waiting on a port nothing was
        // asked of.
        let mut platform = Platform::with(Vec::new());
        platform.takes_the_notification = false;

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(
            platform.did,
            vec!["open a notification port", "ask to be told about devices"]
        );
    }

    #[test]
    fn what_the_run_asked_of_this_loop_is_answered_on_every_turn() {
        // A run asks this loop to open a keyboard and waits for the answer, so a
        // turn that came round without answering is a run that has taken
        // nothing — and nothing to type on until the next one.
        let mut platform = Platform::with(Vec::new());
        platform.turns_left = 3;

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(platform.answered, platform.turns);
    }

    #[test]
    fn nothing_is_read_where_what_is_already_attached_cannot_be_listed() {
        // The two are asked for in an order, and a run that carried on without
        // the second would read only the keyboards plugged in after it started.
        let mut platform = Platform::with(vec![7]);
        platform.lists = false;

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(platform.turns, 0);
        assert_eq!(platform.delivered, Vec::new());
    }

    #[test]
    fn a_loop_stops_turning_once_nothing_is_reading_what_it_delivers() {
        // The loop is the writer and the run is the reader, so a write finding
        // nobody there is how a run that has gone is ever learned: one that kept
        // turning would be a thread holding this machine's hooks with nobody to
        // hand a keystroke to (ADR-0008).
        let mut platform = Platform::with(vec![7]);
        platform.reading = false;
        platform.turns_left = 5;

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(platform.turns, 0, "not one turn after the first delivery");
    }

    #[test]
    fn the_forwarding_loop_asks_in_the_order_win32_takes() {
        // Each is what the next is asked of: there is no window of a class that
        // does not exist, no registration for a window that was not made, and
        // nowhere for a hook to hand its keys until the window has been named.
        let mut watching = Watching::with(Vec::new());

        relay(&mut watching, Duration::from_millis(1), true);

        assert_eq!(
            watching.did,
            vec![
                "register the window class",
                "make a window",
                "ask for the mice",
                "send the keys to that window",
                "state the chord",
                "start the probe timer",
                "hook the keyboard",
                "hook the pointer",
            ]
        );
    }

    #[test]
    fn a_run_that_refuses_nothing_installs_no_hook() {
        // A hook is what takes the keyboard away, so installing one on a run
        // asked to refuse nothing is the failure that lands on the keyboard the
        // person is typing on.
        let mut watching = Watching::with(Vec::new());

        relay(&mut watching, Duration::from_millis(1), false);

        assert!(!watching.did.contains(&"hook the keyboard"));
        assert!(!watching.did.contains(&"hook the pointer"));
        assert_eq!(watching.turns, 1, "and it still reads what arrives");
    }

    #[test]
    fn nothing_is_read_where_the_window_the_input_arrives_at_cannot_be_made() {
        let mut watching = Watching::with(vec![7]);
        watching.makes_a_window = false;

        relay(&mut watching, Duration::from_millis(1), true);

        assert_eq!(watching.turns, 0);
        assert_eq!(watching.delivered, Vec::new());
    }

    #[test]
    fn nothing_is_read_where_only_one_of_the_two_hooks_went_on() {
        // Both or neither: this is where the keys come from, so a run with one
        // of them forwards half of what was typed.
        let mut watching = Watching::with(vec![7]);
        watching.hooks = false;

        relay(&mut watching, Duration::from_millis(1), true);

        assert_eq!(watching.turns, 0);
        assert_eq!(watching.delivered, Vec::new());
    }

    #[test]
    fn what_a_turn_could_not_name_is_delivered_after_the_device_it_came_from() {
        // The order is the whole of what this drain is for: a machine that
        // learns a device from its first report has nothing to call that report
        // by until the number is handed out, and the number arrives in the
        // device drain of this same turn.
        let mut watching = Watching::with(vec![7]);
        watching.waiting = vec![EventKind::Probe];

        relay(&mut watching, Duration::from_millis(1), true);

        assert_eq!(
            watching.delivered,
            vec![EventKind::DeviceLost(DeviceId(0)), EventKind::Probe],
            "the device it came from first, then what was waiting"
        );
    }

    #[test]
    fn what_a_turn_handed_over_is_given_back_after_it_has_been_read() {
        // The order is the whole of it, because that payload is memory the
        // system allocated to deliver with: read after it is given back is
        // freed memory, and never given back is memory held for the life of the
        // run.
        let mut watching = Watching::with(Vec::new());
        watching.turns_left = 2;

        relay(&mut watching, Duration::from_millis(1), true);

        assert_eq!(
            watching.took,
            vec![1],
            "taken out of the turn that produced one"
        );
        assert_eq!(watching.let_go, vec![1], "and given back on that same turn");
    }

    #[test]
    fn a_turn_that_came_back_with_nothing_hands_nothing_over() {
        // Nothing was produced, so there is nothing to take and nothing to give
        // back — and a machine handed back what it never made is a call about a
        // thing that does not exist.
        let mut watching = Watching::with(Vec::new());
        watching.turns_left = 1;

        relay(&mut watching, Duration::from_millis(1), true);

        assert_eq!(watching.turns, 1);
        assert_eq!(watching.took, Vec::new());
        assert_eq!(watching.let_go, Vec::new());
    }

    #[test]
    fn a_keyboard_is_started_delivering_in_the_order_the_api_takes() {
        // Every step is something the next is made on, and two of them are
        // orders the API dictates rather than choices: a queue started before
        // it is scheduled delivers nothing, and one scheduled with no callback
        // registered wakes this loop for nobody.
        let mut platform = Platform::with(Vec::new());
        platform.asks = vec![Ask::Read { device: Handed(7) }];

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(
            after_the_setup(&platform.did),
            vec![
                "every element of",
                "make a queue over",
                "how many elements of",
                "put one element on",
                "let go of the elements",
                "watch for its values",
                "put it where values arrive",
                "start it delivering",
                "keep the queue",
            ]
        );
        assert_eq!(platform.answers, vec![0], "and the run is told it started");
    }

    #[test]
    fn a_queue_nothing_went_on_is_given_back_rather_than_kept() {
        // One that wakes for nothing is worse than none at all: a run told a
        // keyboard is being read would wait on a device that reports nothing.
        let mut platform = Platform::with(Vec::new());
        platform.asks = vec![Ask::Read { device: Handed(7) }];
        platform.nothing_goes_on = true;

        convert(&mut platform, Duration::from_millis(1));

        assert!(platform.did.contains(&"let go of the queue"));
        assert!(!platform.did.contains(&"keep the queue"));
        assert_eq!(platform.answers, vec![-1]);
    }

    #[test]
    fn the_elements_are_given_back_even_where_no_queue_was_made() {
        // They are a copy this asked for, so one nobody let go of is memory
        // held for the life of the run.
        let mut platform = Platform::with(Vec::new());
        platform.asks = vec![Ask::Read { device: Handed(7) }];
        platform.queueless = vec![7];

        convert(&mut platform, Duration::from_millis(1));

        assert!(platform.did.contains(&"let go of the elements"));
    }

    #[test]
    fn a_keyboard_that_goes_is_given_back_before_the_run_is_told_it_went() {
        // Its queue stopped before it is let go of, because letting go of a
        // running one leaves the machine holding a source over memory nothing
        // owns — and the run reads about a keyboard that has already been given
        // back rather than one still held.
        let mut platform = Platform::with(Vec::new());
        platform.gone = vec![7];
        platform.turns_left = 2;

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(
            after_the_setup(&platform.did),
            vec![
                "stop it delivering",
                "let go of the queue",
                "give it back",
                "forget it"
            ]
        );
        assert_eq!(platform.delivered, vec![EventKind::DeviceLost(DeviceId(7))]);
    }

    #[test]
    fn every_value_a_queue_had_is_delivered_before_what_says_it_ran_dry() {
        // The order is what a report is assembled out of: a run told the queue
        // ran dry before the values on it would end a report the values had not
        // reached yet.
        let mut platform = Platform::with(Vec::new());
        platform.with_values = vec![3];
        platform.values = vec![9];
        platform.turns_left = 2;

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(
            platform.delivered,
            vec![
                EventKind::Delay(0),
                EventKind::HidValue {
                    device: DeviceId(3),
                    page: 0,
                    usage: 9,
                    stamp: 0,
                    value: 1,
                },
                EventKind::HidValuesDone {
                    device: DeviceId(3),
                },
            ]
        );
    }

    #[test]
    fn one_event_answers_one_probe() {
        // The count is the whole of it: a supervisor is owed as many answers as
        // it asked for, and a loop that answered once for a burst would look
        // wedged for the rest of them (ADR-0008).
        let mut platform = Platform::with(Vec::new());
        platform.probes = vec![3];

        convert(&mut platform, Duration::from_millis(1));

        assert_eq!(
            platform.delivered,
            vec![EventKind::Probe, EventKind::Probe, EventKind::Probe]
        );
    }
}
