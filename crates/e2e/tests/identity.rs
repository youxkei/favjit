//! What this machine presents as itself, and when it makes it.
//!
//! ADR-0004 rests on the peer pinning one key, so which key a machine presents on
//! its second run is the whole of whether pairing holds: a machine that made a fresh
//! one every run would be refused by a peer that pinned the first. A run that listens
//! establishes it before it opens anything, and the machine underneath is simulated
//! down to the file it is kept in.

use favjit_engine::pairing::{Identity, NoIdentity};
use favjit_engine::sink::{self, Ending, InputConfig, Request};
use favjit_engine::Layout;
use favjit_host_sim::{derived_from, Did, IdentityCall, SimHost};

fn private_key(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

/// The identity a machine whose entropy hands over `private_key(fill)` would make.
///
/// Derived for real ([`favjit_noise::keypair`]) rather than chosen alongside a
/// public half of its own: the public half is arithmetic over the private one,
/// not a second free choice.
fn an_identity() -> Identity {
    derived_from(&private_key(1))
}

/// A run that listens, since that is the run an identity is for.
fn listening() -> Request {
    Request::Injecting { listen: true }
}

fn run(mac: &mut SimHost) -> Ending {
    sink::run(
        &listening(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        None,
    )
    .0
}

/// Whether the run said this is why nothing is listening.
///
/// Read off the log rather than a record of its own, because the reason reaching
/// the person is the whole of what it is for: a run that concluded it and said
/// nothing looks from the other machine exactly like one that is refusing. Matched
/// on [`NoIdentity`]'s own words so the sentence carrying them stays free to change.
/// Matched on the words a run reads off the reason rather than on the reason
/// itself, because one of them carries what the machine said about its own
/// filesystem with it: a test asserting the whole sentence would be asserting
/// the simulator's own wording back at itself.
fn warned_of(why: &str, mac: &SimHost) -> bool {
    mac.warnings().iter().any(|said| said.contains(why))
}

#[test]
fn a_machine_with_no_file_makes_an_identity_and_listens_with_it() {
    // Made on first use rather than by an installer: a machine nobody has paired
    // has nothing to protect yet.
    let mut mac = SimHost::new().that_can_make(&private_key(1));

    run(&mut mac);

    assert_eq!(mac.kept(), Some(an_identity().to_bytes()));
    assert_eq!(mac.listened_with(), Some(an_identity()));
}

#[test]
fn a_new_identity_is_made_opened_and_written_in_order() {
    let mut mac = SimHost::new().that_can_make(&private_key(1));

    run(&mut mac);

    assert_eq!(
        mac.identity_calls(),
        [
            IdentityCall::Read,
            IdentityCall::MadeDirectory,
            IdentityCall::Opened,
            IdentityCall::Wrote,
        ]
    );
}

#[test]
fn an_identity_failure_stops_before_the_next_file_operation() {
    let operations = [
        IdentityCall::MadeDirectory,
        IdentityCall::Opened,
        IdentityCall::Wrote,
    ];

    for (failed, expected) in
        operations
            .iter()
            .zip([&operations[..1], &operations[..2], &operations[..3]])
    {
        let mut mac = SimHost::new()
            .that_can_make(&private_key(1))
            .whose_identity_fails_at(*failed);

        run(&mut mac);

        assert_eq!(&mac.identity_calls()[1..], expected, "failed at {failed:?}");
        assert!(!mac.did().contains(&Did::BoundLink));
    }
}

#[test]
fn the_second_run_listens_with_what_the_first_one_kept() {
    // The one that matters: a machine that made a fresh identity every run would
    // present a key no peer pinned, and look configured while being refused.
    let mut first = SimHost::new().that_can_make(&private_key(1));
    run(&mut first);
    let kept = first.kept().expect("the first run kept one");

    let mut again = SimHost::new()
        .with_identity_file(&kept)
        .that_can_make(&private_key(9));
    run(&mut again);

    assert_eq!(again.listened_with(), Some(an_identity()));
    assert_eq!(again.kept(), None, "nothing is written over it");
}

#[test]
fn a_file_that_is_not_an_identity_is_left_alone_and_nothing_listens() {
    // Refused rather than replaced: what is in it is either somebody else's or a
    // write that did not finish, and overwriting it would throw away an identity a
    // peer may have pinned. Converting carries on — the Mac's own keyboards do not
    // depend on the link.
    let mut mac = SimHost::new()
        .with_identity_file(b"not a keypair")
        .that_can_make(&private_key(1));

    assert_eq!(run(&mut mac), Ending::Converted);
    assert_eq!(mac.kept(), None);
    assert_eq!(mac.identity_file(), Some(b"not a keypair".to_vec()));
    assert!(!mac.did().contains(&Did::BoundLink));
    assert!(warned_of(&NoIdentity::Foreign.to_string(), &mac));
}

#[test]
fn an_identity_that_cannot_be_kept_is_not_listened_with() {
    // Using it would present an identity the next run cannot present again, which
    // is a peer refusing a machine that looks paired.
    let mut mac = SimHost::new()
        .that_can_make(&private_key(1))
        .with_an_unwritable_identity_file();

    run(&mut mac);

    assert!(!mac.did().contains(&Did::BoundLink));
    assert!(warned_of("cannot keep the keypair that was made", &mac));
}

#[test]
fn a_machine_that_cannot_make_an_identity_converts_without_a_link() {
    // There is nothing to present, so nothing is opened — and the keyboards in
    // front of the person are converted regardless.
    let mut mac = SimHost::new().with_no_keypair();

    assert_eq!(run(&mut mac), Ending::Converted);
    assert!(!mac.did().contains(&Did::BoundLink));
    assert!(warned_of(&NoIdentity::CannotMake.to_string(), &mac));
    assert_eq!(mac.took_input(), Some(true));
}

#[test]
fn a_run_that_does_not_listen_leaves_no_identity_behind() {
    // An identity is a file written for a link: a run with no socket has nothing to
    // present it to, and writing one would be a change made by a run that was not
    // asked to make it.
    let mut mac = SimHost::new().that_can_make(&private_key(1));

    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );

    assert_eq!(mac.kept(), None);
    assert_eq!(mac.identity_file(), None);
    // Nothing is said either, because nothing was asked of the file: a run told not
    // to listen has no missing identity to report.
    assert_eq!(mac.warnings(), Vec::<String>::new());
}
