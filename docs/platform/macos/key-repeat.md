# Key repeat

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-28.

## A held key produces one HID value, not a stream

Holding a key while favjit was capturing through `IOHIDQueue` (the registry path
in `hid-device-enumeration.md`) produced exactly one converted keystroke. The
queue carried one value for the press and one for the release, and nothing in
between for as long as the key was held.

So there is no repeat to forward. Whatever generates the repeats a person sees
when typing normally sits somewhere this path does not pass through, and favjit
has to produce them itself.

## The OS repeats a key held on a virtual HID keyboard, and any report resets that repeat

Three runs, and the first two are worth stating because the reading that looks
obvious from them is wrong.

**A letter held for about three seconds produced one character**, while favjit was
repeating on its own timer at the machine's rate — sending another key-down every
33 ms, each of them the same 67 bytes as the report before it.

**Sent as a release and a press instead, the same hold produced 55 characters.** A
250 ms delay and 55 repeats at 33.3 ms is 2.1 s, the length of the hold. So a
report identical to the one standing is not a keystroke, and a repeat has to change
the state to be one.

**Then a run wedged on purpose with a key down repeated it by itself**, on and on
until the watchdog killed it, with favjit sending nothing at all. So something
repeats a key the device says is down, which the first run cannot be read as
saying — and what that something is was measured rather than reasoned about.

Three seconds with `o` held on the virtual keyboard, counted off a listen-only tap,
differing only in what went out while it was held. Each preceded by two seconds
counted with nothing held at all, which came to zero every time — a key a probe
failed to release would still be repeating, and its repeats would be counted as the
next case's:

| sent every 33 ms while the key was held | characters |
|---|---|
| nothing | 84 |
| the same report again | 85 |
| a release and a press | 82 |

**The OS repeats a key the virtual keyboard holds down, at the machine's own rate,
and what is sent while it is held makes no difference to that.** Not an identical
report, and not even a release and a press — the third case sends ninety keystrokes
of its own and the total does not move, so the two are not adding.

That leaves favjit's own repeat with nothing to contribute on this path: the rate is
the machine's either way, and a converted key held down repeats as the *converted*
key, since that is the one the device is holding.

The one character of the first run is not reproduced by any of the three cases and
is **unexplained**. The likeliest reading is that the key was not held as long as
intended, and nothing rests on it: every measurement since, including the 55
characters, is 26–28 a second, which is what the rates below say to expect.

## How to count these, and two instruments that cannot

Count the key-downs on a listen-only event tap created once and disabled when the run
ends, and precede each case with a window holding nothing at all. That window has to
come to zero: anything else means the instrument is still counting an earlier case.

Two ways of counting do not work here, both for the same reason — each adds a
condition to the thing being measured:

- **The measuring process's own stdin.** Input reaches it only with Secure Keyboard
  Entry routing input there, and that is one of the conditions a repeat measurement
  is trying to hold still.
- **A tap created per case and left enabled.** Each one counts every event of every
  case run so far, so two cases that produced the same number report different ones,
  in a ratio that looks like a real effect.

## The rates live inside `HIDParameters`, not on the entry

`HIDInitialKeyRepeat` and `HIDKeyRepeat` are **not** properties of the
`IOHIDSystem` registry entry. Reading them from it directly comes back empty:

```
IOServiceGetMatchingServices(0, IOServiceMatching("IOHIDSystem"), &it)
IORegistryEntryCreateCFProperty(entry, CFSTR("HIDInitialKeyRepeat"), NULL, 0)
    -> NULL
```

`ioreg -c IOHIDSystem -d 1 -l` confirms it: the entry's own property list has no
key of that name anywhere in it.

They are inside a dictionary the entry does carry:

```
IORegistryEntryCreateCFProperty(entry, CFSTR("HIDParameters"), NULL, 0)
    -> CFDictionary
        "HIDInitialKeyRepeat" = 250000000
        "HIDKeyRepeat"        =  33333333
```

Nanoseconds: a 250 ms delay before the first repeat, then about 30 a second.
Read back through the same dictionary, favjit logged `key repeat: 250ms then
every 33.333333ms`, which is what `ioreg` shows for this machine with System
Settings' sliders where they are.

The same two names also appear inside each HID event service's
`HIDEventServiceProperties`, with the same values. Which of the two an
application is supposed to read has not been established here; `HIDParameters`
on `IOHIDSystem` is the one favjit uses, because it is one entry rather than one
per keyboard.
