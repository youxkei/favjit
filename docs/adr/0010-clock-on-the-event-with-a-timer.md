# ADR-0010: Put the clock on the event, and give the host surface one replaceable timer

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

[ADR-0006](0006-host-boundary.md) puts the clock behind the host, for timing-dependent conversion rules. Two rules need it, and they need different things.

The first is the tap-versus-hold decision behind the built-in keyboard's space bar — shift while held, a space when tapped — so something has to tell a tap from a hold. It needs no wake-up: if the modifier is sent lazily, nothing is due at the window's edge, and whether the window has closed can be read off the release's own timestamp when it arrives.

The second is key repeat, and it is not of that shape. `docs/platform/macos/key-repeat.md` records what the capture path delivers for a key held down: one HID value for the press, one for the release, and nothing in between. There is no repeat arriving to forward, so favjit has to be able to produce them — and a repeat is a thing that has to *happen* at a deadline with no input to hang it on. The same document records where the machine keeps the rates it should happen at: inside the `HIDParameters` dictionary on the `IOHIDSystem` registry entry, 250 ms then every 33.3 ms on this machine.

[ADR-0009](0009-trace-and-replay.md) sets the constraint over both. `core` is a deterministic function of its event stream, and **anything `core` reads that is absent from the trace makes a replay diverge**. A `now()` return value is exactly such a read: keeping replay honest would mean recording the call and its result, which is the same information an event timestamp already carries.

## Decision

Every `HostEvent` carries the time it happened. There is no `now()` at the host surface.

The surface has one timer: `set_timer(Option<Instant>)`, where `None` cancels. **At most one wake-up is outstanding, and setting another replaces it.** It comes back as an ordinary `EventKind::Timer` on the same stream as everything else, carrying its own timestamp like any other event.

A hold is lazy. The modifier reaches the OS when a key that needs it arrives, never before, so the tap window needs no wake-up of its own.

`Injected`'s modifier set names the modifiers in effect **at the OS**. A hold that has not been sent yet is matched against by rules and absent from injected events.

The repeat rates are configuration handed to the sink at start, beside the layout — not read through the host. A repeat belongs to the physical key that last emitted a non-modifier keystroke, and re-sends what that key resolved to when it went down.

## Consequences

- Replay needs no extra machinery: a recorded trace already holds every timestamp `core` ever saw, and a wake-up is an event in it like any other.
- The host surface carries a timer it would not otherwise need. Everything inside a host is beyond the reach of the simulated suite, so each operation there is a cost.
- One timer with no identity means no cancellation bookkeeping and no way for two deadlines to disagree about which is outstanding. It also means the day a second concurrent deadline is wanted, this is what gets revisited.
- Under the simulator the clock is the script's cursor and a wake-up is delivered when it comes due before the next scripted event, so timing rules and repeats are exact, instant, and in time order. A script that ends with a key still held ends the run rather than repeating forever.
- On the macOS side the wake-up is a bounded wait on the capture channel, so the loop stays single and nothing else produces events into it.
- Tapping a hold key produces no modifier event at all, in either direction. A non-lazy hold would wrap every tapped space in a shift down and up that applications can see.
- **The modifier set rules match against and the set stamped on events are not the same set.** So `Outcome::Emit` reports which modifiers it consumed and which it added, rather than a finished set: only the sink knows what the OS has actually been told.
- A repeat re-sends the recorded output, modifiers included. **Pressing shift while a key is already repeating does not shift the repeats** — the stream continues as it started.
- A hold that is still undecided does not repeat, and a modifier neither repeats nor interrupts what is repeating.
- Reading the rates once at start means changing them takes a restart to take effect.
- **The tap window's length is favjit's choice, not a ported one.** The Karabiner configuration this layout came from sets no tap-window parameter, so it runs on a default whose value is recorded nowhere here.

## Alternatives considered

### `now()` at the host surface

The obvious shape, and how a program normally reads a clock. Not taken: it is a read that does not appear in the event stream, so ADR-0009's replay diverges unless the call and its result are recorded as well — carrying the same information the event timestamp carries, for an extra operation and an extra thing to get wrong.

### No timer at all

Available only if nothing is ever due at a deadline. That holds for the tap window, with a lazy hold, and does not hold for a repeat: a repeat is precisely a thing that happens when no input arrives.

### A set of timers with identities

Rejected as machinery for nothing. Exactly one deadline is ever outstanding — the key currently repeating — so identities would buy only the ability to cancel one of several, which nothing asks for.

### A thread with a timer, handing repeats to the loop

Rejected. A second producer into the event stream is the one thing that could reorder it, and [ADR-0006](0006-host-boundary.md) keeps each role single-looped so that the suite stays deterministic. The thread would also have to hand its event over through the same channel a bounded wait already covers.

### Forwarding the OS's own repeats

Not possible on the capture path favjit uses: nothing arrives to forward (`docs/platform/macos/key-repeat.md`). Reaching a level that does deliver repeats would mean giving up the per-device attribution and the unnamed-key access that path was chosen for.

### Sending the hold eagerly and retracting it on a tap

Simpler state — no lazy bookkeeping, no two modifier sets. Not taken: a shift down and up lands around every tapped space, which applications can observe and some act on. It also gives the tap window a deadline of its own, since the retraction has one.

### Resolving the key again on each repeat, or recomputing its modifiers

Rejected. Re-resolving would let a layer taken up mid-repeat change the key under the user's finger, and leave the eventual key-up releasing something other than what went down. Recomputing only the modifiers would need the rule's consumed and added sets a second time, and a shift the rule added itself would compound with one the user is holding.

### Reading the rates through the host, per keystroke

Rejected: a value `core` read that no event carries is exactly what ADR-0009 rules out. Handing them in as configuration puts them where the layout is, which is also what a recorded trace needs to replay.

### Wall clock instead of a monotonic one

Rejected, for the reason ADR-0009 already gives for refusing to order a merged trace by wall clock: it moves, and the amounts it moves by exceed the intervals being measured here.
