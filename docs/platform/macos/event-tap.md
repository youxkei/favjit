# CGEventTap

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-27,
from an unsigned `cargo`-built binary in a terminal holding Input Monitoring
(see [input-permissions.md](input-permissions.md)).

Karabiner-Elements 1.8.0 was running throughout, and its DriverKit virtual
keyboard was present as a HID device.

## Creating a tap

`CGEventTapCreate` succeeded with `options = kCGEventTapOptionListenOnly (1)`,
`place = kCGHeadInsertEventTap (0)` and an event mask of
`(1<<10) | (1<<11) | (1<<12)` — key down, key up, flags changed — at all three
tap locations:

| `tap` argument | Result |
|---|---|
| `kCGHIDEventTap (0)` | non-NULL |
| `kCGSessionEventTap (1)` | non-NULL |
| `kCGAnnotatedSessionEventTap (2)` | non-NULL |

## Running the loop

`CFRunLoopRunInMode(kCFRunLoopCommonModes, …)` **fails**:

```
invalid mode 'kCFRunLoopCommonModes' provided to CFRunLoopRunSpecific
```

`kCFRunLoopCommonModes` is accepted by `CFRunLoopAddSource` and rejected by
`CFRunLoopRunInMode`. `kCFRunLoopDefaultMode` works for both.

The call returns as soon as it has processed activity, so one call does not hold
the loop open for its full timeout; a caller wanting to stay in the loop has to
call it repeatedly.

## What a tap callback receives

Every integer field `0..=255` was read via `CGEventGetIntegerValueField`, for a
synthetic F16 down/up posted by the probe itself with
`CGEventSourceCreate(1)` + `CGEventCreateKeyboardEvent` + `CGEventPost(0, …)`.
All three taps saw both events. The non-zero fields, for the HID and session
taps:

```
type=10 flags=0x0020800100  9=106 10=91 39=114716 40=1572 41=39036 43=501
                            44=20 45=1 50=248 53=3 55=10 58=<mach time> 59=545259776
```

- `9` is the key code (`kCGKeyboardEventKeycode`): 106 = `kVK_F16`.
- `10` is `kCGKeyboardEventKeyboardType`: 91.
- `41` is `kCGEventSourceUnixProcessID`: the probe's own pid, i.e. the posting
  process.
- `55` matches the event type (10 = key down, 11 = key up).

The annotated tap saw the same key code but different flags
(`0x0000800100` rather than `0x0020800100`), carried an extra non-zero field
`52`, and a different value in `59`.

## What a real hardware key event looks like

From the internal keyboard, with Karabiner's user agents stopped, on a
listen-only tap at `kCGHIDEventTap`:

```
type=10 flags=0x0000000100  9=3 10=91 39=9861479 40=51854 45=1 50=248 55=10
                            58=<mach time> 59=256 76=252 77=102 78=102 80=10
                            85=1355059 87=4294971366 101=4 169=<mach time>
```

More fields carry values than on a posted synthetic event: 76, 77, 78, 80, 85,
87, 101, 102 and 169 were absent from the synthetic one and present here. `77`
and `78` always held the same value and tracked which key was pressed. `39`,
`40`, `85` and `87` were constant across every event of the run.

`flags` was `0x0000000100` for a plain letter and `0x0000100108` for a
`flagsChanged` event (type 12) whose field 9 read 55.

## The system disables a tap whose callback is too slow

An event of `type = 4294967294` (`0xFFFFFFFE`) arrived twice during a 30s run,
carrying `41=43414` (this process's pid), `43=501`, `44=20`, `50=248`,
`101=16`, `102=63` and no key code.

The probe's callback read all 256 integer fields, took a mutex and wrote a line
to stdout per event, so it was slow on purpose by accident. The effect is
measurable: over the same window the tap delivered **78** key events while
`IOHIDManagerRegisterInputValueCallback` delivered **349**
(see [hid-input-callbacks.md](hid-input-callbacks.md)) — the tap was off for most
of the run.

**A slow callback therefore costs events silently unless this notification is
handled**, which matters for [ADR-0008](../../adr/0008-input-suppression-and-watchdog.md):
the OS turning a tap off is a safety net for a wedged process, and equally a way
for a merely-slow one to drop input.

`CGEventTapEnable(tap, true)`, called from inside the callback that received the
notification, does recover it. A later run whose callback read 23 fields instead
of 256 and did no I/O at all still received the notification **twice** in 30s,
re-enabled each time, and came away with 338 key events against the HID
callback's 1228 — proportionate, rather than the near-total loss of the heavy run.
So the notification arrives even for a light callback, and handling it is not
optional.

## The tap carries a keyboard type, not a device

Two keyboards were used across two 15s windows — the internal one and a
Bluetooth JIS keyboard. Of the 24 fields recorded, only `58` and `169` took
values in one window that never appeared in the other, and both are monotonic
mach timestamps that would differ between any two windows. **No field identified
the originating keyboard.**

Two fields did take exactly two distinct values, in both windows, while exactly
two keyboards were in play:

| field | values |
|---|---|
| `10` (`kCGKeyboardEventKeyboardType`) | 42, 91 |
| `87` | 4294971366, 4295243666 |

The flags split too — `0x100108` and `0x100110` in the first window,
`0x20102` and `0x20104` in the second — which tracks which modifier keys were
pressed rather than which keyboard pressed them.

A keyboard *type* is a property of the hardware model, so at its very best it
cannot separate two keyboards of the same model, which is what a per-device rule
needs. That is the reason capture belongs at the level in
[hid-input-callbacks.md](hid-input-callbacks.md), where the device itself arrives
with every event.

## A consuming tap suppresses, and it takes no privilege

A tap created with `kCGEventTapOptionDefault` at `kCGHIDEventTap`, masked to
key-down and key-up, returning `NULL` for one key code and posting a different one
in its place:

```
consuming tap up for 20s. Type s a few times: you should see o and no s.
ooooooooooooooooooooooooooooo
swallowed 29 s-presses, passed 57 other events, tap disabled 0 times
```

Twenty-nine physical `s` presses, twenty-nine `o` characters, **no `s` at all**.
So returning `NULL` really does stop the keystroke reaching applications, on the
Accessibility permission this terminal already holds and with no root.

What that changes, if suppression were done this way rather than by seizing:

- **A pointer on the same device is untouched.** A seize is per device; a tap is
  per event type, and the mask here carries no pointer events.
- **No root, so no LaunchDaemon.** Capture still wants the HID queue for the
  originating keyboard, but an ordinary non-exclusive open is enough for that.
- **The catastrophic failure mode goes away by construction.** A callback that
  stops answering gets its tap disabled by the system, which means physical
  keystrokes flow again — unconverted, which is the degradation ADR-0008 asks for,
  arriving from the platform rather than from a supervisor.

The cost is the mirror of that last point: a tap disabled mid-use leaks
unconverted keystrokes, and one disabled while a person types their password
leaks them into a password field. The 23-field callback above was disabled twice in
30 s; this one, which reads one field and posts one event, was not disabled once.

## Secure Keyboard Entry held elsewhere stops a consuming tap dead

The protection has to be held by a *different* process to answer this: a probe that
enables it itself would be both the protected application and the tap. So one
process called `EnableSecureEventInput` and slept, while the same consuming tap ran
in another, logging every event it received.

Two 15-second windows, the same instruction ("press `s` five times") in each:

| | events reaching the tap | swallowed |
|---|---|---|
| no protection | 15 | 5 |
| held by another process | **0** | 0 |

Not one event — not the `s` presses, not anything else. **A consuming tap receives
nothing while Secure Keyboard Entry is held, so it suppresses nothing**, and the
physical keystrokes go to the application unconverted.

For a layout converter that is specific and bad: a password typed by muscle memory
in the converted layout arrives as the raw one. Suppression by seize does not have
the problem, being below the layer this protects — an injected keystroke reaches a
protected field and the lock screen
([event-injection.md](event-injection.md)).

What appeared on screen across the two windows settles it, since the counters alone
cannot tell "blocked" from "nothing typed": `ooooosssss`. Five replacements while
the tap was receiving, then five raw `s` characters while the protection was held —
the presses happened, and they went straight past.

## Not established

- Which keyboard each value of field `10` and field `87` belongs to. Both
  keyboards produced events in both windows, so the two values could not be
  attributed to one or the other. Settling it needs a window confined to a single
  keyboard, or the tap and HID streams correlated event by event.
- What a tap sees while another remapper holds the keyboards.
- The meaning of fields 39, 40, 43, 44, 45, 50, 52, 53, 59, 76, 77, 78, 80, 85,
  87, 101, 102 and 169. They are reported here as observed values, not as
  anything with a documented name.
- The timeout budget a callback has before the tap is disabled.

## The private IOHID bridge is not reachable

`dlsym(RTLD_DEFAULT, …)` returned NULL for all of:

```
CGEventCopyIOHIDEvent  CGEventGetIOHIDEvent
IOHIDEventGetSenderID  IOHIDEventSystemClientCreate
```

So none of them can be called without locating and `dlopen`ing a private
framework first. Whether they exist elsewhere on the system was not checked.
