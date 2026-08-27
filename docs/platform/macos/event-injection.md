# Injecting key events

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-27,
from an unsigned `cargo`-built binary in a terminal holding the access in
[input-permissions.md](input-permissions.md).

Events were posted the way favjit's host posts them:

```
CGEventSourceCreate(kCGEventSourceStateHIDSystemState)
CGEventCreateKeyboardEvent(source, keycode, down)
CGEventSetFlags(event, flags)
CGEventPost(kCGHIDEventTap, event)
```

## A posted event reaches every tap

A synthetic `kVK_F16` down and up was seen by listen-only taps at
`kCGHIDEventTap`, `kCGSessionEventTap` and `kCGAnnotatedSessionEventTap`, with
field 41 (`kCGEventSourceUnixProcessID`) carrying the posting process's pid. See
[event-tap.md](event-tap.md) for the fields.

## A system shortcut fires from flags alone

`Cmd+Space` was posted as **one space event carrying
`kCGEventFlagMaskCommand`, with no command key event before it** — nothing had
told the system a command key was down. **Spotlight opened.**

The binding was confirmed first: `com.apple.symbolichotkeys` hotkey 64 is
`enabled = 1` with parameters `(65535, 49, 1048576)` — key code 49 (`kVK_Space`)
and modifiers `0x100000` (command).

This settles the shape favjit relies on for its flag-only outputs: `Home` →
`Cmd+Left`, and the Henkan layer's comma and period. **The flag on the event is
enough; a separate modifier key event is not required for a shortcut to fire.**

## A posted modifier key needs its own flag to enter the system's state

Measured, with the numbers in
[input-latency.md](input-latency.md#a-posted-modifiers-own-flag-decides-whether-the-systems-state-moves):
a modifier key down posted with `flags = 0` leaves `CGEventSourceFlagsState`
unmoved, and the same event with its own flag set moves it. So a modifier event
must carry its own flag as well as the others held alongside it, or nothing that
reads the state — a `Cmd`-click, a shift-drag, hardware events arriving while the
remapped key is held — sees the modifier at all.

Characters were never affected either way: a shifted character comes from the
flag on the following key's event.

## The backslash position types a backslash

The Karabiner configuration this layout came from is pinned to an ANSI virtual
keyboard because on JIS that keyboard aliases the US `backslash` position onto
JIS `]`, putting `\` and therefore ctrl+`\` out of reach. favjit has no virtual
keyboard: it posts `kVK_ANSI_Backslash` and the current input source decides.

**No aliasing applies.** Posted into a terminal reading raw bytes, and asked of
the event itself with `CGEventKeyboardGetUnicodeString`:

| posted | arrived on stdin | the layout's own answer |
|---|---|---|
| `kVK_ANSI_Backslash` | `0x5c` `\` | U+005C `\` |
| shift + that position | `0x7c` `|` | U+005C `\` |
| `kVK_JIS_Yen` | nothing | nothing |

Two things follow. The position is `\` and `|` where it should be, so the reason
Karabiner needed an ANSI virtual keyboard does not apply to this path. And the
control character is not made by the layout — `CGEventKeyboardGetUnicodeString`
returns `\` whatever modifier is set, and shift only became `|` once an
application translated it.

`kVK_JIS_Yen` produces nothing at all here, which is consistent with an input
source that has no JIS yen position.

## The keyboard type an event carries, and the one the system reports

Two different things, and a chord can fall between them.

`LMGetKbdType()` on this machine answers `42` (JIS) while the TrackPoint keyboard
is what was last typed on, and `40`-stamped events posted by favjit do not move it:
76 posts later it still read `42`. **The value depends on which keyboard was used
last** — with the Mac's own keyboard in use it answers `46` instead, and `46`
translates like ANSI rather than like JIS
([virtual-hid-device.md](virtual-hid-device.md)). An event created from a
`kCGEventSourceStateHIDSystemState` source carries `91` unless a type is stamped on
it, which is neither of the layout types.

The type matters because the same key code is a different character under each.
`UCKeyTranslate` over the current ABC layout, key code `0x29`:

| modifiers | ANSI (40) | JIS (42) |
|---|---|---|
| none | `;` | `;` |
| shift | `:` | `+` |
| option | `…` | `…` |
| option + shift | `Ú` | `Ú` |

So `Ú` is simply what ABC's option layer puts on that key, under either type — the
character does not say which type was used.

**What this costs.** A window manager resolving a chord written as `:` looks the
character up against `LMGetKbdType()`: at ANSI it finds shift-`0x29`, at JIS it
finds `0x27` unshifted. favjit emits ANSI positions, so while `LMGetKbdType()`
answers JIS the two disagree about where `:` is, the chord is never matched, and the
keystroke falls through to be typed. Karabiner is free of it because its virtual
keyboard is a *device*: being the last one used, it is what `LMGetKbdType()`
answers with, and it declares itself ANSI.

**The character an application receives is translated under the event's stamped
type.** favjit converting physical `q` on the JIS keyboard — key code `0x29` with
shift, stamped ANSI — typed `:` while `LMGetKbdType()` answered `42`, where a JIS
translation of the same event would have given `+`.

So the stamp settles the characters and nothing else: everything that reads the
event is consistent with the layout tables, and everything that reads
`LMGetKbdType()` is not.

## A posted mouse event is positional, and nothing accelerates it

Measured by posting one `kCGEventMouseMoved` at a time and reading the cursor back:

| posted | cursor moved |
|---|---|
| `kCGMouseEventDeltaX = 40`, position unchanged | **0** |
| position `+40`, no delta | **40** |

So the delta fields are carried for whoever reads them and move nothing; the
absolute position is what the cursor follows, one point per point, with no
acceleration curve applied.

**This is what decides how a pointer can be relayed.** A seize is per device, and
the TrackPoint Keyboard II is one HID device — every registry node for it reports
`PrimaryUsagePage 1 / PrimaryUsage 6`, with the pointer on further collections of
the same device — so suppressing its keyboard takes its TrackPoint with it,
observed as a dead cursor while favjit held it. Relaying that pointer through
`CGEventPost` would mean favjit reproducing Apple's acceleration curve itself, from
a machine parameter (`HIDPointerAcceleration`, `45056` here) that is a coefficient
and not a curve.

Karabiner has the same problem and answers it with a virtual pointing device, which
the OS accelerates like any other mouse — and its own code refuses to grab a device
until that device is ready, for exactly this reason.

**favjit cannot present a device of its own through `IOHIDUserDevice`.**
`IOHIDUserDeviceCreate`, `IOHIDUserDeviceHandleReport` and their neighbours have no
SDK header but are exported by the shipped IOKit (`dyld_info -exports`), and the
mechanism is in use on this machine — the TrackPoint keyboard itself appears in the
registry as an `IOHIDUserDevice`, presented by the Bluetooth stack. Called with a
report descriptor lifted from a virtual keyboard that does work, it returns NULL
both as the user and **as root**:

```
uid 0
IOHIDUserDeviceCreate -> NULL (refused)
```

The framework logs the attempt as a ref that never bound to anything:

```
uservdev[36899] [com.apple.iohid:userdevice] 0x0: Destroy: <IOHIDUserDeviceRef ref:0/0 id:0x0 stats:0,0,0>
```

Not established: *why*. No entitlement failure is logged, and with no header there
is nothing to check the properties dictionary against, so a missing required key
cannot be ruled out — the function signatures here are declared from outside too.
What is established is that two obvious attempts fail, which is what the choice of
output has to be made on.

## How long any of it takes

In [input-latency.md](input-latency.md). Briefly: the `CGEventPost` call is 16 µs
at the median, the event reappears at a listen-only tap 113 µs after being
posted, and the tail of that reaches 9.5 ms.

## Secure Keyboard Entry does not stop a posted event

A probe that enabled the protection itself with `EnableSecureEventInput`, posted
`kVK_ANSI_D` and read its own stdin in raw mode:

```
secure keyboard entry already on elsewhere: no
posting with it off  -> arrived: YES
secure keyboard entry now on: yes
posting with it on   -> arrived: YES
secure keyboard entry off again: no
```

`IsSecureEventInputEnabled` reported the protection as on for the second post, and
the character still arrived. **A password field is not out of reach of this injection
path.**

## The lock screen takes them too

With favjit running `--dry-run false --skip-built-in` from a terminal in the
user's session, the screen locked with Ctrl+Cmd+Q, and typing on the seized
external keyboard: the password field accepted the input and the machine unlocked.

Two things follow. A posted event **does** cross into `loginwindow`'s field,
although the posting process belongs to the user's session and that field does not.
And what arrived was the converted stream rather than the raw keys, since a
password typed in Dudrack does not match when the layout is not applied — that part
is inference from the unlock succeeding, not a separate measurement.

`--skip-built-in` is what made the lock-screen check safe to run: only the external
keyboard was seized, so the Mac's own keyboard could type either way.

## Not established
- Whether a post reaches a field in an application that holds Secure Keyboard Entry
  — Safari, a `sudo` prompt. The run above had the process that enabled the
  protection receiving its own post, which is the same system-wide flag and not the
  same arrangement.
- Whether ctrl + the backslash position reaches an application. The control
  character is made by whatever reads the keystroke rather than by the layout, so
  the answer belongs to the receiver.
- Whether the OS ever repeats a posted key that is held down. Nothing here depends
  on it: output goes out as a device rather than as posted events, and a key that
  device says is down is repeated by the OS
  ([key-repeat.md](key-repeat.md)).
- Whether an application sees a flags-only symbol — `Digit2` plus the shift flag
  for `@` — the same way it sees the shift-down, `2`, shift-up that a virtual HID
  keyboard would send.
