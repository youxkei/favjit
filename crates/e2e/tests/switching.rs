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

use favjit_core::link::Message;
use favjit_core::source::{self, Request, Suppressing, SWITCH_BACK, SWITCH_TO_THE_SINK};
use favjit_core::{Buttons, DeviceId, DeviceInfo, Key, PointerReport};
use favjit_host_sim::SimSource;

const KEYBOARD: DeviceId = DeviceId(1);

fn keyboard() -> DeviceInfo {
    DeviceInfo::external(KEYBOARD, 0x17ef, 0x60e1)
}

/// A machine with a keyboard on it, relaying — which is where every one of these
/// starts, because `--dry-run false` asks for the keyboard to be over there.
fn forwarding() -> SimSource {
    let mut host = SimSource::new();
    host.attach(keyboard());
    host
}

fn run(host: &mut SimSource) {
    source::run(&Request::Relaying, host);
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
    let mut host = forwarding();
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
    let mut host = forwarding();
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
fn coming_back_lets_go_of_every_key_the_sink_believes_is_held() {
    // The stuck-modifier failure, from the one direction that can cause it and that
    // the session ending does not cover (ADR-0002): the chord is made with a modifier
    // that already went across, and a sink never told it came up holds it down for
    // ever — chording every later keystroke on the Mac's own keyboard with it.
    //
    // Released last-pressed-first, so a key is never let go of while the modifier it
    // was pressed under is still held there.
    let mut host = forwarding();
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
    let mut host = forwarding();
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
    // And the sink is told what the keyboard is a second time, because a session it
    // has heard nothing from is one whose device list this end no longer knows the
    // state of.
    let mut host = forwarding();
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
        2,
        "the keyboard should be announced once per time it is sent over: {crossed:?}"
    );
    // Twice taken, once per trip over — and never taken twice without being given back
    // in between, which is what makes each trip a trip rather than a state that stuck.
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
    let mut host = forwarding();
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
    let mut host = forwarding();
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
    let mut host = SimSource::new();
    host.attach(keyboard());
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
    let mut host = forwarding();
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
    let mut host = forwarding();
    host.tap(KEYBOARD, Key::J);
    chord(&mut host, SWITCH_BACK);
    host.tap(KEYBOARD, Key::K);
    chord(&mut host, SWITCH_TO_THE_SINK);
    host.tap(KEYBOARD, Key::L);

    run(&mut host);

    // The attach, three taps, and two chords of four events each.
    assert_eq!(host.heartbeats().len(), 1 + 6 + 8);
}
