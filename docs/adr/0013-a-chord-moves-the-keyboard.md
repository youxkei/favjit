# ADR-0013: A chord on the Windows keyboard moves it between the two machines, and the source decides

- **Status**: Accepted
- **Date**: 2026-08-31

## Context

[ADR-0002](0002-input-topology.md) leaves this open by name: "There is still a local question of whether Windows input is currently going to macOS or staying on Windows, but that is one machine's state, not a negotiation." Nothing answered it, so a relaying run took the Windows keyboard for as long as it ran and there was no way back but killing it.

One keyboard cannot drive both machines. What is relayed has to be refused where it was typed, or every keystroke lands on both screens — so this is a mode with two states rather than two things happening at once, and what is needed is a way for a person to move between them without a second input device.

Two constraints decide where that way can live:

- While the keyboard is driving the Windows machine, nothing is converting anything: the keystrokes are that machine's own. A chord named in the layout's vocabulary is one the source could not recognise, and the source reading a layout in order to recognise it would be a second place for the layout to live ([ADR-0003](0003-unified-conversion-pipeline.md)).
- While the keyboard is driving the Mac, the source sees every keystroke before it crosses. So it can recognise a chord in either state, and the Mac can recognise one in only one of them.

ADR-0002 also names the hazard: "a modifier held down at the moment control moves can be left stuck on the sink", with the sink responsible for releasing what it holds when forwarded input stops. A chord makes that moment routine rather than exceptional, and the chord is *made of* a modifier.

## Decision

Option and `n` sends the keyboard to the Mac; option and `s` brings it back. Both are recognised by `engine::source` on the make code the position arrives as, and neither crosses the link.

**A relaying run comes up with the keyboard on the machine it is running on**, and sends nothing until it is asked to. This is the state's starting value and not a second mode: `--dry-run` still decides whether a run will ever take the keyboards and open a link, and this decides only where the keyboard is once one will.

What settles it is that a run is started by a logon rather than by whoever typed the flag: the flag is typed once, and a keyboard that went to the Mac at every logon after that would be going somewhere nobody had asked for it to be that time. The person who did want it there is one chord away from saying so.

That is also the state a run reaches after the chord that brings the keyboard back, so startup is not a case of its own — there is one waiting state and both ways in lead to it. While it waits, the source keeps following what this machine has attached to it without sending anything: a device that arrived while the keyboard was here is one the sink has to be told about when it goes over, and a source that had dropped the attach would have no name for it ([ADR-0003](0003-unified-conversion-pipeline.md) — a key from a device the sink has no record of is passed through unconverted).

**An ask can also arrive from outside the keyboard.** The chord needs the keyboard to be working, and the case a person needs it in most is the one where it is not, so a run also takes an ask on the same event stream as the keystrokes: the tray item's, and a terminal's ([the source's tray item](../platform/windows/tray-item-as-its-own-program.md)). It names the machine it wants rather than saying "the other one", for the reason there are two chords rather than one.

While the keyboard is the Windows machine's, that chord alone is refused there — so pressing it moves the keyboard rather than also reaching whatever has the foreground. Nothing else is refused, and `Suppressing` names the three states so the middle one cannot be confused with the other two.

Refusing exactly one chord costs nothing where the thing that refuses a key is also the thing that reports it: the chord is already on its way to the loop by the time it is turned down ([docs/platform/windows/hooks-and-raw-input.md](../platform/windows/hooks-and-raw-input.md) is why it has to be that way round on Windows).

Coming back, the source sends a release for everything the sink believes is held — keys last-pressed-first, and a button-less report for any pointer holding one — before the keyboard becomes this machine's again. It does not wait for the sink to notice.

**The same hazard exists on the Windows side, and is answered the same way: by letting the release through.** A key whose press reached Windows is down for Windows until a release reaches it, and a refused release does not count ([docs/platform/windows/hooks-and-raw-input.md](../platform/windows/hooks-and-raw-input.md)). A key can be down at the moment everything starts being refused — the option the chord is made with always is, and any key is when the link comes up while it is held, since nothing is refused while there is no link. So while everything is refused, the one thing that still carries on is the release of a key this machine was let see go down. What the machine is believed to hold is exactly what it was shown: a refused press makes nothing held, and the release lets go of it once. The release is still reported and relayed, so the sink lets go of the same key.

**The link outlives the trip.** What comes back is the keyboard, not the session: rebuilt for each trip, going over again costs a question asked of the network, a connection, a handshake and every device announced a second time — seconds in which the keyboard has stopped answering the machine in front of the person and has not started answering the other one, which is the whole of what a chord is supposed to avoid. Kept, the chord is the whole of it, and the sink's list of what is attached is still the list it was told.

What that costs is one frame on the wire while the keyboard is here, because the sink ends a session it has heard nothing on. So the format has one message that is not input, and the source sends it whenever a wait comes back with nothing. Keeping the keyboards is *not* part of it: what is refused is what the person is typing on, and that comes back the moment they ask for it.

## Consequences

- A person can use both machines from one keyboard, which is what the desk looked like before favjit and what makes a forwarding run something to leave running.
- The state is one machine's, so no arbitration crosses the link and ADR-0002's one-way flow is untouched. The Mac is never told which machine the keyboard is driving; it sees input or it does not.
- The stuck modifier ADR-0002 warns about is answered at the source as well as at the sink. Belt and braces on purpose: the sink's release depends on it noticing the input stopped, and the source knows exactly when it stopped because it is the one that stopped it.
- Released last-pressed-first, because what the sink injects for a release is read against the layer it is holding: letting go of the modifier first would resolve the keys under it against the wrong layer.
- The chord's two keys are ordinary letters the rest of the time. They are only a chord with option down, so nothing moves mid-word.
- `n` and `s` chorded with option cannot be *sent* to the Mac. Two positions out of the layout, in exchange for the keyboard being movable at all.
- Refusing one chord is not "the keyboard stopped working": everything else arrives, so the middle state does not reach the outcome [ADR-0008](0008-input-suppression-and-watchdog.md) rules out.
- A link that dropped is the one case left that costs a rebuild, so it is looked for again at once. The pause between attempts is for a machine that is not answering, and a link that was carrying keystrokes a moment ago is the opposite of that — waiting it out is the delay a person sees, on the trip they did not get.
- Which machine the keyboard is driving is a state inside one mode rather than something a run can be asked for: `Request` still has two variants, and neither of the two ways a run could be assembled wrongly becomes expressible.
- The mechanism that refuses a key has to be the one that reports it, which on Windows means the keys are read from a hook and not from raw input — so a forwarded key carries no device, and per-device rules are not expressible for them ([ADR-0003](0003-unified-conversion-pipeline.md) allows for exactly that). `--ansi` was already a property of the machine rather than of a keyboard.
- The end-to-end suite drives the whole of it, because the chord is `engine`'s: which keys, what crosses, what is released, and what is refused in each state.

## Alternatives considered

### A chord named in the converted vocabulary, recognised by the sink

What a person driving the Mac is thinking in, and the sink is the end that sees it. **Not taken**: it only works in one of the two states — the Mac sees nothing while the keyboard is the Windows machine's, so the other direction needs a source-side chord anyway — and telling the source about it means a message from sink to source, which is a reverse direction on the link for one bit. Two mechanisms where one does.

### A chord derived from the layout, so that it *is* option and `s` after conversion

The source would ask the layout which position converts to that, and match on the position. **Not taken**: it hands the layout to the source, which is the one thing [ADR-0003](0003-unified-conversion-pipeline.md) keeps in one place, and it makes the chord move whenever a rule changes.

### Injecting a release on Windows for what it still holds

Send Windows a release for every key it was let see go down, at the moment everything starts being refused. **Not taken**: the procedures ignore what favjit injected, so it would be a second mechanism beside the one that already sees every event, and a modifier released under a person's finger is one Windows will hear go down again the moment the key repeats — which it does not, because the repeats are refused. Letting the one real release through does the same with nothing added.

### One chord that toggles

Fewer keys to remember. **Not taken**: a person who has lost track of which machine the keyboard is on presses it and cannot tell what happened until they type something. Two chords are each idempotent — the one that sends it over does nothing when it is already over.

### A key with no modifier

**Not taken**: it would move the keyboard mid-word, and there is no key on a keyboard that a person never types.

### Refusing nothing while the keyboard is the Windows machine's

Simpler: refuse everything or nothing, and let the chord type as well as move. **Not taken**: every switch would leave a menu open or a letter behind in whatever had the foreground, and the cost of avoiding it is two make codes compared in a procedure that was going to run anyway.

### Suppressing the keyboard with the raw input registration instead

`RIDEV_NOLEGACY` stops keys reaching applications while raw input keeps arriving, which would mean one mechanism rather than two and the device on every event. **Not taken**: it does not stop what the system does with `alt`, so every chord acted on the Windows machine *and* crossed the link — one keystroke doing something at both ends. Measured, not assumed: [docs/platform/windows/hooks-and-raw-input.md](../platform/windows/hooks-and-raw-input.md).

### Starting with the keyboard on the Mac

What `--dry-run false` reads as asking for: a person who typed it wants their keyboard over there, so let the run start that way. **Not taken**, and the reason is that it reads a flag as saying something it does not. `--dry-run` picks the mode — whether this run will ever take the keyboards and open a link at all — and which machine the keyboard is driving is a state inside that mode, moved by the chord while the run goes on. Deriving the second from the first ties them together, and they come apart the moment the run is started by a logon rather than by the person who typed the flag: the flag was typed once, and what it would then be saying at every logon after that is that the keyboard moves to the other machine before anybody has touched it. The cost of the other way round is a chord on the way in, pressed by somebody who is already at the keyboard.
