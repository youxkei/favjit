# IOHIDManager input value callbacks

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-27,
from an unsigned `cargo`-built binary in a terminal holding Input Monitoring.

Karabiner-Elements' user agents had been stopped; its system-domain
`Karabiner-Core-Service`, its `VirtualHIDDevice-Daemon` and its DriverKit
extension were still running. Physical `j` produced `kVK_ANSI_J` (see below), so
no remapping was in effect during the runs described here.

Two keyboards were in play: the MacBook's internal one and a Lenovo TrackPoint
Keyboard II over Bluetooth.

## The device is available, and it comes with every event

```
IOHIDManagerCreate(NULL, 0)
IOHIDManagerSetDeviceMatching(mgr, NULL)        -- every device
IOHIDManagerOpen(mgr, 0)
IOHIDManagerRegisterInputValueCallback(mgr, cb, NULL)
IOHIDManagerScheduleWithRunLoop(mgr, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode)
```

`IOHIDManagerOpen` returned `0xe00002c5` in a run taken while Karabiner's
console user server was alive, and `0x00000000` in a later run after it had been
stopped. **The error is contention, not a permission the probe lacked** — and the
input value callbacks fired either way, 349 page-7 events in the failing run and
1228 in the succeeding one.

In the callback, the originating device is reachable:

```
IOHIDValueGetElement(value) -> element
IOHIDElementGetDevice(element) -> device
IOHIDElementGetUsagePage(element), IOHIDElementGetUsage(element)
IOHIDValueGetIntegerValue(value)      -- 1 on press, 0 on release
```

Every event carried a non-NULL device, and the two keyboards came back
distinctly:

```
Product=Apple Internal Keyboard / Trackpad  Transport=FIFO  LocationID=170
Product=TrackPoint Keyboard II  Transport=Bluetooth Low Energy
  VendorID=6127  ProductID=24801  LocationID=3703680045
```

**This settles the fact [ADR-0003](../../adr/0003-unified-conversion-pipeline.md)
makes per-device rules depend on: at this level the originating keyboard is
available, per event, for both keyboards.** The vendor and product IDs are the
ones `karabiner.json` scopes rules with.

Note what identifies each: the external keyboard by `VendorID`/`ProductID`, the
internal one by `Transport = "FIFO"` and its `Product` string, since it has no
vendor or product id at all. There is no single property that names both.

See [event-tap.md](event-tap.md) for the CGEvent level, which carries a keyboard
*type* but no device identity.

## The values are HID usages, not virtual key codes

Usages observed on page 7 from the internal keyboard: `1`, `4`, `7`, `9`, `12`,
`13`, `22`, `42`, `227`, and `4294967295`. Elements reporting usage `1` and
`4294967295` arrived alongside real key presses and are not keys; `4294967295`
also carried values that repeat a neighbouring usage number (e.g. `value=1801`).

Pairing each HID event with the `CGEventTap` event it produced gives a
correspondence between the two vocabularies, observed rather than assumed:

| HID usage | tap field 9 | `kVK_` name |
|---|---|---|
| 7 | 2 | `kVK_ANSI_D` |
| 9 | 3 | `kVK_ANSI_F` |
| 13 | 38 | `kVK_ANSI_J` |

So the whole table can be established by observation, one key at a time, rather
than recalled. The usage page also has room for keys that
[virtual-key-codes.md](virtual-key-codes.md) shows have no `kVK_` constant at
all.

## The JIS keyboard reports usages the US one never does

Across two 15s windows — the first typed mostly on the internal keyboard, the
second mostly on the TrackPoint — the page-7 usages seen were:

| | usages |
|---|---|
| window 1 | 4, 7, 9, 10, 11, 13, 14, 15, 22, 24, 42, 51, 227, 231 |
| window 2 | 4, 7, 9, 13, 14, 15, 22, 24, 42, 51, 56, 135, 137, 225, 229 |

`135` and `137` appeared only in the window typed on the JIS keyboard. Those are
in the range the JIS-only key positions occupy, and they are values a CGEvent has
no `kVK_` constant for (see [virtual-key-codes.md](virtual-key-codes.md)).

Usage pages other than 7 also arrived: `1` and `9`, from the TrackPoint's
pointing device.

## The PC-JIS thumb keys, measured one at a time

Each key was pressed alone in its own window, with nothing else typed, and the
usage read back off the stream. In prompt order:

| key | usage | name in `IOHIDUsageTables.h` |
|---|---|---|
| 変換 | `0x8A` (138) | `kHIDUsage_KeyboardInternational4` |
| 無変換 | `0x8B` (139) | `kHIDUsage_KeyboardInternational5` |
| カタカナひらがな | `0x88` (136) | `kHIDUsage_KeyboardInternational2` |

The SDK's names say nothing about which key each is, so the association rests on
observation: three windows eight seconds apart, whose output appeared in the
order the keys were called for.

変換 was then confirmed a second way, by holding it and pressing `a`, `s`, `d`
through the real conversion pipeline. The outputs were `Tab`, `Escape`,
`ReturnOrEnter`, in that order and repeating — the Henkan layer's meanings for
those three keys. `Tab` is what makes it conclusive: on a Dudrack keyboard the
physical Tab key is remapped to a command key, so nothing else can produce a
`Tab` output.

**Pick a key whose converted output no other key can produce.** Holding 変換 and
pressing `q` gives `Digit1`, which the physical `1` key also gives by passing
straight through, so that pair cannot tell a conversion from a pass-through. The
instruction also has to be unambiguous when spoken: 「キューのキー」 is Q and 9 alike
in Japanese.

**`Fn` produced nothing at all.** It was pressed in a fourth window and no
page-7 usage appeared, which agrees with there being no `kVK_` constant for it
either.

Where it does report is not in the SDK's usage tables, and is not measured here.
It is taken from Karabiner-Elements, which reads the key successfully on this
machine: its vendored `pqrs/hid/usage_page.hpp` puts Apple's top-case page at
`0x00FF` and `pqrs/hid/usage.hpp` puts `keyboard_fn` at `0x0003` on it. The same
page carries `reserved_mouse_data` at `0xC0`, which Karabiner's own list of
top-case usages leaves out along with `clamshell_latched`.

The built-in keyboard says as much itself. Its element list —
`IOHIDDeviceCopyMatchingElements` with a NULL match, 279 elements — contains
exactly one element outside page 7 that could be `Fn`:

```
page 0x00ff  usage 0x0003  type 1     (kIOHIDElementTypeInput_Misc)
```

The other non-page-7 elements are page `0x0c` consumer buttons, page `0x08` LED
output elements, a page `0xff00` pair, and one page `0xff01` feature element at
usage `0x0b`. **`reserved_mouse_data` (`0xC0`) is not among them**, so on this
keyboard admitting the whole top-case page would not in fact have brought pointer
data — the usage-at-a-time rule is the safer one for other devices, not a fix for
this one.

Karabiner adds every element of a device to its queue and filters by a
page-and-usage whitelist in the callback. favjit filters when adding elements
instead, which is the same whitelist a step earlier.

Confirmed end to end: with that one element queued, pressing `Fn` on the built-in
keyboard produced a `RightCommand` down and up per press, alternating cleanly, and
nothing else. **`Fn` reports on page `0x00FF` at usage `0x0003`, with values 1 and
0 like any key** — so the modifier it converts to is released rather than left
down.

Two things to watch for in a check of this kind, each of which reads as a key that
does not report:

- **A view that shows only the key-downs.** A key with no release is
  indistinguishable from one with it, which is the half that matters for a modifier.
- **A run watching only the built-in keyboard** while the typing goes to the
  external one. It sees nothing at all.

## A dormant virtual keyboard still enumerates as a keyboard

With Karabiner's user agents stopped but its DriverKit extension still loaded,
the devices announced were:

```
device 0  built_in=false  vendor=1452  product=591     <- Karabiner's virtual keyboard
device 1  built_in=true   vendor=None  product=None    <- the internal keyboard
device 2  built_in=false  vendor=6127  product=24801   <- TrackPoint Keyboard II
```

Two things fall out. `Transport = "FIFO"` did pick the internal keyboard out
correctly, which is the only signal available for it. And **another remapper's
virtual keyboard is indistinguishable from a real one by usage**, so anything
that captures every keyboard captures that too — which is what `karabiner.json`'s
own `devices` list with `ignore` exists to handle.

`IOHIDManagerOpen` returned `0x00000000` in this run.

## The built-in keyboard's function row reports page-7 function keys

Observed on 2026-09-01, from favjit itself: installed as the daemon, seizing the
internal keyboard, converting for real. The brightness and volume keys were pressed
and did nothing at all, and the run's report of usages the tables cannot name — the
list `--usages` prints, written to `/var/log/favjit.log` on the way out — held
exactly this for that device:

```
  device 0 page 0x0007 usage 0x44 (68)
  device 0 page 0x0007 usage 0x45 (69)
  device 0 page 0x0007 usage 0x3b (59)
  device 0 page 0x0007 usage 0x3a (58)
```

**Nothing arrived on the consumer page**, and nothing on Apple's, though the
keyboard's element list has consumer buttons on it
([hid-device-enumeration.md](hid-device-enumeration.md)). The four usages are
`kHIDUsage_Keyboard_F1`, `F2`, `F11` and `F12`, which are the positions of the two
brightness keys and the two volume keys on that row — the pairing of usage to key
is that physical order and not a measurement, since the report is a set and does
not say which press produced which.

So the icons printed on that row are not what the keyboard sends. **What sends
them is the OS**, reading the row of its own keyboard, and a process that has
seized the keyboard gets the function keys instead.

The consequence for anything holding this keyboard is in
[ADR-0019](../../adr/0019-convert-the-function-row-to-its-icons.md): the row has to
be converted, because a function key handed back through a virtual device is a
function key.

## Not established

- **Whether the OS applies that same reading to the virtual keyboard's `F1`.** It
  is not needed either way — the row is converted to the controls directly — and
  the only evidence gathered is Karabiner-Elements', which reproduces the whole
  behaviour in its own configuration rather than relying on the OS for any of it.
- Which physical key each of the four usages above is, beyond the row's order.

- Which specific key each of the other usages corresponds to. They are recorded
  as observed; only three were tied to a `kVK_` value by pairing with a tap.
- Whether the per-event device is stable across a disconnect and reconnect, and
  whether `LocationID` survives one.
- Whether this path sees a device that another process has already seized.
