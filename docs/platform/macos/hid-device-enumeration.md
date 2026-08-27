# HID device enumeration

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-27,
from an unsigned `cargo`-built binary in a terminal holding Input Monitoring.

Karabiner-Elements 1.8.0 was running. The keyboard at vendor 1241 / product 1330,
named in `karabiner.json`, did not appear at all.

## An external Bluetooth keyboard carries everything a per-device rule wants

The Lenovo TrackPoint Keyboard II appeared in a later run of the same
enumeration, connected over Bluetooth:

```
VendorID     = 6127
ProductID    = 24801
Product      = TrackPoint Keyboard II
Manufacturer = Chicony
Transport    = Bluetooth Low Energy
LocationID   = 3703680045
```

The vendor and product IDs match the ones `karabiner.json` uses to scope rules to
this keyboard, so an external keyboard is identifiable here in a way the internal
one is not.

Contrast the internal keyboard above, which has no `VendorID` or `ProductID` at
all — the two are told apart by different properties, not by different values of
the same one.

### Its keyboard and its pointer are elements of one node

`IOHIDDeviceCopyMatchingElements` over that device, with no open, returns **2113
elements on a single node** whose primary usage is page 1 usage 6 (keyboard):

| page | elements |
|---|---|
| 0x01 generic desktop | 11, including X, Y and wheel |
| 0x07 keyboard | 239 |
| 0x09 button | 3 |
| 0x0c consumer | 1027 |
| 0x00, 0x06, 0x08 | 20, 1, 5 |

So the pointer is not a node that could be left alone while the keyboard is
seized — which is why a seize kills the TrackPoint — and equally the pointer's
elements are reachable from the same queue as the keyboard's. Relaying is
mandatory here and it is not blocked at the capture end.

Enumeration is only the precondition. Whether a queue on a seized device
*delivers* those values is a separate question, and the answer is in
[input-suppression.md](input-suppression.md).

## Opening the manager fails; enumeration works anyway

```
IOHIDManagerCreate(NULL, 0)                -> non-NULL
IOHIDManagerSetDeviceMatching(mgr, NULL)   -- NULL matching dictionary
IOHIDManagerOpen(mgr, 0)                   -> 0xe00002c5
IOHIDManagerCopyDevices(mgr)               -> a set of 22 devices
```

`IOHIDManagerOpen` returned the error `0xe00002c5`, and `IOHIDDeviceGetProperty`
still returned properties for every device in the copied set. Input *value*
callbacks fire without a successful open as well — 349 page-7 events in a run
where that same error came back
([hid-input-callbacks.md](hid-input-callbacks.md)).

## The internal keyboard exposes no vendor, product or built-in flag

Four entries share `Product = "Apple Internal Keyboard / Trackpad"`,
`Manufacturer = "Apple"`, `Transport = "FIFO"`, `LocationID = 170`, differing
only in usage page and usage. The keyboard one is
`PrimaryUsagePage = 1`, `PrimaryUsage = 6`:

```
Product          = Apple Internal Keyboard / Trackpad
Manufacturer     = Apple
Transport        = FIFO
LocationID       = 170
PrimaryUsagePage = 1
PrimaryUsage     = 6
```

`VendorID`, `ProductID` and `BuiltIn` were all absent (the property lookup
returned NULL). Of the properties probed — `VendorID`, `ProductID`, `Product`,
`Manufacturer`, `Transport`, `BuiltIn`, `LocationID`, `RegistryEntryID`,
`PrimaryUsagePage`, `PrimaryUsage`, `SerialNumber`, `AppleVendorSupported`,
`KeyboardLanguage`, `CountryCode`, `AlternateHandlerID` — **`BuiltIn` was absent
on every one of the 22 devices**, and `RegistryEntryID` was absent on every one
too.

So `is_built_in` cannot be read from a `BuiltIn` property here. What
distinguishes this keyboard in the data available is `Transport = "FIFO"` and its
`Product` string.

## Karabiner's virtual keyboard is indistinguishable from a real one by usage

Two entries, both `PrimaryUsagePage = 1` / `PrimaryUsage = 6`:

```
VendorID     = 1452
ProductID    = 591
Product      = Karabiner DriverKit VirtualHIDKeyboard 1.8.0
Manufacturer = pqrs.org
SerialNumber = pqrs.org:Karabiner-DriverKit-VirtualHIDKeyboard
```

It appeared **twice** in the set. A virtual pointing device
(vendor 5824 / product 10202) was present as well.

This matters for favjit's own injection: whatever favjit injects through will
show up in this enumeration too, and a host that captures every
usage-page-1/usage-6 device would capture its own output.

## Not established

- Everything about the two external keyboards in `karabiner.json`; neither was
  attached.
- Whether `Transport = "FIFO"` reliably identifies the internal keyboard across
  models, or is an artefact of this one.
