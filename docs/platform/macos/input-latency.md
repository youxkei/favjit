# Input latency

Measured on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-28,
from unsigned binaries in a terminal holding Input Monitoring. Karabiner-Elements
was quit for the runs that opened the real keyboards.

Two keyboards were in play: the MacBook's internal one and a Lenovo TrackPoint
Keyboard II over Bluetooth.

## The HID value's timestamp is on the mach absolute clock

`IOHIDValueGetTimeStamp` compared against `mach_absolute_time()` taken at the top
of the value-available callback gives differences of tens of microseconds. So the
two are the same clock, and the difference is a latency rather than an offset
between clocks.

## From the HID stamp to our callback: ~60 µs

One `IOHIDQueue` per keyboard, values drained in the value-available callback,
20 key events per run:

| run loop | min | p50 | p90 | max |
|---|---|---|---|---|
| turned by hand in 50 ms slices | 47 µs | 61 µs | 156 µs | 208 µs |
| `CFRunLoopRunInMode` for the whole window | 32 µs | 65 µs | 81 µs | 89 µs |

**Turning the run loop by hand in 50 ms slices costs nothing.** A value arriving
mid-slice wakes the loop from inside `CFRunLoopRunInMode` and the callback runs
then, rather than waiting for the slice to expire — which is what the capture
thread depends on, since it turns the loop that way in order to look at the
watchdog's probe descriptor between turns.

## Injection: ~16 µs to post, ~113 µs to come back out

`CGEventSourceCreate(kCGEventSourceStateHIDSystemState)` once, then
`CGEventCreateKeyboardEvent` / `CGEventSetFlags` / `CGEventPost(kCGHIDEventTap)`
per event — favjit's injection path exactly — with a listen-only tap at
`kCGHIDEventTap` watching for the event to reappear. 200 posts of `kVK_F16`, none
lost:

| | min | p50 | p90 | p99 | max |
|---|---|---|---|---|---|
| the `CGEventPost` call | 7 µs | 16 µs | 26 µs | | 5392 µs |
| post → seen at the tap | 66 µs | 113 µs | 1003 µs | 5436 µs | 9523 µs |

The tail is three orders of magnitude above the median, on an otherwise idle
machine.

## End to end, favjit's converted event arrives *before* the original

Measured with favjit injecting while the keyboards were left shared, so both events
reach the tap: the physical one (`pid 0`) and favjit's converted one (favjit's pid).
Ten pairs from five presses of left shift:

```
 1430.759  flagsChanged  keycode  56  pid 0        <- physical
 1446.943  flagsChanged  keycode  56  pid 99840    <- favjit, +16.2 ms
 1520.488  flagsChanged  keycode  56  pid 99840
 1521.403  flagsChanged  keycode  56  pid 0        <- favjit 0.9 ms earlier
 2930.469  flagsChanged  keycode  56  pid 99840
 2931.645  flagsChanged  keycode  56  pid 0        <- favjit 1.2 ms earlier
```

Every pair after the first has favjit's event **1.0–1.3 ms ahead of the physical
one**. The whole of favjit — queue callback, channel, conversion, post, and the
trip back out through the system — is faster than the remaining distance the
physical event still has to travel to the same tap point.

The first keystroke after startup is the exception, at +16 ms. One-off; every
subsequent press in the same run was ahead.

**So favjit adds no perceptible latency of its own.** A lag felt while it is
running is not accounted for by anything measured here.

## Seizing the keyboard does not slow the queue

The numbers above are from a keyboard opened without exclusivity, because nothing
else can open a seized one to measure it from outside. favjit therefore measures
these segments itself and reports them at exit, which is the only view into the
configuration that matters. Under `--dry-run false --skip-built-in`, 178 HID
values from the TrackPoint keyboard:

| segment | p50 | p90 | p99 | max |
|---|---|---|---|---|
| HID stamp → capture thread | 78 µs | 111 µs | 140 µs | 164 µs |
| capture → handed to `CGEventPost` | 13 µs | | | |
| the `CGEventPost` call | 30 µs | 45 µs | 313 µs | **43.5 ms** |

So a seize costs nothing on the way in — if anything the seized figures are
tighter than the unseized ones.

A third run, with both keyboards seized rather than only the external one, and
Japanese input in the mix — 392 HID values, 142 injections:

| segment | p50 | p90 | p99 | max |
|---|---|---|---|---|
| HID stamp → capture thread | 116 µs | 155 µs | 3.19 ms | 3.20 ms |
| capture → handed to `CGEventPost` | 16 µs | 20 µs | 71 µs | 1.34 ms |
| the `CGEventPost` call | 44 µs | 57 µs | 407 µs | **41.7 ms** |

Holding two keyboards costs something at the tail — a 3.2 ms worst case on the
way in, against 164 µs when only the external keyboard is held — and nothing at
the median.

**The last cell recurs**: one `CGEventPost` per run, 43.5 ms in one and 41.7 ms in
the other, against medians of 30 and 44 µs. The tap measurement above has the same
shape — the first keystroke after startup late by 16 ms and every later one early
— so favjit's report prints the first sample of each segment beside its
quantiles, which is what would show whether the outlier is the run's first call.

## A posted modifier's own flag decides whether the system's state moves

`CGEventSourceFlagsState` is what an application consults for Cmd-click and
shift-drag, rather than the flags on a key event. Posting `kVK_Shift` down and
reading it back:

| what was posted | state while held | shift in the state |
|---|---|---|
| shift down, `flags = 0` | `0x20000000` | clear |
| shift down, `flags = kCGEventFlagMaskShift` | `0x20020000` | **SET** |

Both released cleanly back to `0x20000000` on a key-up with `flags = 0`.
`0x20000000` is present at rest, so it is not a modifier bit.

So a modifier key event must carry its **own** flag, not only the other modifiers
held alongside it. Confirmed through favjit: with the own flag set, holding
physical Caps Lock — which Dudrack types as left control — moved the state to
`0x20040000`, control SET. The physical Caps Lock cannot produce that (it moves
alpha-shift, `0x00010000`), so the control bit can only have come from the
injected event.

Characters were never affected by this: a shifted character comes from the flag
stamped on the *following* key's event, which was always set.

## Not established

- Whether the ~42 ms `CGEventPost` outlier is the run's first call. It appeared
  once per run in both suppressing runs, and the samples that would decide it were
  not kept.
- Whether the suppressing configuration ever adds a lag these segments do not
  show. One was felt in it, and three later runs in the same configuration — the
  last with both keyboards seized — did not reproduce it.
