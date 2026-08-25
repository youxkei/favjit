# ADR-0006: Put everything a test cannot drive behind a per-platform `host`, and make each role a single loop over what it supplies

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

Nothing about this project can be tested by pressing keys on real machines and watching the screen — not at the volume needed, and not for the cases that matter most. So the line between logic that can be driven in a test and the machine underneath has to be drawn deliberately rather than falling wherever the platform code happens to end.

Four things are on the far side of it, because nothing but a machine can answer them:

- OS input: capturing key and mouse events, suppressing them, injecting converted ones
- The transport between the two machines
- The clock
- The supply of events — that is, concurrency

The clock belongs there for a concrete reason. Layout conversion needs timing: distinguishing a tap from a hold, treating a held key as a modifier. Against a real clock those rules can only be tested by waiting, which is both slow and unreliable at exactly the boundaries that matter.

The last item is where the shape of `core` gets decided. The sink's inputs look concurrent — key events from local keyboards, devices attaching and detaching, packets arriving, timers for hold thresholds, the link going down — and the obvious structure gives each its own thread. That structure costs something that lands on testing: a run explores one arbitrary interleaving chosen by the OS scheduler, and no assertion can be made about a result until it has arrived, so every assertion becomes a wait with a timeout. A timeout that fires on a loaded machine is indistinguishable from a real hang.

Those inputs do not actually need separate threads. Every one of them is "something happened outside", and they can arrive on one stream.

One property shapes everything else: whatever ends up inside this boundary can only be exercised on real hardware. It is invisible to any test that replaces the machine.

It decides how coarse an operation may be as well as where the boundary runs, because a host can hold a great deal while still looking like a wrapper. Two operations named for what they achieve — "shake hands with whatever connected", "give me the next frame" — would hold the order of a handshake, the lengths its reads use, what becomes of a message that will not open, and the constants the machine at the other end has to state identically. None of that is a platform, and all of it would be beyond reach.

## Decision

Every operation a test cannot drive goes behind a `host` facade, one per platform: OS input, transport, clock, and the supply of events. `core` makes none of those calls itself.

Those inputs arrive as a single `HostEvent` stream, and each role is one loop over `host.next_event()`. `core` spawns no threads and holds no internal concurrency. Where real concurrency is unavoidable — OS callbacks, sockets — it lives inside the host implementation and is funnelled into that same stream.

The concrete host is constructed by whatever owns `main` — a binary or the end-to-end harness — and handed to the role loop as `&mut dyn`. `core` names no host crate and contains no conditional compilation ([ADR-0005](0005-crate-layout.md)).

**What may live in a host is what the suite cannot drive, and nothing else: the OS calls, the sockets, the filesystem, the run loops, the threads.** Everything else is `core`'s, however much it looks like the platform's own — a table of HID usages, the bytes of a report and the state one describes, the construction of a session two machines share, the path three processes agree on. Neither *"it means nothing on the other platform"* nor *"it is pure"* is the test. The first is true of every table and leaves them where no test can reach them; the second only asks for two pure places instead of one, with the suite able to reach the first.

**A unit test inside a host says the line has moved.** Something a unit test can drive is something that decides, and a call into the platform reporting what happened cannot be driven that way at all — which is the whole reason this boundary exists. The one exception is a test that exercises the IO itself: a socket taking a connection, a file whose absence means converting is on. Anything else found under test in a host is logic to move, and its test belongs in the suite.

**Every operation on a host boundary is one call into the platform, reporting what happened, and the order over them is `core`'s.** Each step of a sequence is its own operation, named for the call it makes rather than for what the sequence achieves, and what to do about its result is decided in `core`. A host that decides anything, or that performs a sequence, decides it where nothing can drive it.

**One operation cannot be a bare call, and it is the wait.** Supplying the next event means blocking, and the block has to be bounded by whichever deadline comes first — the wake-up `core` asked for, the bound the run was given, the interval at which being asked to stop is noticed. Bounding it needs a clock, which `core` has not got; and the conditions that end a stream have to be noticed by whatever is waiting, because a keyboard nobody is typing on produces no event to notice them on. So the host holds that loop, and reports *which* condition ended the stream rather than deciding what it means.

**Starting a loop is a host operation, and what runs on it is `core`'s.** Where a role needs a second loop turning — one that waits on something the role's own loop must not wait on — `core` hands the host that loop to turn alongside its own, in one call, as work that owns everything it touches. The loop itself is `core`'s, so the sequence it follows stays where the suite can drive it, and `core` still spawns nothing.

**That loop coming back is an event on the stream, in its place.** Whatever it was doing is not being done any more, and the run is what decides what that means — so the machine reports it where every other fact about the outside arrives, behind everything that loop already put there. A flag the run polled instead would be read at the top of a wait, ending the run with those events still unread.

This boundary governs everything whose order has to be checkable, the watchdog of [ADR-0008](0008-input-suppression-and-watchdog.md) included: its judgement is a module of `core`'s and reaches its machine through a host like any role. What it does not have is a host *crate* — a supervisor needs a pipe and a way to end a process, and reaching those through the crate that also holds the capture and the injection would put a platform's whole surface inside the program that must stay trivial. So its platform halves live in `bin-watchdog` beside its `main`, which is the one place [ADR-0005](0005-crate-layout.md)'s layout is departed from.

## Consequences

- `core` never calls `std::thread`, `std::time`, or the network. Everything it reads arrives through the facade.
- The transport implementation is platform-independent, so the two facades share it internally. **The surface is per platform; the implementation need not be.**
- `host-sim` is not gated on `target_os` and builds anywhere, which is what allows one process to run both roles and makes a cross-machine test possible at all.
- Timing-dependent conversion rules become exactly testable, including at their boundaries.
- `core` has no data races to find, so a deterministic test suite gives up nothing by not exploring thread interleavings. Races that remain live in the host implementations, which only real hardware exercises anyway.
- Assertions in the end-to-end suite are exact rather than timed, and the same loop runs in production and under the simulator.
- **Outbound operations must not block indefinitely at the host surface.** A single loop has no other thread to make progress on, so a blocking send or injection stalls everything, including the timers that would otherwise recover from it.
- The ordering question shrinks to one thing: when several events carry the same timestamp, in what order does `next_event()` return them. That is a property the simulator controls ([ADR-0007](0007-deterministic-e2e.md)).
- **Everything inside a host is beyond the reach of the simulated suite.** That is the reason to keep hosts thin, and it is the seam where the simulator and reality can drift apart.
- More trait methods than a coarser boundary would need. That is the cost of the order being somewhere a test can reach it.
- A sequence a host would otherwise have run is driven by the suite, including each way it can stop early — the failure a step would produce for a step that did not happen is the failure the order exists to prevent.
- Constants two machines have to agree on are stated in `core` rather than in either host, since a host that restated one would hold a second copy of an agreement. A copy that disagrees is not an error but a read waiting for bytes nobody will send. **The same holds for the construction and not only the constants** — a session's handshake, a report's layout — which is why the cryptography both ends perform is `core`'s and each host only carries the bytes.
- A host's own tests are the inventory of what is in it. `host-macos` keeps three, and every one drives real IO: a control file appearing and going, a connection that went away, a source getting in over a socket. A fourth arriving is a question to answer before it is written — can the suite drive this?
- `core` therefore compiles tables and libraries that some platforms have no use for: HID usages and reports mean nothing to Windows, which speaks scancodes. That is the price of naming no platform in a `cfg` ([ADR-0005](0005-crate-layout.md)), and what is being paid for it is a table and a pure-Rust library.
- A future need for genuine concurrency inside `core` would mean revisiting this, not working around it.
- The name is `host` rather than `api` because this layer is what the program depends on. The environment a process runs in — its OS, its clock, the network it can see — is what the word covers, and in a two-machine test there are two hosts, which reads correctly.

## Alternatives considered

### Coarse operations, named for what they achieve

One call per thing a role wants done — "shake hands with whatever connected", "give me the next frame" — which is a smaller boundary and reads better at the call site. Not taken: each hides a sequence, and a sequence inside a host is one nothing can drive, which is the whole of what this boundary is for.

### Drawing the line at what is platform-shaped

The reading that suggests itself: a HID usage number means nothing to Windows, so it belongs beside the code that speaks HID. It reads well, and it is the line that lets a table of usages, the bytes of a report and the state one describes all end up where the suite cannot see them. The tables are the vocabulary the layout is written against; keeping them at arm's length from the layout makes a gap between the two something only a running machine can show.

### Drawing it at what is pure

Tighter, and still wrong. A table is pure, a report's state machine is pure, and a session's handshake is pure — so this line asks only that a host be handed pure code to hold. Two pure places, one of which no test can reach, is what the boundary exists to avoid.

### Calling the layer `api`

Not taken. The word normally names the interface a component offers outward, and this is the opposite direction: the interface it consumes. Readers resolve it the wrong way round on first encounter.

### Leaving the clock and the transport outside the boundary

Not taken. A real clock means the end-to-end suite waits in real time, which makes timing rules effectively untestable and the suite slow and unreliable. A real transport means the cross-machine test needs real sockets, and with them real timing and real failure modes that cannot be summoned on demand.

### One host crate with `cfg(target_os)` inside

Not taken. It cannot be depended on twice — once per platform — within one process, which is exactly what the cross-machine test needs.

### A compile-time switch selecting the host, by `target_os` or by a feature

Not taken. The choice is fixed at build time, so a switch looks like the honest shape — but it puts a host crate's name inside `core`, which reverses the dependency the vocabulary needs to run the other way ([ADR-0005](0005-crate-layout.md)), and it makes one process running both roles a matter of two mutually exclusive builds. `&mut dyn` costs an indirect call at a boundary the process is about to cross into the OS on anyway, and no generic parameter reaches any signature in `core`.

### A thread per concern inside `core`

Not taken. It needs locks around state that is otherwise owned by one loop, it makes the end-to-end suite non-deterministic, and it turns every assertion into a timed wait whose failures cannot be distinguished from real hangs.

### An async runtime inside `core`

Not taken. It has the same determinism problem unless the runtime's scheduling is itself under the simulator's control — at which point the runtime has become part of the host boundary, and the loop is back.
