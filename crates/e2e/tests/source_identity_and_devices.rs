//! `favjit --identity` and `favjit --devices`, on the forwarding machine.
//!
//! Each is its own way into `engine::source` rather than a binary calling
//! `pairing` and `device` directly and holding the order between the calls
//! itself, where nothing could drive it (ADR-0006). So what a person reads
//! from either mode is answered here, against a scripted machine, rather than
//! only on real hardware.

use favjit_engine::source::{describe_devices, identify};
use favjit_host_sim::{identity, SimSource};

#[test]
fn a_machine_with_no_sink_pinned_is_shown_that_way() {
    let mut host = SimSource::new();

    let identified = identify(&mut host).expect("this machine can always make an identity");

    assert_eq!(identified.sink, None);
}

#[test]
fn a_pinned_sink_is_shown_by_the_digits_pairing_wrote_down() {
    let sink = identity(2);
    let mut host = SimSource::new();
    host.pinned_to(sink.clone());

    let identified = identify(&mut host).expect("this machine can always make an identity");

    assert_eq!(
        identified.sink.as_deref(),
        Some(sink.fingerprint().as_str())
    );
}

#[test]
fn every_attached_device_is_singled_out_by_its_own_vendor_and_product() {
    let attached = [
        (true, String::from(r"\\?\HID#VID_17EF&PID_60E1#foo")),
        (false, String::from(r"\\?\ACPI#PNP0303#bar")),
    ];

    let described = describe_devices(&attached);

    assert!(described[0].keyboard);
    assert_eq!(described[0].identity, Some((0x17ef, 0x60e1)));
    assert!(!described[1].keyboard);
    assert_eq!(
        described[1].identity, None,
        "no USB bus behind it, no rule to match"
    );
}
