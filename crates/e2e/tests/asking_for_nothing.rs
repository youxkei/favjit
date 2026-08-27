//! The runs that cannot be asked for.
//!
//! A run is described by what it was asked to be, and some of what could be written
//! down is not a mode anybody could want. Taking the keyboards while delivering
//! nowhere is the one that matters: every keystroke on this machine is swallowed and
//! reaches nothing, which is the single outcome ADR-0008 rules out — and it would be
//! *asked for* rather than failed into, so nothing downstream would notice and no
//! ending would report it.
//!
//! There is nothing here to run, because there is nothing to write down: this file
//! is the record of what [`Request`] refuses to express, and it fails to compile if
//! that stops being true.
//!
//! Suppressing is not among the tests, because it is not a field: a run that
//! delivers takes the keyboards exclusively and one that does not takes nothing, so
//! there is no combination to enumerate. That the loop honours it is behaviour, and
//! `starting_up.rs` is where the two modes' calls are read off.

use favjit_core::sink::Request;

/// Every request that can be made of a converting machine.
///
/// Written out to be counted: one flag inside one variant is two, plus the mode that
/// delivers nothing. Eight would be three independent flags — suppressing,
/// listening and delivering — and the five that are missing are the ones where the
/// first two are asked of a run that delivers nowhere, plus the one that delivers
/// without taking the keyboards.
const EVERY_REQUEST: [Request; 3] = [
    Request::DryRun,
    Request::Injecting { listen: false },
    Request::Injecting { listen: true },
];

#[test]
fn listening_is_only_askable_of_a_run_that_delivers() {
    // The one flag that is still a choice: input from the other machine let in by a
    // run that delivers nowhere is converted into nothing, and a socket opened by a
    // run that changes nothing outside itself is a change it was not asked to make.
    for asked in EVERY_REQUEST {
        let (delivers, listens) = match asked {
            Request::DryRun => (false, false),
            Request::Injecting { listen } => (true, listen),
        };
        assert!(delivers || !listens, "{asked:?} listens for nothing");
    }
}
