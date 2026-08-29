# A low-level hook that refuses a key takes it away from raw input too

Observed on **Windows 11 25H2, build 10.0.26200.9168**, 2026-08-31, from unsigned
`cargo`-built binaries run from a terminal. A message-only window registered for
usages 1/6 and 1/2 with `RIDEV_INPUTSINK`, and a `WH_KEYBOARD_LL` hook on the same
thread.

This is what the source's suppression rested on, so it is measured rather than
assumed: capture from raw input, refusal from a low-level hook, on the assumption that
the two paths are independent. **For keyboards they are not.**

## Keyboards: what the hook refuses never arrives

A probe that refused exactly one chord — alt and `s`, make code `0x1F`, so the keyboard
kept working — over twenty seconds of ordinary typing:

```
hook calls                     : 127
hook refusals (alt+s)          : 7
raw input, keyboards           : 120
  of those, not a key          : 0
raw input for the refused chord: 5
raw input, mice                : 0
```

**120 is exactly 127 − 7.** Every event the hook passed on arrived as raw input, and
every event it refused did not. The five arrivals for make code `0x1F` are `s` typed
without alt, which the hook let through.

The hook runs first, and returning non-zero ends the event for everything downstream —
raw input included.

## Mice: what the hook refuses arrives anyway

From a real relaying run of favjit over the same session, with both hooks refusing
everything:

```
keys captured: 0
pointer reports captured: 567
messages sent to the sink: 567

refused by the hooks: 150 keys, 567 pointer events
  nothing arrived as raw input while keys were being refused, so those keystrokes
  reached neither machine
```

**567 refused and 567 captured**, against 150 keys refused and none captured. So the
asymmetry is real: a `WH_MOUSE_LL` refusal leaves raw input alone, and a
`WH_KEYBOARD_LL` refusal does not.

## What this rules out

Capturing keys from raw input while refusing them with a low-level hook cannot work on
this version. The two are alternatives, not layers: whichever the hook eats is the
part nothing can relay.

That is not a bug to work around at this level. A hook is documented as ending the
event, and raw input is downstream of it.

## `RIDEV_NOLEGACY` suppresses keys and leaves raw input arriving

The other way to stop keystrokes reaching applications, measured the same way. A
probe registered for usage 1/6 and ran for ten seconds while a person typed into the
terminal it was started from:

```
RIDEV_INPUTSINK | RIDEV_NOLEGACY : true

registration removed             : true
raw input, keyboards             : 60
  of those, a key going down     : 30
```

**Nothing that was typed appeared in the terminal**, and sixty raw inputs arrived over
the same window. So:

- The two flags are accepted **together**. A registration does not have to choose
  between hearing input while it has no foreground and stopping it reaching anybody.
- Suppression and capture are the same registration rather than two mechanisms that
  have to agree, which is what the hook and raw input were not.
- `RIDEV_REMOVE` gives it back, so the release is one call and not a flag some
  procedure has to keep reading.

**It is all or nothing for a usage.** The flag names a usage page and usage, not a key,
so "refuse this one chord and nothing else" is not something this mechanism can say.

## It does not stop what the system does with `alt`

From a relaying run of favjit with the keyboard registered `RIDEV_INPUTSINK |
RIDEV_NOLEGACY`: ordinary keys were captured, relayed, and did not appear on this
machine — and **every `alt` chord acted on this machine anyway**. The keys still arrive
as raw input, so they are relayed as well, which is one keystroke doing something at
both ends.

So what the flag stops is legacy messages, and `alt` is not delivered to a window as
one: menu activation and the window-switching chords are handled above that layer. A
mechanism that suppresses everything an application would see is not the same as one
that suppresses everything the system would act on.

## A hook does refuse the `alt` chords

From using favjit with the keys read from a `WH_KEYBOARD_LL` hook and refused there
while relaying: `alt` chords no longer act on this machine, and they cross the link
instead. So what the registration could not stop, the hook does — which is the other
half of why the keys are read from it rather than from raw input.

## Not established

- What a shell hotkey does — `Win`+`r`, `Ctrl`+`Alt`+`Del` — and whether it is reachable
  while input is being refused. `alt` above was measured because it was the one that
  broke; these were not.
- Whether `RIDEV_NOLEGACY` on usage 1/2 stops a mouse moving the cursor. The pointer
  is suppressed by a hook today, and that works, so nothing has needed to ask.
- Whether the mouse asymmetry above holds for a hook that refuses only some pointer
  events. The run that measured it refused all of them.
- Whether `RegisterRawInputDevices` can be called from a thread other than the one
  that owns the window it names, which is what deciding suppression outside the capture
  thread would need.
