# ADR-0009: Record a bounded trace in host-provided memory the watchdog can read, and replay it through the simulator

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

The simulated suite reaches everything above the host boundary, but misbehavior happens on real machines, where it does not reach. A report that "a modifier got stuck this afternoon" is not something the suite can be pointed at.

`engine` is a deterministic function of its event stream ([ADR-0006](0006-host-boundary.md)), which makes that stream sufficient to reconstruct what happened. And because the simulator drives `engine` through the same stream ([ADR-0007](0007-deterministic-e2e.md)), a recording taken on real hardware is the same shape as a simulated script.

Where the recording lives is decided by when it is wanted. The occasions that most need a trace are the ones where the process cannot produce one: a hang holds the trace in memory and cannot be asked for it, and a termination runs no code at all. A panic handler covers neither. Worse, [ADR-0008](0008-input-suppression-and-watchdog.md) has the watchdog end a wedged process, so the action that gives the machine back would also destroy the record of why it was needed — unless something outside the process can already see it.

Two constraints bound the answer. The trace is a fixed-size structure rather than a growing one. And a trace of keystrokes is a keylog.

## Decision

The trace holds every `HostEvent`, every outbound call together with its result, and periodic checkpoints of `engine`'s full state. Its vocabulary is the same `HostEvent` the simulator uses, so a trace loads directly as a script.

**A result is the machine's own code, not whether it worked.** A boolean answers the one question nobody is left asking by the time a trace is read: that something failed is already visible from what did not follow it, and which failure it was is the whole of what a reading is for — a write that timed out and a connection the other end reset are the same `false` and different problems. So an outbound operation answers with the number the platform returned, zero for the call that worked ([ADR-0006](0006-host-boundary.md): a code is carried as the code), and the record holds that number. It fits because a code is fixed-width, which a sentence is not: the words a person reads are composed in `engine` from the code, and the trace is where the code itself survives.

**What `engine` decided is recorded apart from what the machine answered.** An injection has two results — whether the run could render it into a report at all, and what the device said when it was written — and one field cannot hold both. A trace carrying only the first would show a keystroke as delivered while nothing reached the machine, which is the reading that matters most in the case a trace exists for.

**The memory it occupies is provided by the host** ([ADR-0006](0006-host-boundary.md)), not allocated by `engine`; `engine` writes into a buffer it is handed and does not know what backs it. On a real host that memory is a shared region — not file-backed — that the supervising watchdog can read. Under `host-sim` it is ordinary process memory. Providing that region is the host's operation; how large a window to retain is `engine`'s policy, stated there once and handed to every host operation that needs the size.

Eviction is by segment — a checkpoint plus the events following it — never by individual event, so what remains always begins at a checkpoint. A checkpoint is taken on whichever comes first of an event count, a byte budget, or one minute.

**There is one recording, and it is the sink's.** The source's records cross the link to it, so what a person reads is one region on one machine. The alternative is two files to be merged afterwards, which puts the question a reading exists to answer — which end let a link go — behind having both machines' files in one place, and a person whose link keeps dropping has one machine in front of them.

**The source keeps what it cannot send, and sends it once a session is up.** The records that explain a dropped link are made when there is no link to send them over, so a design that only carried what could cross live would lose exactly the ones being looked for. They cross on the session after, behind that session's keystrokes rather than in front of them: what a run owes the person at the keyboard comes first, and the backlog goes over on the waits that come back empty and on the way out.

Wall clock is recorded for reading, never for ordering. **The order in the region is the order the records were written into it**, which needs no clock the two machines share.

**A record that crossed is named in both ends' records by the transport's own count, not by a field favjit puts on the wire.** A sequence number added to a frame would be one more thing that can disagree, which on this link means a record that opens to the wrong thing rather than an error. The Noise session counts every record either end sends or receives; the sender writes that count beside what it sent and the receiver beside what arrived. Two ends that disagreed about it would be a record that would not open at all.

**Which machine wrote a record is part of the record.** A replay reproduces the sink's own run, so it reads the sink's records only: a checkpoint's state taken out of a reading that mixed both would take whatever the other machine happened to have written next as this one's.

**The recording is asked for while the run is going, and the supervisor answers without ending it.** The reading that matters most is of a run that has not stopped: a link that keeps dropping wedges nothing, so the ending never comes, and a recording only offered at the ending would never be read in the case a person is actually looking at. So the supervisor listens, and hands the bytes over on the way round — bounded, because a reader that stops reading must not stop the supervisor, which is the one thing ADR-0008 does not allow.

**The bytes, not a path.** The supervisor runs as root; a path it took from whoever asked would be a way to have root write anywhere. The asking process writes the file, at its own privilege.

**The supervisor keeps the run before the current one, and it does the keeping when it starts rather than when a run ends.** Every ending worth reading also ends the supervisor: the process it supervises going away is what it exits on, and being killed from outside runs no code in it at all. So a supervisor that handed the recording over on the way down would cover only the endings it was still there for, which excludes the ones a person is holding the trace to explain. Two regions with names of their own answer it instead: the supervisor moves what is in the live one into the kept one, clears the live one, and only then starts the run. Nothing is asked of the process that is gone, and the recording of the run that failed is still there one run later.

**So the regions outlive the processes that map them, and what guards them is mode.** A region unlinked while open is unreachable to anything that comes after it, which is the property being given up; the reach that replaces it is root's, and root can already ask the supervisor for the recording over its socket. An answer carries both recordings, live first, because the alternative is the supervisor reading a request from whoever asked — a read from something that may never write, in the loop that must not stop ([ADR-0008](0008-input-suppression-and-watchdog.md)).

**The live region is cleared before the run starts, in full.** A run that records nothing would otherwise answer for itself with the run before it, and a recording that names the wrong run is worse than an empty one. Cleared in full rather than at the few bytes a reader goes by, because which bytes those are is the recording's own format and the supervisor does not have it.

**Only the machine that keeps the recording is asked.** The forwarding machine's records are already in it, so a second place to ask would be a second half-answer to the question a reading exists for.

When the watchdog ends a process it retains what the region held and offers it. Nothing is written out or transmitted without an explicit user action, and what a trace contains is stated at that point. Checkpoints exclude key material.

## Consequences

- A field incident becomes a reproducible case, and then a regression test.
- The trace survives a hang and a termination, covering what a panic handler cannot reach.
- **Replay reproduces `engine`'s behavior exactly; it does not reproduce a defect inside a host implementation.** On replay the host is replaced by its recording, so a host bug shows up as a record of what the host did, and the fix is in the host. The seam does not move.
- **Completeness is load-bearing.** Anything `engine` reads that is absent from the trace makes replay diverge, which is a second reason for the rule that `engine` reaches nothing impure directly. Recording outbound calls without their results would break it, since a rejected injection changes what `engine` does next.
- Writing to memory `engine` was handed costs nothing beyond a memory write. There is no message passing in the interactive input path, so mouse motion rates cost nothing extra, and there is no backpressure question — which matters, because dropping events would break the completeness replay depends on.
- Because the buffer comes from the host, the simulated suite needs no shared memory at all: `host-sim` hands over plain process memory and the same `engine` code exercises the same path.
- The watchdog copies a region rather than interpreting it, so [ADR-0008](0008-input-suppression-and-watchdog.md)'s smallness is preserved.
- **A trace of keystrokes is a keylog.** It holds whatever was typed in the retained window, passwords included. This is inherent rather than incidental — replaying a conversion bug needs the actual keys — so it is handled rather than mitigated: nothing is persisted or uploaded on its own, and a user is never asked to attach a trace without being told what is in it.
- **Not being file-backed is load-bearing for the same reason.** Mapping the region to a file would leave a keylog on disk permanently, and saving automatically on every failure would leave one after every failure.
- A retained trace is lost on reboot. That is the price of keeping keystrokes off disk, and it is the right way round.
- **A run that fails is readable from the run after it, which is what makes the common failures debuggable at all.** The endings that matter most — the output device's connection going, a link that will not come back, a kill from outside — are all endings the supervisor does not outlive, so a recording kept only by the process that held it would never be read.
- Two runs' keystrokes are in memory rather than one's, and the names are reachable to root for as long as the machine is up. The watchdog can read keystrokes either way, which makes it a sensitive process; its privileges, and who else can map the regions, belong to the same security surface as [ADR-0004](0004-peer-authentication.md).
- Excluding key material from checkpoints has to be settled when the checkpoint serialization is written. A checkpoint carrying the private half of the pinned identity from [ADR-0004](0004-peer-authentication.md) would turn a trace file into a credential leak.
- The retained window varies with what the user is doing. Mouse motion arrives at a rate far above typing, so an event-count and byte budget alongside the time interval is what keeps the window from collapsing during mouse use.

## Alternatives considered

### Merging the two traces on wall clock

Not taken, and it is the most tempting mistake here. Two machines' clocks skew, and the skew can exceed the intervals being investigated — the gap between a key being captured and the converted event being injected. Sorting by wall clock can therefore present causally ordered events in reverse, with nothing to indicate it happened. Sequence numbers give an exact order for anything causally related and leave genuinely concurrent events unordered, which is the honest result.

### `engine` owning the buffer

Not taken. Allocating a shared region is an OS operation, and `engine` reaching for one directly is exactly what [ADR-0006](0006-host-boundary.md) rules out. Taking the buffer from the host also removes shared memory from the test path entirely.

### Streaming the trace to the watchdog

Not taken. It puts a message write in the interactive input path, and forces a choice between blocking the loop and dropping events — and dropping events breaks replay. It also moves the ring buffer's eviction into the watchdog, which is the one component that should stay trivial.

### Handing the recording of a failed run over on the way down

Not taken. It covers only the endings the supervisor is still there for, and the ones worth reading are the ones that end it too — a kill from outside runs no code in it, and the process it supervises going away is what it exits on. Keeping at the next start asks nothing of the process that is gone.

### Writing every run's recording out as it ends

Not taken. It is a keylog on disk after every restart, and on this machine a restart is what every wake from sleep produces (`docs/platform/macos/virtual-hid-device.md`) — so the file would be rewritten constantly and the interesting one overwritten by the next boring one. Writing one out stays an explicit action, and what makes it possible is the recording still being there to ask for.

### Letting whoever asks name which recording it wants

Not taken. The supervisor would have to read a request before it could answer, and a read from something that may never write is the supervisor stopping for the one thing [ADR-0008](0008-input-suppression-and-watchdog.md) does not allow. Both go over, in an order stated once.

### One region, written twice

Not taken. The recording has to be somewhere the next run is not writing, and a single region holds the new run's first second in place of the failed run's last minute.

### Dumping to a file from a panic handler

Not taken as the mechanism. It covers panics and misses terminations and hangs, which is most of what the watchdog exists for. It can still run as a convenience for the panic path; it cannot be what the design relies on.

### Mapping the region to a file so the contents land on disk unattended

Rejected. It is the most direct route to the trace surviving anything, and it makes a permanent on-disk keylog the normal state of the system.

### A fixed one-minute checkpoint interval alone

Not taken. Under mouse motion a minute holds orders of magnitude more events than a minute of typing, so a fixed interval makes the retained history depend on the input type rather than on a budget.

### Evicting individual events from the front

Not taken. It eventually drops the checkpoint the remaining events depend on, leaving a suffix from which no state can be computed.

### Recording only outbound calls

Enough to see a symptom, not enough to replay one. Without the inbound events there is no input to feed `engine`.

### Redacting key identity from the trace

Not taken. It removes exactly what a conversion bug needs, which is most of what traces are for. The privacy problem is addressed by controlling where traces go, not by making them useless.
