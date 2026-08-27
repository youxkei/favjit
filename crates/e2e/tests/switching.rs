//! Which machine the keyboard in front of the person is driving.
//!
//! One keyboard cannot drive both: what is relayed has to be refused here, or every
//! keystroke lands on both screens. So a chord moves it — option and `n` to send it to
//! the Mac, option and `s` to bring it back — and these are what that costs.
//!
//! The chord is the *position* and not what the layout makes of it. While the keyboard
//! is this machine's, nothing is converting anything: the keystrokes are this
//! machine's own, so a chord named in the sink's vocabulary is one this end could not
//! recognise. The conversion stays the sink's alone (ADR-0003).
//!
//! Driven through `source::run` rather than any piece of it, because what a chord does
//! is spread across the whole run: it decides whether the keyboards are refused,
//! whether a link is opened, and what the sink is told on the way out.

use favjit_engine::link::Attached;
use favjit_engine::link::Message;
use favjit_engine::source::{self, Request, Suppressing, SWITCH_BACK, SWITCH_TO_THE_SINK};
use favjit_engine::{Buttons, DeviceId, Key, PointerReport};
use favjit_host::source::Driving;
use favjit_host_sim::{identity, SimSource};

const KEYBOARD: DeviceId = DeviceId(1);

/// The sink this machine is pinned to.
const SINK: u8 = 2;

/// The keyboard on the machine these scripts run on, by the vendor and product
/// a rule names it with.
const TRACKPOINT: (u16, u16) = (0x17ef, 0x60e1);

/// What the sink is told when the run announces it.
///
/// Written out here rather than taken from the machine: what the machine
/// presents is an interface path, and reading a vendor and a product out of one
/// is the run's own doing (ADR-0006).
fn keyboard() -> Attached {
    Attached {
        device: KEYBOARD,
        is_built_in: false,
        vendor_id: Some(TRACKPOINT.0),
        product_id: Some(TRACKPOINT.1),
    }
}

/// A machine with a keyboard on it, which is where a run comes up: the keyboard is this
/// machine's and nothing is being sent anywhere (ADR-0013).
fn forwarding() -> SimSource {
    let mut host = SimSource::new();
    host.pinned_to(identity(SINK));
    host.attach_external(KEYBOARD, TRACKPOINT.0, TRACKPOINT.1);
    host
}

/// The beats a run makes on its way to a link, beside the one each event gets.
///
/// Two: one before the connection and one before the handshake, which are the waits on
/// that path a run cannot take in pieces — so what it owes its supervisor there is a
/// beat on each side of them (ADR-0008). Counted rather than left out of the sums below,
/// because a third blocking call added there with no beat in front of it is exactly the
/// failure this catches: a run ended while it was reaching the other machine.
const BEATS_PER_LINK: usize = 2;

/// The pieces the wait between attempts is taken in.
///
/// One, because that wait is a beat long: the looking is what waits, and the gap before
/// the next question is not something to make a person sit out. Counted rather than
/// assumed, so a longer gap put back here shows up as the delay it is.
const RETRY_PIECES: usize = 1;

/// The same machine, with the keyboard already sent over.
///
/// Where the questions about coming back start, and about what crosses while it is over
/// there. By the chord rather than by an ask from outside the keyboard, because that is
/// this file's subject — the counts below include its four key events.
fn sending() -> SimSource {
    let mut sent_over = forwarding();
    chord(&mut sent_over, SWITCH_TO_THE_SINK);
    sent_over
}

fn run(host: &mut SimSource) {
    source::run(&Request::Relaying, false, host, None);
}

/// Everything that reached the link, without the timestamps.
fn sent(host: &SimSource) -> Vec<Message> {
    host.sent().iter().map(|sent| sent.message).collect()
}

fn down(key: Key) -> Message {
    Message::KeyDown {
        device: KEYBOARD,
        key,
    }
}

fn up(key: Key) -> Message {
    Message::KeyUp {
        device: KEYBOARD,
        key,
    }
}

/// The other way of asking: the tray item, or a terminal, which reach the run without a
/// keystroke (`docs/platform/windows/tray-item-as-its-own-program.md`).
fn asked(host: &mut SimSource, driving: Driving) {
    host.asked_for(driving);
}

/// Hold option, press this, let option go — the chord as a person makes it.
fn chord(host: &mut SimSource, key: Key) {
    host.press(KEYBOARD, Key::LeftOption);
    host.press(KEYBOARD, key);
    host.release(KEYBOARD, key);
    host.release(KEYBOARD, Key::LeftOption);
}

#[test]
fn the_chord_brings_the_keyboard_back_to_this_machine() {
    // The whole point: a person driving the Mac has to be able to get their own machine
    // back without reaching for the mouse or killing anything.
    //
    // Read off what crossed rather than off what was refused, because the refusals
    // cannot tell the two apart: a run releases everything as it stops, so a chord that
    // was ignored leaves the same last state as one that was obeyed. What only happens
    // when it is obeyed is that the keystrokes after it stay here.
    let mut host = sending();
    host.tap(KEYBOARD, Key::J);
    chord(&mut host, SWITCH_BACK);
    host.tap(KEYBOARD, Key::K);
    host.tap(KEYBOARD, Key::L);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        crossed.contains(&down(Key::J)),
        "the key typed before the chord should have crossed: {crossed:?}"
    );
    assert!(
        !crossed.contains(&down(Key::K)) && !crossed.contains(&down(Key::L)),
        "keys typed after the chord reached the Mac: {crossed:?}"
    );
}

#[test]
fn the_chord_itself_is_not_relayed() {
    // It moves the keyboard rather than typing, at both ends of the journey: an `s`
    // arriving on the Mac would be a character nobody asked for.
    let mut host = sending();
    chord(&mut host, SWITCH_BACK);
    chord(&mut host, SWITCH_TO_THE_SINK);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        !crossed.contains(&down(SWITCH_BACK)),
        "the chord's key crossed the link: {crossed:?}"
    );
    assert!(
        !crossed.contains(&down(SWITCH_TO_THE_SINK)),
        "the chord's key crossed the link: {crossed:?}"
    );
}

#[test]
fn the_chord_naming_where_the_keyboard_already_is_types_nothing_over_there() {
    // A person who has lost track presses the chord that sends the keyboard over while
    // it is already over. Idempotent means nothing happens (ADR-0013), and on the Mac
    // option-`n` is a dead key, so "nothing" has to hold for the keystroke as well as
    // for the trip: neither half of the `n` crosses, and the keyboard is not announced
    // a second time.
    let mut host = sending();
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        !crossed.contains(&down(SWITCH_TO_THE_SINK)),
        "the chord's key crossed the link: {crossed:?}"
    );
    assert!(
        !crossed.contains(&up(SWITCH_TO_THE_SINK)),
        "the chord's release crossed the link: {crossed:?}"
    );
    assert_eq!(
        crossed
            .iter()
            .filter(|message| matches!(message, Message::DeviceAttached(_)))
            .count(),
        1,
        "the keyboard made a second trip over: {crossed:?}"
    );
    assert!(crossed.contains(&down(Key::K)), "{crossed:?}");
}

#[test]
fn option_held_through_the_chord_is_still_option_on_the_other_side() {
    // Hold option, press `n`, and with option still down press `a`: what the person
    // means is option-`a` on the Mac. The option press happened while the keyboard was
    // this machine's, so the sink was never told about it — the source has to say so
    // as the keyboard goes over, and before the `a`. Its release then crosses like any
    // other, so the sink is not left holding option after the finger has lifted.
    let mut host = forwarding();
    host.press(KEYBOARD, Key::LeftOption);
    host.press(KEYBOARD, SWITCH_TO_THE_SINK);
    host.release(KEYBOARD, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::A);
    host.release(KEYBOARD, Key::LeftOption);

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(Key::LeftOption),
            down(Key::A),
            up(Key::A),
            up(Key::LeftOption),
        ]
    );
}

#[test]
fn coming_back_lets_go_of_every_key_the_sink_believes_is_held() {
    // The stuck-modifier failure, from the one direction that can cause it and that
    // the session ending does not cover (ADR-0002): the chord is made with a modifier
    // that already went across, and a sink never told it came up holds it down for
    // ever — chording every later keystroke on the Mac's own keyboard with it.
    //
    // Released last-pressed-first, so a key is never let go of while the modifier it
    // was pressed under is still held there.
    let mut host = sending();
    host.press(KEYBOARD, Key::LeftOption);
    host.press(KEYBOARD, Key::J);
    host.press(KEYBOARD, SWITCH_BACK);

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(Key::LeftOption),
            down(Key::J),
            up(Key::J),
            up(Key::LeftOption),
        ]
    );
}

#[test]
fn coming_back_lets_go_of_a_button_too() {
    // A sink has nothing to say about a button of its own accord, so a pointer whose
    // keyboard left while it held one would hold it for ever — the same failure in the
    // pointer's vocabulary.
    let held = PointerReport {
        buttons: Buttons::NONE.with(1),
        ..PointerReport::default()
    };
    let mut host = sending();
    host.pointer(KEYBOARD, held);
    host.press(KEYBOARD, Key::LeftOption);
    host.press(KEYBOARD, SWITCH_BACK);

    run(&mut host);

    assert_eq!(
        sent(&host).last(),
        Some(&Message::Pointer {
            device: KEYBOARD,
            report: PointerReport::default()
        }),
        "the button was left down on the Mac: {:?}",
        sent(&host)
    );
}

#[test]
fn the_other_chord_sends_the_keyboard_over_again() {
    // And the keyboard is announced once, not once per trip: the session it was
    // announced over is the same session, so the sink's list of what is attached is
    // still the list it was told (`the_link_outlives_a_trip_of_the_keyboard`).
    let mut host = sending();
    chord(&mut host, SWITCH_BACK);
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        crossed.contains(&down(Key::K)),
        "nothing crossed after the keyboard was sent back over: {crossed:?}"
    );
    assert_eq!(
        crossed
            .iter()
            .filter(|message| matches!(message, Message::DeviceAttached(_)))
            .count(),
        1,
        "the keyboard was announced again over a link that never went: {crossed:?}"
    );
    // Twice taken, once per trip over — and never taken twice without being given back
    // in between, which is what makes each trip a trip rather than a state that stuck.
    // The link outliving the trip does not mean the keyboards do: what is refused here
    // is what the person is typing on, and it comes back the moment they ask for it.
    assert_eq!(
        host.suppressions(),
        2,
        "the keyboards should be taken once per time the keyboard is sent over: {:?}",
        host.refusals()
    );
    assert!(
        !host
            .refusals()
            .windows(2)
            .any(|pair| pair == [Suppressing::Everything, Suppressing::Everything]),
        "{:?}",
        host.refusals()
    );
}

#[test]
fn the_switch_keys_are_ordinary_keys_without_the_chord() {
    // `n` and `s` are letters. A run that read them as a switch whenever they were
    // typed would move the keyboard mid-word.
    let mut host = sending();
    host.tap(KEYBOARD, SWITCH_BACK);
    host.tap(KEYBOARD, SWITCH_TO_THE_SINK);

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(SWITCH_BACK),
            up(SWITCH_BACK),
            down(SWITCH_TO_THE_SINK),
            up(SWITCH_TO_THE_SINK),
        ]
    );
    assert_eq!(
        host.refusals(),
        [
            // The keyboard is this machine's as the run comes up, and the chord that
            // sent it over is what took it from there (ADR-0013).
            Suppressing::TheSwitch,
            Suppressing::Nothing,
            Suppressing::Everything,
            Suppressing::Nothing
        ],
        "the keyboard was moved by a letter nobody chorded"
    );
}

#[test]
fn an_option_key_is_still_a_modifier() {
    // Holding it is not asking for anything: what the chord is made of has to keep
    // working as itself, or the layout loses a modifier.
    let mut host = sending();
    host.tap(KEYBOARD, Key::LeftOption);
    host.tap(KEYBOARD, Key::RightOption);

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(Key::LeftOption),
            up(Key::LeftOption),
            down(Key::RightOption),
            up(Key::RightOption),
        ]
    );
}

#[test]
fn either_option_key_makes_the_chord() {
    // A person uses whichever thumb is free, and a chord that only one of them made
    // would be one they had to think about.
    let mut host = sending();
    host.press(KEYBOARD, Key::RightOption);
    host.press(KEYBOARD, SWITCH_BACK);

    run(&mut host);

    // Read off the wire for the reason the headline test is: what only happens when the
    // chord is obeyed is that the key it was made with is let go of over there, and the
    // chord's own key never crosses.
    assert_eq!(
        sent(&host).last(),
        Some(&up(Key::RightOption)),
        "the chord made with the right-hand key did nothing: {:?}",
        sent(&host)
    );
}

#[test]
fn what_is_refused_while_the_keyboard_is_this_machines_is_the_chord_alone() {
    // Refused, so pressing it moves the keyboard rather than also reaching whatever has
    // the foreground. Refusing it costs nothing, because what refuses a key is also what
    // reports it: the chord is already on its way here by the time it is turned down.
    //
    // Only it, because refusing anything more here is the keyboard taken away with
    // nothing to show for it — the outcome ADR-0008 rules out.
    let mut host = sending();
    chord(&mut host, SWITCH_BACK);
    host.tap(KEYBOARD, Key::K);
    host.tap(KEYBOARD, Key::L);

    run(&mut host);

    assert_eq!(host.suppressions(), 1, "{:?}", host.refusals());
    let refusals = host.refusals();
    assert!(
        refusals.contains(&Suppressing::TheSwitch),
        "nothing was refused while the keyboard was this machine's: {refusals:?}"
    );
    let last_taken = refusals
        .iter()
        .rposition(|what| *what == Suppressing::Everything)
        .expect("the keyboards were taken at all");
    assert!(
        !refusals[last_taken + 1..].contains(&Suppressing::Everything),
        "the keyboards were taken again without being asked for: {refusals:?}"
    );
}

#[test]
fn every_event_is_still_answered_with_one_heartbeat() {
    // The promise the watchdog rests on (ADR-0008), across the one place the loop
    // changes shape: an event handled while the keyboard is this machine's is still an
    // event the loop came back round on, and a watchdog told otherwise would end a run
    // that is working.
    let mut host = sending();
    host.tap(KEYBOARD, Key::J);
    chord(&mut host, SWITCH_BACK);
    host.tap(KEYBOARD, Key::K);
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::L);

    run(&mut host);

    // The attach, three taps, and three chords of four events each — the one that sent
    // the keyboard over as the run came up, and the two below — plus what the one link
    // cost on the way up. One link and not two, because it outlives the trip.
    assert_eq!(host.heartbeats().len(), 1 + 6 + 12 + BEATS_PER_LINK);
}

#[test]
fn the_link_outlives_a_trip_of_the_keyboard() {
    // What makes the trip back immediate: the session is not what the keyboard's being
    // here ends. Rebuilt per trip, going over again costs a question asked of the
    // network, a connection, a handshake and every keyboard announced a second time —
    // seconds of it, while the person waits at a keyboard that has stopped answering
    // the machine in front of them and has not started answering the other one.
    let mut host = sending();
    host.tap(KEYBOARD, Key::J);
    chord(&mut host, SWITCH_BACK);
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        crossed.contains(&down(Key::J)) && crossed.contains(&down(Key::K)),
        "both trips should have sent what was typed on them: {crossed:?}"
    );
    assert_eq!(
        host.connects(),
        1,
        "the link was rebuilt for the second trip"
    );
    assert_eq!(
        crossed
            .iter()
            .filter(|message| matches!(message, Message::DeviceAttached(_)))
            .count(),
        1,
        "the keyboard was announced again over a link that never went: {crossed:?}"
    );
}

#[test]
fn asking_for_the_keyboard_is_not_made_to_sit_out_a_pause() {
    // The pause exists so a run does not spin at a machine that is switched off, and
    // what it must not delay is the person who has just pressed the chord: they are
    // waiting at the keyboard, and a wait between attempts is not a wait they asked for.
    //
    // Driven from an attempt that already failed, since that is what the pause is set by:
    // the Mac is missing once, so the keyboard comes back and goes over again.
    let mut host = forwarding();
    host.sink_missing(1);
    chord(&mut host, SWITCH_TO_THE_SINK);
    chord(&mut host, SWITCH_BACK);
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        crossed.contains(&down(Key::K)),
        "the keyboard never went over: {crossed:?}"
    );
    // One pause for the one attempt that failed, and none for either ask: a second
    // would be the person sitting out a wait meant for a machine.
    assert_eq!(
        host.pauses(),
        RETRY_PIECES,
        "an ask waited out the pause between attempts"
    );
}

#[test]
fn nothing_crosses_until_the_keyboard_is_asked_for() {
    // A run comes up with the keyboard on the machine it is running on. Starting the
    // other way round is what an installed favjit would do at every logon: the keyboard
    // and the pointer would go to the Mac the moment a person logged in, without anybody
    // having asked for it that time (ADR-0013).
    let mut host = forwarding();
    host.tap(KEYBOARD, Key::K);
    host.pointer(
        KEYBOARD,
        PointerReport {
            dx: 3,
            dy: 4,
            ..PointerReport::default()
        },
    );

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![],
        "the keyboard was the Mac's before anybody asked"
    );
    // Not even announced, and no socket opened: announcing a device is telling the sink
    // about a keyboard whose keys are about to arrive.
    assert_eq!(host.connects(), 0, "a link was opened with nothing to send");
}

#[test]
fn the_chord_is_what_starts_the_sending() {
    // The other half of the one above: asked for, it goes over, and the keyboard is
    // announced then rather than at startup.
    let mut host = forwarding();
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(Key::K),
            up(Key::K),
        ]
    );
}

#[test]
fn a_keyboard_plugged_in_while_the_keyboard_is_here_is_announced_when_it_goes_over() {
    // The state a run comes up in is the one where nothing is relayed, so every device
    // this machine has is attached during it. A source that dropped those attaches could
    // not name the device afterwards, and a key from a device the sink has no record of
    // is passed through unconverted — the layout silently stopping at the machine the
    // input is coming from (ADR-0003).
    let mut host = forwarding();
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert_eq!(
        crossed.first(),
        Some(&Message::DeviceAttached(keyboard())),
        "the sink was sent keys from a keyboard it was never told about: {crossed:?}"
    );
}

#[test]
fn a_keyboard_unplugged_while_the_keyboard_is_here_is_not_announced_at_all() {
    // The other half: one that came and went while nothing was being relayed is one the
    // sink has no reason to hear about, and an attach followed by a detach for a device
    // it never had is two events no hardware made as far as it is concerned.
    let mut host = forwarding();
    host.detach(KEYBOARD);
    chord(&mut host, SWITCH_TO_THE_SINK);

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![],
        "the sink was told about a keyboard that had gone before anything was relayed"
    );
}

#[test]
fn the_keyboard_can_be_sent_over_without_being_typed_on() {
    // The tray item exists because the chord needs the keyboard to be working, and while
    // the keyboard is this machine's the pointer is the person's — nothing is refused but
    // the chord itself (`docs/platform/windows/tray-item-as-its-own-program.md`).
    let mut host = forwarding();
    asked(&mut host, Driving::TheSink);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        crossed.contains(&down(Key::K)),
        "the ask did not send the keyboard over: {crossed:?}"
    );
}

#[test]
fn the_keyboard_can_be_brought_back_without_being_typed_on() {
    // The half that matters more: a person driving the Mac whose chord is not getting
    // through has one thing left that is not killing the process.
    //
    // From the keyboard actually being over there, or this would pass on a run that
    // ignored the ask entirely — the state it asks for being the state it is in.
    let mut host = sending();
    host.tap(KEYBOARD, Key::J);
    asked(&mut host, Driving::ThisMachine);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert!(
        crossed.contains(&down(Key::J)),
        "nothing crossed before the ask, so there was nothing to bring back: {crossed:?}"
    );
    assert!(
        !crossed.contains(&down(Key::K)),
        "the keyboard was still the Mac's after the ask brought it back: {crossed:?}"
    );
}

#[test]
fn an_ask_naming_where_the_keyboard_already_is_changes_nothing() {
    // The item reads the state it shows rather than remembering one, but it reads it when
    // it draws and a person clicks later, so the item they click can name where the
    // keyboard already is. That has to be nothing rather than a trip — asked in the
    // direction where a trip would show, since a run that took it as "the other one"
    // would announce the keyboard twice and take it twice.
    let mut host = sending();
    asked(&mut host, Driving::TheSink);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    let crossed = sent(&host);
    assert_eq!(
        crossed
            .iter()
            .filter(|message| matches!(message, Message::DeviceAttached(_)))
            .count(),
        1,
        "the keyboard made a second trip over: {crossed:?}"
    );
    assert_eq!(host.suppressions(), 1, "{:?}", host.refusals());
}

#[test]
fn an_ask_is_not_something_the_sink_is_told_about() {
    // It is not input. Whatever the tables say, nothing arriving on this path can turn
    // into a keystroke over there — the reason a probe is a kind of its own.
    let mut host = forwarding();
    asked(&mut host, Driving::TheSink);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(Key::K),
            up(Key::K),
        ]
    );
}

#[test]
fn an_ask_is_answered_with_a_heartbeat_like_anything_else() {
    // Reaching the next heartbeat is what says the loop came back round, and an ask
    // arrives on the same stream as the keystrokes precisely so that it does (ADR-0008).
    let mut host = forwarding();
    asked(&mut host, Driving::ThisMachine);
    asked(&mut host, Driving::TheSink);
    host.tap(KEYBOARD, Key::K);

    run(&mut host);

    // The attach, the two asks, and the tap, plus what the one link cost on the way up.
    assert_eq!(host.heartbeats().len(), 1 + 2 + 2 + BEATS_PER_LINK);
}
