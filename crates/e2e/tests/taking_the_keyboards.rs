//! How the Mac's own keyboards are found, seized and read, one call at a time.
//!
//! IOKit hands over a registry entry, opening it produces a device, and what the
//! device says about itself is what a rule is matched against. Every step is a
//! call, the order is the API's rather than favjit's, and a step written where no
//! case reaches it is a step held by whoever last read it (ADR-0006).
//!
//! Driven with the keyboards coming through the capture loop, which is the
//! opposite trade from every other file here: elsewhere a script says what
//! arrives under the number it gave, because what is asserted is what a keystroke
//! turns into. Here the loop is what is asserted, so the numbers are its own.
//!
//! Taking a keyboard and reading it are things the run *asks* for once it has
//! been told there is one, so the two loops are turned alongside each other here
//! the way a real platform turns them — one turn at a time, with the run holding
//! the token, so that the order a case states is the order it gets.

use favjit_engine::sink::{self, Ending, InputConfig, Request, Settings};
use favjit_engine::Layout;
use favjit_host_sim::{CaptureCall, SimHost, SimKeyboard};

/// The registry's own names, which is what says two findings are one keyboard.
const ONE: u64 = 0x5a5a;
const ANOTHER: u64 = 0x6b6b;

/// A keyboard-page usage the layout names, so a value that goes all the way
/// through is one this suite can recognise at the other end.
const PAGE: u32 = 0x07;
const K: u32 = 0x0e;

fn run(mac: &mut SimHost) -> Ending {
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        // Ended by its stream running out rather than by a bound, because what a
        // case here asserts happens after the run has been told there is a
        // keyboard: a bound reached on the first look would end it before it had
        // asked for anything.
        Settings::default(),
        InputConfig::default(),
        mac,
        None,
    )
    .0
}

fn machine(keyboards: Vec<SimKeyboard>) -> SimHost {
    SimHost::new().whose_capture_loop_finds(keyboards)
}

#[test]
fn a_keyboard_is_found_in_the_order_the_api_takes() {
    // The order is the whole of it, and it is IOKit's rather than favjit's: the
    // port before the notification and the source before either, the finding
    // given back as soon as the device is out of it, and the removal watched for
    // before anything is done with the device — a device that went in between
    // would be one nothing would say had gone.
    let mut mac = machine(vec![SimKeyboard::named(ONE)]);

    run(&mut mac);

    // Up to the point the keyboard is one the run can be told about, since what
    // it asks for after that has a case of its own.
    let found: Vec<CaptureCall> = mac
        .capture_asked()
        .into_iter()
        .take_while(|call| !matches!(call, CaptureCall::Seized { .. }))
        .collect();
    assert_eq!(
        found,
        vec![
            CaptureCall::OpenedAPort,
            CaptureCall::AskedToBeToldAboutDevices,
            CaptureCall::PutThatPortOnThisLoop,
            CaptureCall::LookedAtWhatIsHere,
            CaptureCall::FoundOneHere { named: ONE },
            CaptureCall::Opened { named: ONE },
            CaptureCall::LetGoOfTheFinding { named: ONE },
            CaptureCall::WatchedForItsRemoval { named: ONE },
        ]
    );
}

#[test]
fn the_port_is_asked_to_tell_before_what_is_here_is_listed() {
    // The other order has a gap in it: a keyboard plugged in between the listing
    // and the asking is in neither, so it is never read and never converted. This
    // order's cost is a device found twice, and the second finding is recognised
    // by the registry's own name for it.
    let mut mac = machine(vec![SimKeyboard::named(ONE)]);

    run(&mut mac);

    let asked = mac.capture_asked();
    let told = asked
        .iter()
        .position(|call| *call == CaptureCall::AskedToBeToldAboutDevices);
    let listed = asked
        .iter()
        .position(|call| *call == CaptureCall::LookedAtWhatIsHere);
    assert!(
        told.is_some() && listed.is_some() && told < listed,
        "{asked:?}"
    );
}

#[test]
fn a_keyboard_that_arrives_after_the_run_started_is_found_too() {
    // The whole point of asking to be told: a keyboard plugged in while the run
    // is up has to be found, or it is one the person types on and nothing
    // converts.
    let mut mac = machine(vec![SimKeyboard::named(ANOTHER).arriving_later()]);

    run(&mut mac);

    let asked = mac.capture_asked();
    assert!(
        asked.contains(&CaptureCall::FoundOneArriving { named: ANOTHER }),
        "{asked:?}"
    );
    assert!(
        asked.contains(&CaptureCall::Opened { named: ANOTHER }),
        "and it is opened like any other: {asked:?}"
    );
}

#[test]
fn a_keyboard_the_machine_will_not_name_is_let_go_of_rather_than_opened() {
    // The name is what says two findings are one keyboard, so one with no name is
    // one this loop could read twice without knowing — and reading a keyboard
    // twice converts every keystroke twice.
    let mut mac = machine(vec![SimKeyboard::named(ONE).the_machine_will_not_name()]);

    run(&mut mac);

    let asked = mac.capture_asked();
    assert!(
        !asked.contains(&CaptureCall::WatchedForItsRemoval { named: ONE }),
        "nothing is done with a device that cannot be told from one already \
         being read: {asked:?}"
    );
    assert!(
        asked.contains(&CaptureCall::LetGoOfTheFinding { named: ONE }),
        "and the finding is given back rather than held: {asked:?}"
    );
}

#[test]
fn a_finding_that_will_not_open_is_given_back_and_the_rest_carry_on() {
    // Ending here would mean one keyboard IOKit will not open stops favjit
    // converting on every other, which is the opposite of what a person wants
    // from the one that still works.
    let mut mac = machine(vec![
        SimKeyboard::named(ONE).that_will_not_open(),
        SimKeyboard::named(ANOTHER),
    ]);

    run(&mut mac);

    let asked = mac.capture_asked();
    assert!(
        asked.contains(&CaptureCall::LetGoOfTheFinding { named: ONE }),
        "{asked:?}"
    );
    assert!(
        asked.contains(&CaptureCall::Opened { named: ANOTHER }),
        "and the other one is still opened: {asked:?}"
    );
}

#[test]
fn two_keyboards_are_each_found_under_their_own_name() {
    // The name is the registry's and the number is the run's, so two keyboards
    // are two findings whichever order they come in: a loop that numbered by the
    // place they were found in would give one of them the other's layers the next
    // time they came up in the other order.
    let mut mac = machine(vec![
        SimKeyboard::named(ONE),
        SimKeyboard::named(ANOTHER).built_in(),
    ]);

    run(&mut mac);

    let asked = mac.capture_asked();
    assert!(
        asked.contains(&CaptureCall::Opened { named: ONE })
            && asked.contains(&CaptureCall::Opened { named: ANOTHER }),
        "{asked:?}"
    );
}

#[test]
fn something_the_machine_describes_that_is_no_keyboard_is_still_opened_and_described() {
    // Every device found is described, including the ones a run declines: which of
    // them is a keyboard is a table `engine` holds, and a page nothing names yet
    // is where a key reporting somewhere unexpected would be found (ADR-0006) —
    // so the machine deciding would be the one decision no case could drive.
    let mut mac = machine(vec![SimKeyboard::named(ONE).not_a_keyboard()]);

    run(&mut mac);

    assert!(
        mac.capture_asked()
            .contains(&CaptureCall::Opened { named: ONE }),
        "{:?}",
        mac.capture_asked()
    );
}

#[test]
fn a_keyboard_is_started_delivering_in_the_order_the_api_takes() {
    // The elements before the queue, because a queue with nothing on it wakes for
    // nothing; then the callback, the schedule and the start, which is the order
    // the API takes — a queue started before it is scheduled delivers nothing, and
    // one scheduled with no callback registered wakes this loop for nobody.
    let mut mac = machine(vec![SimKeyboard::named(ONE)]);

    run(&mut mac);

    let asked: Vec<CaptureCall> = mac
        .capture_asked()
        .into_iter()
        .skip_while(|call| !matches!(call, CaptureCall::Seized { .. }))
        .collect();
    assert_eq!(
        asked,
        vec![
            CaptureCall::Seized {
                named: ONE,
                exclusive: true,
            },
            CaptureCall::MadeAQueueOver { named: ONE },
            CaptureCall::PutAnElementOn { named: ONE, at: 0 },
            CaptureCall::WatchedForItsValues { named: ONE },
            CaptureCall::StartedItDelivering { named: ONE },
        ]
    );
}

#[test]
fn a_keyboard_a_delivering_run_takes_is_taken_exclusively() {
    // Always exclusively for a run that delivers, because the physical keystroke
    // is delivered by the OS as well: one taken shared would type every key twice,
    // once unconverted from the keyboard and once converted from here.
    let mut mac = machine(vec![SimKeyboard::named(ONE)]);

    run(&mut mac);

    assert!(
        mac.capture_asked().contains(&CaptureCall::Seized {
            named: ONE,
            exclusive: true,
        }),
        "{:?}",
        mac.capture_asked()
    );
}

#[test]
fn a_keyboard_the_machine_will_not_seize_is_never_started_delivering() {
    // Something else holds it, or this process is not privileged to take it:
    // either way reading it without the seize would convert the keystroke *and*
    // leave it on the machine, which is every key typed twice.
    const HELD_ELSEWHERE: i32 = 0xE00002C5u32 as i32;
    let mut mac = machine(vec![
        SimKeyboard::named(ONE).that_will_not_be_seized(HELD_ELSEWHERE)
    ]);

    run(&mut mac);

    let asked = mac.capture_asked();
    assert!(
        asked.contains(&CaptureCall::Seized {
            named: ONE,
            exclusive: true,
        }),
        "the seize is attempted: {asked:?}"
    );
    assert!(
        !asked.contains(&CaptureCall::StartedItDelivering { named: ONE }),
        "and nothing is read after it was refused: {asked:?}"
    );
}

#[test]
fn what_a_keyboard_reports_goes_all_the_way_to_the_output_device() {
    // The whole path in one case: found through the loop, seized, put on a queue,
    // and the value off that queue converted against the layout and written out —
    // which is what says the sequence above produced a keyboard that reports.
    let mut mac = machine(vec![SimKeyboard::named(ONE).reporting(PAGE, K, 1)]);

    run(&mut mac);

    assert!(
        !mac.reports().is_empty(),
        "a value that went all the way through writes a report; the loop asked: \
         {:?}",
        mac.capture_asked()
    );
}

#[test]
fn a_keyboard_that_goes_away_is_given_back_after_it_was_started() {
    // What the run reads about a keyboard going is a keyboard that has already
    // been given back: the queue stopped and the device let go of first, because
    // letting go of a running queue leaves the run loop holding a source over
    // memory nothing owns.
    let mut mac = machine(vec![SimKeyboard::named(ONE).that_goes_away()]);

    run(&mut mac);

    let asked = mac.capture_asked();
    let started = asked
        .iter()
        .position(|call| *call == CaptureCall::StartedItDelivering { named: ONE });
    let back = asked
        .iter()
        .position(|call| *call == CaptureCall::GaveItBack { named: ONE });
    assert!(
        started.is_some() && back.is_some() && started < back,
        "{asked:?}"
    );
}

#[test]
fn a_keyboard_the_run_was_told_to_leave_alone_is_never_seized() {
    // `--skip-built-in` is what a person reaches for when favjit is converting
    // the wrong keyboard, so it has to stop the seize and not only the
    // conversion: a keyboard taken away and then left unconverted is one that has
    // stopped working.
    let mut mac = machine(vec![SimKeyboard::named(ONE).built_in()]);

    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        Settings::default(),
        InputConfig {
            skip_built_in: true,
            ..InputConfig::default()
        },
        &mut mac,
        None,
    );

    assert!(
        !mac.capture_asked()
            .iter()
            .any(|call| matches!(call, CaptureCall::Seized { named: ONE, .. })),
        "{:?}",
        mac.capture_asked()
    );
}

#[test]
fn a_machine_with_no_keyboards_is_looked_at_and_nothing_more() {
    // The three calls that have to happen before a device can be found happen
    // anyway, because a keyboard plugged in a moment later arrives through them:
    // a loop that skipped them on an empty machine would be one that never heard
    // about the keyboard somebody is about to plug in.
    let mut mac = machine(Vec::new());

    run(&mut mac);

    assert_eq!(
        mac.capture_asked(),
        vec![
            CaptureCall::OpenedAPort,
            CaptureCall::AskedToBeToldAboutDevices,
            CaptureCall::PutThatPortOnThisLoop,
            CaptureCall::LookedAtWhatIsHere,
        ]
    );
}
