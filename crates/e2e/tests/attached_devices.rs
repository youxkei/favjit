//! The enumeration `favjit --devices` reads, which is what a rule is written
//! against.
//!
//! The count before the list and the room before each path, which is the shape
//! this kind of enumeration has: a machine says how much it needs before it will
//! fill any of it in. Driven here rather than left behind the calls, because the
//! order is a sequence and a sequence no case reaches is one held by whoever last
//! read it (ADR-0006).

use favjit_engine::source::what_is_attached;
use favjit_host_sim::{ListingCall, SimListing};

#[test]
fn every_device_the_machine_names_comes_back_under_its_path() {
    // Keyboard or pointer, because those are the two a rule can single out: the
    // path is what a person writes the rule against.
    let mut listing = SimListing::new()
        .with_a_keyboard_at(r"\\?\HID#VID_17EF&PID_60E1")
        .with_a_pointer_at(r"\\?\HID#VID_046D&PID_C52B");

    assert_eq!(
        what_is_attached(&mut listing),
        vec![
            (true, String::from(r"\\?\HID#VID_17EF&PID_60E1")),
            (false, String::from(r"\\?\HID#VID_046D&PID_C52B")),
        ]
    );
}

#[test]
fn the_count_is_asked_for_before_anything_is_looked_at() {
    // The order the platform's own enumeration takes: a look asked for before the
    // count would be a look at however much the last caller happened to want.
    let mut listing = SimListing::new().with_a_keyboard_at("one");

    what_is_attached(&mut listing);

    assert_eq!(
        listing.calls(),
        [
            ListingCall::AskedHowMany,
            ListingCall::Looked { how_many: 1 },
            ListingCall::AskedWhatIsAt { place: 0 },
            ListingCall::AskedHowLongThePathIs { place: 0 },
            ListingCall::AskedForThePath { place: 0, room: 3 },
        ]
    );
}

#[test]
fn a_device_the_run_has_no_name_for_is_passed_over_rather_than_ending_it() {
    // What is being asked for is the devices a rule could single out, so one that
    // is neither a keyboard nor a pointer is not one of them — and a listing that
    // stopped there would hide every device behind it.
    let mut listing = SimListing::new()
        .with_something_it_will_not_name()
        .with_a_keyboard_at("behind it");

    assert_eq!(
        what_is_attached(&mut listing),
        vec![(true, String::from("behind it"))]
    );
}

#[test]
fn a_path_that_will_not_fit_leaves_that_device_out_and_no_other() {
    // The room was asked for from the machine itself, so a path that will not go
    // in it is the machine disagreeing with itself: that device is unnameable
    // rather than the listing being wrong, and the rest still answer.
    let mut listing = SimListing::new()
        .with_a_keyboard_at("cannot fit")
        .with_a_pointer_at("fits")
        .whose_path_will_not_fit(0);

    assert_eq!(
        what_is_attached(&mut listing),
        vec![(false, String::from("fits"))]
    );
}

#[test]
fn a_machine_that_looks_at_fewer_than_it_said_is_read_no_further() {
    // The look answers how many it could, and that is what the run walks: reading
    // to the count instead would read places nothing was looked at in.
    let mut listing = SimListing::new()
        .with_a_keyboard_at("looked at")
        .with_a_pointer_at("not looked at")
        .that_looks_at(1);

    assert_eq!(
        what_is_attached(&mut listing),
        vec![(true, String::from("looked at"))]
    );
}

#[test]
fn a_machine_with_nothing_attached_answers_with_nothing() {
    let mut listing = SimListing::new();

    assert_eq!(what_is_attached(&mut listing), Vec::new());
    assert_eq!(
        listing.calls(),
        [
            ListingCall::AskedHowMany,
            ListingCall::Looked { how_many: 0 }
        ],
        "and nothing is asked about a place, since there is none"
    );
}
