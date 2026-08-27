# The tray item is its own program, and reaches favjit through favjit's own window

- **Date**: 2026-09-01

The findings it rests on are [window properties and messages](window-properties-and-messages.md)
and [hooks and raw input](hooks-and-raw-input.md).

## Context

[ADR-0013](../../adr/0013-a-chord-moves-the-keyboard.md) gave the keyboard a way to move between the machines, and the chord is the whole of it. Three things a person at this machine cannot find out: whether favjit is running, whether it has found the Mac, and which machine the keyboard is currently driving. All three are in the log, which is not on the screen.

The Mac has [an item in its menu bar](../macos/menu-bar-item-as-its-own-agent.md), called there "the escape that does not need the keyboard... reachable with the trackpad, which favjit never captures whatever it is doing to the keyboards".

**That last clause is not true of this machine.** A relaying run here refuses the pointer along with the keys — the mouse is read through raw input and refused through the hook ([ADR-0011](../../adr/0011-pointer-feel-split-between-engine-and-the-device.md) is why it is captured at all) — so while the keyboard is the Mac's, this machine's pointer is frozen and there is nothing here to click. What covers a favjit that is refusing wrongly on this machine is the watchdog ([ADR-0008](../../adr/0008-input-suppression-and-watchdog.md)), and it already does.

So the question is not what the Mac's item answers. It is: where does the state a person needs to see live, and how does something outside the converter read it and ask it for something.

Two constraints on the answer:

- The thread that holds the keyboards is the loop, and ADR-0008 rests on it coming back round. Nothing that draws a menu may run on it, and nothing outside it may make a call that waits on it.
- The state has to stop existing when the run does. A person looking at an item that says the keyboard is the Mac's, while nothing at all is forwarding, has been told the one thing that would send them to the wrong machine.

What the OS gives for this was measured before deciding: a window property is readable from another process and a message can be posted to a window from another process, neither call waiting on the owning thread, and both gone when the process is ([docs/platform/windows/window-properties-and-messages.md](../platform/windows/window-properties-and-messages.md)).

## Decision

A separate binary, `favjit-tray`, which holds no state of its own.

**The channel is favjit's own capture window** — the message-only window raw input is already delivered to, found by its class name. What is being refused is published on it as a window property; an ask for the keyboard is a message posted to it, which the capture loop turns into an event on the same stream the keystrokes arrive on. **What the channel says is a crate of its own, `tray-wire`**, the way the link's frames are `link-wire`: where the window is found, what the property is called, which message an ask arrives in and what each number means are one copy that both programs read, because a second copy is not an error anywhere — it is a click that does nothing, and an icon drawn from a state nobody published. It is not in either program because both read it, and it is not in a host because every name in it is favjit's own rather than the platform's, which is the one thing [ADR-0006](../../adr/0006-host-boundary.md) does not let a host state.

The calls are the host's, one per operation: publishing the state is the run's, and finding the window and reading the property are two operations the item composes — a single call that did both would hide the answer that matters most, which is that there is no window, so nothing is forwarding at all.

**The item draws four states, not three**: the keyboard is this machine's, the keyboard is the Mac's, this machine's with no link to the Mac, and nothing running. The last is what makes the other three worth trusting.

An ask names the machine it wants rather than saying "the other one", so an item drawn before a chord moved the keyboard cannot move it back by being clicked.

`favjit --to-the-mac` and `favjit --back-here` are the same ask from a terminal, the way `favjit --disable` is the Mac item's ([the daemon install](../macos/install-as-a-daemon-and-turn-off-with-a-file.md)).

**It is a window-subsystem program with no console, and it logs to a file** under the same directory as the identity. A tray item started at every logon by a console program is a black window on the desktop for the whole session, and there is nothing for a console to carry: what this program has to say, it says by being an icon. The file is for the two failures where it cannot be one — the notification area refusing the item, and the icon refusing to be drawn — which without it would be a tray item that silently is not there.

**It is not the escape the Mac's item is.** While the keyboard is the Mac's it cannot be clicked, and it says so rather than offering something that will not happen.

## Consequences

- A person can see what favjit is doing without reading a log, including the state that until now looked identical to every other kind of silence: no link to the Mac.
- The state cannot be stale in the way a file's would be. No window means no favjit, and the item says so.
- The state the item draws is set from the same call that changes what the hooks refuse, so the two cannot come apart.
- Neither reading the state nor asking for the keyboard waits on the thread holding the keyboards, so a wedged favjit makes the item useless rather than making it hang.
- The GUI dependencies stay out of the converter, which is [ADR-0005](../../adr/0005-crate-layout.md)'s arrangement and the reason the Mac's item is split out as well.
- Clicking is worth nothing while the keyboard is the Mac's. That is a real hole and it is the watchdog's, not this item's: the pointer comes back when the run ends, and ending a run that is not answering is what the supervisor is for.
- Two more flags on `favjit`, and they are the only ones that act on a run other than this one.
- What the suite drives is the ask itself: it arrives as an event on the same stream the keystrokes do, so a run is driven through it exactly as it is driven through the chord, and both are pinned in `crates/e2e/tests/switching.rs` — the keyboard goes over, comes back, and an ask naming where it already is moves nothing. The window, the property and the calls are a host's, and what they do was measured on the machine instead.

## Alternatives considered

### A control file, the way the Mac's off switch is one

The mechanism already in the repository, and it needs no window. **Not taken**: a file outlives the process that wrote it, so an item drawn from one would go on saying the keyboard is the Mac's after favjit had stopped — which is precisely the state a person consults the item to rule out. The Mac's file does not have this problem because what it records is a person's wish rather than a run's state, and a wish does outlive the run.

### The item inside the converter, on a second thread

One process to install and start. **Not taken**: it puts a GUI toolkit in the process that holds the keyboards, and the reason ADR-0005 keeps `core` free of them applies with more force to the one binary that must not be doing anything but its loop. A second thread inside it would also be the first concurrency in a program whose determinism the suite rests on.

### `SendMessageW`, or `GetWindowTextW`, to read the state

Fewer moving parts: one call, an answer straight back. **Not taken**: both are answered by the owning thread, which is the thread that might be wedged. An item that hangs when favjit hangs is worse than one that says nothing, because the hang is on the person's screen.

### A named pipe or a local socket between the two

The general answer, and it would carry more than three states. **Not taken**: it is a second thing to keep alive, and it needs deciding what happens to a listener whose peer has gone — which is the staleness above, back in another form. The window already exists, and it is destroyed with the process by the OS rather than by any code of favjit's.

### One item that toggles

**Not taken**: for [ADR-0013](../../adr/0013-a-chord-moves-the-keyboard.md)'s reason for two chords rather than one. A menu is drawn once and clicked later, so a toggle would act on the state as it was drawn.

### The supervisor drawing the item

The one process that knows *why* favjit is not running, and the only one that could start it again — which would make the item's fourth state something a person could act on instead of only read. **Not taken**: the supervisor is the single program whose staying responsive is what the keyboards coming back depends on ([ADR-0008](../../adr/0008-input-suppression-and-watchdog.md)), and a toolkit whose event loop wants the main thread would split its supervising loop onto a second one. What made it attractive is also what removes the need for it: the logon task restarts the supervisor when it fails, which is the same job launchd's `KeepAlive` does on the Mac ([logon tasks](logon-tasks.md)), so "favjit is not running" is a state that ends on its own rather than one waiting for a click.

### A Quit item

**Not taken**: nothing restarts this — there is no launchd here — so a Quit would take the item away until the next logon. What a person reaching for it wants is almost always to stop forwarding rather than to stop being told about it, and stopping forwarding is bringing the keyboard back.
