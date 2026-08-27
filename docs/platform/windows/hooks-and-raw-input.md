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

## A release the hook refuses leaves the key down for this machine

Read off a favjit trace on 2026-09-10, on the same machine as above (the build was not
re-checked that day), and matching an earlier report of shift doing the same. The
source's session had gone while the keyboard was the Mac's, so the hook was refusing
nothing while the run looked for the sink again. Right control went down in that
window and the down carried on to Windows. The link came up while it was still held,
everything was refused from then on, and its release was refused with the rest:

```
Source  KeyDown  RightControl     6834.644 s   carried on, then relayed on the new link
Source  KeyDown  RightControl ×25 6835.1–6835.9 the repeats, refused
Source  KeyUp    RightControl     6836.291 s   refused
Source  KeyDown  RightControl     7218.740 s   the person pressing it to clear it
```

Between the two, everything typed on this machine after the keyboard came back acted
as though control were held, until the key was pressed and released again. So a key
whose down reached Windows stays down for Windows until an up reaches it; a refused
up does not count. What favjit does about it: while everything is refused, the
release of a key this machine was let see go down still carries on
([ADR-0013](../../adr/0013-a-chord-moves-the-keyboard.md)).

## A refused chord leaves its modifier reading as a lone tap, and an inert tap masks it

Reported from using favjit: alt and `s` did nothing where it was pressed, and letting
alt go moved the focus to the menu bar of whatever had the foreground — Brave's, in
the case seen. The hook refuses the chord's letter and does not refuse the modifier
(refusing a modifier's release leaves it down, above), so what this machine is shown
is the modifier pressed and released with nothing in between, which is its own ask for
the foreground window's menu.

**What masks it is a key tapped in between that the machine acts on in no way.**
Taken from `~/repo/katnas`, which drives this machine's windows and refuses chords on
it the same way: its keyboard hook injects `Ctrl` down and up beside every swallowed
alt or super chord, "so the swallowed key doesn't read as a lone modifier (menu /
Start)" (`crates/katnas-backend-windows/src/lib.rs`), and it uses the same tap again
to stage focus without opening a menu
(`winlogic.rs::alt_ctrl_unlock`). That is a working implementation on this hardware
rather than a measurement taken here, which is the standing this has.

The injected tap comes back through favjit's own hook, and is dropped there: the
procedures read `LLKHF_INJECTED` as nothing of ours, so the tap is neither relayed
nor masked again. katnas needs a sentinel in `dwExtraInfo` for the same purpose
because its hook passes injected events through to its own logic.

## Coming back mid-hold, the modifier is up for this machine

Reported from using favjit: with the keyboard the Mac's and alt held, alt and `s`
brought it back, and then — with Alacritty in the foreground — letting `s` go typed an
`s`, while alt and `a` pressed without letting alt go arrived as a bare `a`.

Both follow from the same fact as the refused release above, the other way round: the
alt press was refused along with everything else, so this machine never saw it, and
`GetAsyncKeyState(VK_MENU)` is what the procedures ask whether the chord's modifier is
down (`favjit_host::source::Answers::the_modifier_is_down`). With the answer `false`,
the chord's own letter is no longer recognised — its repeats and its release carry on
to the foreground — and the key after it arrives with no modifier.

What the run knows and the machine does not is which option keys are held: it follows
them off the stream it relayed (`engine::source::Chording`), because the sink has to be
told about them as the keyboard goes over. What favjit does about it: coming back, it
shows this machine a press of the modifier as well, the same key the procedures ask
about. No release of its own — the finger coming off reaches the machine as itself,
since nothing is refused once the keyboard is back.

**And the shown press is masked like any other.** A press whose only company is that
release is the lone tap of the section above, arriving one finger-lift later: what goes
out is `Alt↓ Ctrl↓ Ctrl↑`, which is katnas's `alt_ctrl_unlock` without the closing
`Alt↑` it appends only where no finger is holding one. Reported from using favjit with
the press shown unmasked: letting alt go moved the focus to Brave's menu bar, the same
way a refused chord does.

## Not established

- Whether a press sent with `SendInput` is what `GetAsyncKeyState` then answers with,
  and whether the foreground application agrees. katnas relies on an injected alt press
  re-enabling `SetForegroundWindow` (`winlogic.rs::alt_ctrl_unlock`), which is that
  mechanism working for one caller on this machine and not a measurement of this one.
- Whether the masking tap is needed for the chord alone. While everything is refused,
  a modifier held from before the link came up is shown pressed and released with
  every keystroke in between refused, which is the same lone tap — measured for
  neither, and a tap per refused key is not the answer either.
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
