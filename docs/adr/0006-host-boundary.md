# ADR-0006: Put everything a test cannot drive behind a per-platform host, and make each role a single loop over what it supplies

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

Nothing about this project can be tested by pressing keys on real machines and watching the screen. Not at the volume needed and not for the cases that matter most — but before either of those, because **trying a suspect build by hand is the accident and not a test of it**. What favjit does when it is wrong is take the keyboard away from the person at it, and a keyboard that is gone is also how the run would have been stopped. So a build is not tried in order to find out whether it works: what it does has to be known before it runs, and [ADR-0008](0008-input-suppression-and-watchdog.md)'s watchdog is there for the occasions when knowing was not enough.

That is what sets the price of anything left on the machine's side of the line, and the price is not proportional to size. A judgement a host makes is discovered by the accident it causes, so there is no amount of it small enough to be worth leaving there — a warning given once too often and a poll interval chosen by hand are as far out of reach as a handshake would be. So the line between logic that can be driven in a test and the machine underneath has to be drawn deliberately rather than falling wherever the platform code happens to end.

Four things are on the far side of it, because nothing but a machine can answer them:

- OS input: capturing key and mouse events, suppressing them, injecting converted ones
- The transport between the two machines
- The clock
- The supply of events — that is, concurrency

The clock belongs there for a concrete reason. Layout conversion needs timing: distinguishing a tap from a hold, treating a held key as a modifier. Against a real clock those rules can only be tested by waiting, which is both slow and unreliable at exactly the boundaries that matter.

The last item is where the shape of `engine` gets decided. The sink's inputs look concurrent — key events from local keyboards, devices attaching and detaching, packets arriving, timers for hold thresholds, the link going down — and the obvious structure gives each its own thread. That structure costs something that lands on testing: a run explores one arbitrary interleaving chosen by the OS scheduler, and no assertion can be made about a result until it has arrived, so every assertion becomes a wait with a timeout. A timeout that fires on a loaded machine is indistinguishable from a real hang.

Those inputs do not actually need separate threads. Every one of them is "something happened outside", and they can arrive on one stream.

One property shapes everything else: whatever ends up inside this boundary can only be exercised on real hardware. It is invisible to any test that replaces the machine.

It decides how coarse an operation may be as well as where the boundary runs, because a host can hold a great deal while still looking like a wrapper. Two operations named for what they achieve — "shake hands with whatever connected", "give me the next frame" — would hold the order of a handshake, the lengths its reads use, what becomes of a message that will not open, and the constants the machine at the other end has to state identically. None of that is a platform, and all of it would be beyond reach.

## Decision

Every operation a test cannot drive goes behind a host facade, one per platform: OS input, transport, clock, and the supply of events. `engine` makes none of those calls itself.

Those inputs arrive as a single `HostEvent` stream, and each role is one loop over `host.next_event()`. `engine` spawns no threads and holds no internal concurrency. Where real concurrency is unavoidable — OS callbacks, sockets — it lives inside the host implementation and is funnelled into that same stream.

The concrete host is constructed by whatever owns `main` — a binary or the end-to-end harness — and handed to the role loop as `&mut dyn`. `engine` names no host crate and contains no conditional compilation ([ADR-0005](0005-crate-layout.md)).

**What may live in a host is what the suite cannot drive, and nothing else: the OS calls, the sockets, the filesystem, the run loops, the threads.** Everything else is `engine`'s, however much it looks like the platform's own — a table of HID usages or of scancodes, the bytes of a report and the state one describes, the construction of a session two machines share, the path three processes agree on. Neither *"it means nothing on the other platform"* nor *"it is pure"* is the test. The first is true of every table and leaves them where no test can reach them; the second only asks for two pure places instead of one, with the suite able to reach the first.

**A unit test inside a host says the line has moved.** Something a unit test can drive is something that decides, and a call into the platform reporting what happened cannot be driven that way at all — which is the whole reason this boundary exists. The one exception is a test that exercises the IO itself: a socket taking a connection, a file whose absence means converting is on. Anything else found under test in a host is logic to move, and its test belongs in the suite.

**A host decides nothing.** That is the rule, and the ones below it are how it is kept rather than additions to it.

**Every operation on a host boundary is one call into the platform, reporting what happened, and the order over them is `engine`'s.** Each step of a sequence is its own operation, named for the call it makes rather than for what the sequence achieves, and what to do about its result is decided in `engine`. One call per operation is how an order is kept out of a host: two calls in one body have an order between them, and an order is a decision.

**Counting calls is a check on the rule above and never a statement of it.** A body with one call in it, or with none at all, holds a decision just as easily — which branch is taken, how long to wait, how often something happens, what is said about it. All of those are invisible to a count, so a host measured only that way is a host measured against the half of this decision that is easiest to see.

**Four kinds of decision make no platform call, and none of them is a host's:**

- **What is said.** A line of log is a choice of words about what a fact means, and the fact is the thing that crossed the boundary. `Host::warn` is the operation for it: `engine` composes the words and the host writes them where this machine's log goes. A host that formats its own message has kept the decision and handed over only the typing.
- **How often it is said.** A warning given once rather than per event is a policy, and the flag implementing it is state a host keeps in order to run that policy. Which occurrences are worth reporting is `engine`'s, decided from the facts it was given each time — including the ones it chooses to say nothing about.
- **How long to wait, and how often to look.** A poll interval, a retry limit, a number of failures before giving up: each is a bound on a machine's behaviour that the machine did not state, and a run that had been given a different one would behave differently in a way no test could show.
- **A constant that is favjit's own judgement.** The test is not whether the other machine has to agree on it. It is whether the machine would reject any other value: a struct's size, a protocol's version, the byte naming a request are the platform's own vocabulary and stay in the host, while anything the machine would have accepted a range of was picked by favjit and belongs in `engine`.

**"Reporting what happened" means the platform call's own return value, unwrapped or reshaped, and nothing besides.** A comparison against a constant, a length checked against what was expected — every one of those is `engine`'s, called from `engine`, even where the number being compared can only be produced by a call the host made. A host operation that calls into `engine` has not stopped being a decision for having borrowed the words to make it: the call moved, the decision did not. Where a construction two machines share is driven one step at a time — a session's handshake among them — `engine` holds the steps, and a host operation supplies only what a step needs that `engine` has not got, such as entropy.

**So the body of a host operation is three things and has room for nothing else: the arguments turned into what the platform's own call takes, that call, and what it returned turned back.** Neither conversion consults anything — a number is widened, a pointer is wrapped, a code is carried as the code — because a conversion that reads a table or compares against a constant is the decision again under a name that sounds like plumbing.

**A platform's own way of naming a key crosses the boundary unchanged.** IOKit reports a HID page and a usage, raw input reports a scancode, and turning either into the key a layout is written against is a table. So the host carries the numbers and `engine` names them. That is the rule above and nothing more, said separately because this is where a table sitting beside the API that produced its input reads as cohesion rather than as a decision out of reach: the tables *are* the vocabulary the layout is written in, and a key the layout has a rule for and a table has no name for converts on one machine and not the other.

**Every rule here is held by something that fails a build or a test run, and not by a reading of this file.** A rule that is only read is held as well as its last reader, and the reader with the most occasion to break it is the one adding a platform call to make a machine work. So each rule is attached to a mechanism that refuses:

- **A host crate depends on neither `engine` nor `log`.** The first keeps the vocabulary out ([ADR-0005](0005-crate-layout.md)); the second means a host cannot write a line at all, so the words a run says exist only where `engine` composed them and `Host::warn` is the only way out of the process. Both are refusals by cargo, which is the strongest kind available: the code does not compile.
- **What a missing dependency cannot refuse, a check over the hosts' own syntax refuses.** It runs in the suite rather than beside it, so a `cargo test` that passes is a claim about the boundary and not only about behaviour, and it fails on a message a host writes through `std`, on a bound or a limit a host states for itself, on a `#[test]` outside the inventory below, and on a function making more than one call into the platform.
- **The exceptions are a list in one place, and an entry is a violation on record rather than a permission.** Each names the function and the reason it is there, so what remains is countable and shrinks; a rule whose exceptions are spread through the code it governs is a rule nobody can total up.

**None of this is reachable from inside a host, because a host crate does not depend on `engine`** ([ADR-0005](0005-crate-layout.md)). These traits and the types their signatures are written in are the whole content of the `host` crate — definitions, and no code at all — and `engine` holds every operation over them. A host that would decide something has to write the decision out itself, in a crate whose whole content is calls into a platform, where it is visible as what it is.

**Waiting is not an exception, and it takes an argument rather than deciding one.** Supplying the next event means blocking, and the block has to be bounded — but which bound is the sooner one is arithmetic over instants `engine` already holds (a wake-up it asked for, the bound the run was given, a responsiveness interval `engine` states once as a constant), and arithmetic over values already in hand is not a platform call, so `engine` does it itself before the host is ever entered. What the host receives is one deadline, already the only one there is by the time it arrives; converting that deadline into whatever unit the platform's own wait takes is the argument conversion, the wait itself is the one call, and the outcome — a value, or that none arrived by then — is what comes back turned into the answer. No host operation folds several deadlines into one, because folding is exactly the decision this rule has no room for.

**A condition that might explain why nothing is arriving is its own operation, not a branch inside waiting.** Whether the output device is still connected, whether the run was asked to stop, whether converting is switched on: each is one call reporting one fact, on its own, and never combined with another inside a single operation. Which of them explains an empty wait, and in what order they are worth asking, is decided in `engine` once it has each answer separately — a host that asked them together, or in a chosen order, would be running the sequence this rule keeps out of reach.

**Starting a loop is a host operation, and what runs on it is `engine`'s.** Where a role needs a second loop turning — one that waits on something the role's own loop must not wait on — `engine` hands the host that loop to turn alongside its own, in one call. The loop itself is `engine`'s, so the sequence it follows stays where the suite can drive it, and `engine` still spawns nothing.

**That loop coming back is an event on the stream, in its place.** Whatever it was doing is not being done any more, and the run is what decides what that means — so the machine reports it where every other fact about the outside arrives, behind everything that loop already put there. A flag the run polled instead would be read at the top of a wait, ending the run with those events still unread.

This boundary governs everything whose order has to be checkable, the watchdog of [ADR-0008](0008-input-suppression-and-watchdog.md) included: its judgement is a module of `engine`'s and reaches its machine through a host like any role. What it does not have is a host *crate* — a supervisor needs a pipe and a way to end a process, and reaching those through the crate that also holds the capture and the injection would put a platform's whole surface inside the program that must stay trivial. So its platform halves live in `bin-watchdog` beside its `main`, which is the one place [ADR-0005](0005-crate-layout.md)'s layout is departed from.

## Consequences

- `engine` never calls `std::thread`, `std::time`, or the network. Everything it reads arrives through the facade.
- The transport implementation is platform-independent, so the two facades share it internally. **The surface is per platform; the implementation need not be.**
- `host-sim` is not gated on `target_os` and builds anywhere, which is what allows one process to run both roles and makes a cross-machine test possible at all.
- Timing-dependent conversion rules become exactly testable, including at their boundaries.
- `engine` has no data races to find, so a deterministic test suite gives up nothing by not exploring thread interleavings. Races that remain live in the host implementations, which only real hardware exercises anyway.
- Assertions in the end-to-end suite are exact rather than timed, and the same loop runs in production and under the simulator.
- **Outbound operations must not block indefinitely at the host surface.** A single loop has no other thread to make progress on, so a blocking send or injection stalls everything, including the timers that would otherwise recover from it.
- The ordering question shrinks to one thing: when several events carry the same timestamp, in what order does `next_event()` return them. That is a property the simulator controls ([ADR-0007](0007-deterministic-e2e.md)).
- **Everything inside a host is beyond the reach of the simulated suite.** That is the reason to keep hosts thin, and it is the seam where the simulator and reality can drift apart.
- More trait methods than a coarser boundary would need. That is the cost of the order being somewhere a test can reach it.
- **A wait is one more operation with the same three-part body, not a fourth kind of thing.** `engine` computes the deadline from instants it already holds and a constant it states once; the host converts that single value into the platform's own wait, makes the one call, and turns the outcome back. Whether the output device is live, whether the run was asked to stop, whether converting is switched on: each of those is a further, separate call, asked and prioritised by `engine` once every wait has come back empty — never folded into the wait itself.
- A sequence a host would otherwise have run is driven by the suite, including each way it can stop early — the failure a step would produce for a step that did not happen is the failure the order exists to prevent.
- **The suite drives everything a host would otherwise have done for itself.** A keyboard the run declines to read, a pointer report assembled out of the element values that describe it, a key that arrives at a usage no table names: each is a step in a run with a test that says what it is, rather than a decision behind the boundary that only a machine sat at can show.
- **A host reports more, and smaller, things.** The numbers a platform hands over cross as they are, so an event carries a page and a usage rather than the key they name, and the run does the naming. The stream is longer for it, and one stream drives both platforms' tables.
- Constants two machines have to agree on are stated in `engine` rather than in either host, since a host that restated one would hold a second copy of an agreement. A copy that disagrees is not an error but a read waiting for bytes nobody will send. That is the sharpest case and not the edge of the rule: a constant nothing at the other end cares about is still favjit's judgement if the machine would have taken another value, so a poll interval and a retry limit are `engine`'s on the same terms as a record length. A host that needs one is handed it at the call rather than reading it, since reading it is the dependency [ADR-0005](0005-crate-layout.md) removed. **The same holds for the construction and not only the constants** — a session's handshake, a report's layout — which is why the cryptography both ends perform is `engine`'s and each host only carries the bytes.
- **A thread inside a host reports a failure on the stream or not at all**, since it has no line of its own to write and no `engine` to call. That is the same answer the loop coming back gets, and it closes the same hole: a capture thread whose read failed is a fact the run needs, and a log line would have been the only place it appeared.
- A host's own tests are the inventory of what is in it. `host-macos` keeps three, and every one drives real IO: a control file appearing and going, a connection that went away, a source getting in over a socket. A fourth arriving is a question to answer before it is written — can the suite drive this? What the other end of such a test needs, it reaches for itself: a socket test wanting a real handshake at the far end takes the cryptography library under `[dev-dependencies]`, since `engine` is not there to be borrowed from and a test is not the place the absence stops mattering.
- `engine` therefore compiles every platform's tables and libraries, each of them dead weight to the other: HID usages and reports mean nothing to Windows and scancodes mean nothing to the Mac, and both tables are here. That is the price of naming no platform in a `cfg` ([ADR-0005](0005-crate-layout.md)), and what is being paid for it is two tables and a pure-Rust library.
- A future need for genuine concurrency inside `engine` would mean revisiting this, not working around it.
- The name is `host` rather than `api` because this layer is what the program depends on. The environment a process runs in — its OS, its clock, the network it can see — is what the word covers, and in a two-machine test there are two hosts, which reads correctly.

## Alternatives considered

### Leaving these rules to review

Write them down once, here, and hold them by reading the code against them. Not taken: the rules that get broken are the ones a mechanism was not attached to, and the two halves of this decision are not equally easy to read. Whether a body makes two calls is visible at a glance; whether it decides something is visible only to somebody who already has all four kinds in mind, and a reviewer arriving with a checklist built from this file builds it out of whatever is most emphatic in it. A refusal by cargo needs nothing held in mind at all.

### A macro every host operation is written through

Have each operation declared in a form that admits only the three parts — the conversion in, the call, the conversion back — so a body with a second call or a branch in it is a body that will not parse. It is the tightest enforcement available and it needs no separate checker. Not taken: the operations that are genuinely one call are already obviously so, and the ones this would have to admit are the run loops, the callback registrations and the threads, each of which has a different shape. A macro wide enough for all of them stops refusing anything, and one narrow enough to refuse would put the remainder outside itself, where nothing checks them — with every host operation now written in a syntax a reader has to learn before they can see the call being made.

### Coarse operations, named for what they achieve

One call per thing a role wants done — "shake hands with whatever connected", "give me the next frame" — which is a smaller boundary and reads better at the call site. Not taken: each hides a sequence, and a sequence inside a host is one nothing can drive, which is the whole of what this boundary is for.

### Folding the deadlines, and the conditions that might explain an empty wait, into one call

Let the host compute which of several instants is soonest, and let the same call notice whether the output is gone, whether the run was asked to stop, whether converting is switched off, answering with whichever explains what happened. One call at the surface, so it reads as the same kind of thing as every other operation here. Not taken: choosing among several named outcomes from several pieces of state is a decision wearing the shape of a wait, and it is exactly as far from a test as a table would be — the call looking singular at the surface says nothing about what runs inside it.

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

Not taken. The choice is fixed at build time, so a switch looks like the honest shape — but it puts a host crate's name inside `engine`, which reverses the dependency the vocabulary needs to run the other way ([ADR-0005](0005-crate-layout.md)), and it makes one process running both roles a matter of two mutually exclusive builds. `&mut dyn` costs an indirect call at a boundary the process is about to cross into the OS on anyway, and no generic parameter reaches any signature in `engine`.

### A thread per concern inside `engine`

Not taken. It needs locks around state that is otherwise owned by one loop, it makes the end-to-end suite non-deterministic, and it turns every assertion into a timed wait whose failures cannot be distinguished from real hangs.

### An async runtime inside `engine`

Not taken. It has the same determinism problem unless the runtime's scheduling is itself under the simulator's control — at which point the runtime has become part of the host boundary, and the loop is back.
