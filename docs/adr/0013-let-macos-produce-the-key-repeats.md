# ADR-0013: On macOS, let the OS produce the key repeats

- **Status**: Accepted
- **Date**: 2026-08-29

**macOS only**, like the two before it. It follows from
[ADR-0011](0011-macos-output-through-a-virtual-hid-device.md): output on this
platform is a device that holds key state, and that changes who repeats a held key.

## Context

[ADR-0010](0010-clock-on-the-event-with-a-timer.md) has a timer on the host surface,
for a reason measured on the capture side: the HID queue carries one value for a
press and nothing while the key is held, so no repeat arrives there to forward
([key-repeat.md](../platform/macos/key-repeat.md)). That is the capture side; the
output side is the rest of the path.

What the output side does was measured three ways, with `o` held for three seconds
on the virtual keyboard and the resulting key-downs counted off a listen-only tap,
each case preceded by two seconds of counting with nothing held — which came to
zero every time, so no case was reading the one before it:

| sent every 33 ms while the key was held | characters |
|---|---|
| nothing | 84 |
| the same report again | 85 |
| a release and a press | 82 |

**The OS repeats a key the device says is down, at the machine's own rate, and what
favjit sends while it is held makes no difference.** The third case sends ninety
keystrokes of its own and the total does not move, so favjit's repeats and the OS's
are not adding: favjit's are either absorbed or preventing the OS's, and at this
rate the two cannot be told apart.

What it takes to count these without the instrument changing the answer is recorded
with the finding, in [key-repeat.md](../platform/macos/key-repeat.md).

## Decision

**favjit asks for no repeats on macOS.** `sink::run` is given `None`, and the
machine's rates are read and logged rather than used.

The timer stays on the host surface. ADR-0010's reasoning about the clock is
untouched, and a recorded trace still replays `Timer` events that a run made.

## Consequences

A held key repeats at whatever rate System Settings says, without favjit reading
those sliders or following changes to them — which it never did anyway, since the
rates were read once at startup.

It repeats as the **converted** key, because the converted key is the one the device
is holding. Nothing has to remember which key a repeat belongs to, so there is
nothing there to get wrong.

An undecided tap-hold does not repeat, because nothing has been sent for it. Holding
the built-in space bar produces no stream of spaces, which is the behaviour that
makes it usable as a shift.

Nothing on this platform sets a timer, so `EventKind::Timer` is a surface with no
user here. It is kept rather than removed: a host that has to produce repeats itself
is exactly what the capture-side measurement above describes, and removing the
surface would be a separate decision from this one.

The rates are still logged, which is the one thing a person can check when the
repeat feels wrong.

## Alternatives considered

### Keep favjit's own repeats

They cost a timer, a wake-up every 33 ms while a key is held, and two reports per
repeat. **Not taken**: they were measured to change nothing about the rate, so the
cost buys nothing — and two sources of repeats is the kind of arrangement that
works until something changes the timing and then doubles.

### Keep them and stop the OS's

There is no mechanism for the second half. Nothing favjit sends while a key is held
was found to suppress the OS's repeat, which is what the third row above says.

### Read the rates and match them

Pointless in the same direction: matching a rate that is already applied produces
either nothing extra or twice as much, depending on which of the two readings of the
third row is true — and the point of not repeating is that it does not matter which.
