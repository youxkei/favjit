//! Keeping the other machine's keyboards apart from this machine's.
//!
//! Device ids are each machine's own numbering, and the rules read them: the
//! MacBook's built-in keyboard takes Dudrack's layers while an external one takes
//! the raw-JIS remaps (ADR-0003's one pipeline still asks *which* keyboard). Two
//! machines numbering from one would put a Windows keyboard on the built-in rules.

use favjit_core::pairing::Authorized;
use favjit_core::sink::Request;
use favjit_core::{
    sink, DeviceId, DeviceInfo, EventKind, Injected, Key, Layout, ModifierKeys as M,
};

/// A run that converts, since what these are about is which keyboard the rules see
/// rather than how the machine was brought up.
fn converting() -> Request {
    Request::Injecting { listen: true }
}
use favjit_host_sim::{SimHost, SimLink};

fn key(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

const PAIRED: u8 = 0xaa;

/// The id the Mac gives its own keyboard, and the one a source may also use.
const ONE: DeviceId = DeviceId(1);

fn paired_list() -> String {
    Authorized::added("", &key(PAIRED))
}

/// A whole run of the converter, with that script arriving over its link.
fn over_the_link(script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(paired_list());
    link.connect(key(PAIRED));
    script(&mut link);

    let mut mac = SimHost::new().with_link(link);
    // The Mac's own keyboard is here too, with the id a source might reuse: the
    // question is whether the sink can tell them apart at all.
    mac.attach(DeviceInfo::built_in(ONE));
    sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    mac
}

/// Every device id the link put into the stream, in order.
fn devices(mac: &SimHost) -> Vec<DeviceId> {
    mac.delivered()
        .iter()
        .filter_map(|kind| match kind {
            EventKind::DeviceAttached(info) => Some(info.id),
            EventKind::KeyDown { device, .. } | EventKind::KeyUp { device, .. } => Some(*device),
            EventKind::DeviceDetached(id) => Some(*id),
            _ => None,
        })
        .collect()
}

/// Every device the link announced, in order.
fn announced(mac: &SimHost) -> Vec<DeviceId> {
    mac.delivered()
        .iter()
        .filter_map(|kind| match kind {
            EventKind::DeviceAttached(info) => Some(info.id),
            _ => None,
        })
        .collect()
}

#[test]
fn a_remote_keyboard_numbered_one_is_not_the_built_in_one() {
    // The built-in space bar is a tap-hold in Dudrack and an external one is not.
    // A remote keyboard taken for the built-in one would turn its space into shift.
    let mac = over_the_link(|link| {
        link.attach(DeviceInfo::external(ONE, 1234, 5678))
            .press(ONE, Key::Spacebar)
            .release(ONE, Key::Spacebar)
            .hang_up();
    });

    assert_eq!(
        mac.injected(),
        vec![
            Injected::KeyDown {
                key: Key::Spacebar,
                modifiers: M::NONE
            },
            Injected::KeyUp {
                key: Key::Spacebar,
                modifiers: M::NONE
            },
        ]
    );
}

#[test]
fn the_ids_the_sink_sees_are_not_the_ones_the_source_sent() {
    // Whatever the mapping is, it has to be a mapping: two machines both numbering
    // from one cannot be told apart by a number that came from either.
    let mac = over_the_link(|link| {
        link.attach(DeviceInfo::external(ONE, 1234, 5678)).hang_up();
    });

    assert_eq!(announced(&mac).len(), 1);
    assert_ne!(announced(&mac)[0], ONE);
}

#[test]
fn one_source_device_keeps_one_id_for_the_whole_session() {
    // The sink's held-key bookkeeping is per device, so a key that went down under
    // one id and came up under another would stay down.
    let mac = over_the_link(|link| {
        link.attach(DeviceInfo::external(ONE, 1234, 5678))
            .press(ONE, Key::J)
            .release(ONE, Key::J)
            .hang_up();
    });

    let ids = devices(&mac);
    assert!(ids.len() >= 4);
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "one device, one id: {ids:?}"
    );
}

#[test]
fn two_source_devices_stay_two_devices() {
    // Mapping them onto one id would merge two keyboards into one, and the layers
    // are per keyboard.
    let mac = over_the_link(|link| {
        link.attach(DeviceInfo::external(DeviceId(1), 1, 2))
            .attach(DeviceInfo::external(DeviceId(2), 3, 4))
            .hang_up();
    });

    let announced = announced(&mac);
    assert_eq!(announced.len(), 2);
    assert_ne!(announced[0], announced[1]);
}
