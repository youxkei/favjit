# ADR-0009: Record a bounded trace in host-provided memory the watchdog can read, and replay it through the simulator

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

The simulated suite reaches everything above the host boundary, but misbehavior happens on real machines, where it does not reach. A report that "a modifier got stuck this afternoon" is not something the suite can be pointed at.

`core` is a deterministic function of its event stream ([ADR-0006](0006-host-boundary.md)), which makes that stream sufficient to reconstruct what happened. And because the simulator drives `core` through the same stream ([ADR-0007](0007-deterministic-e2e.md)), a recording taken on real hardware is the same shape as a simulated script.

Where the recording lives is decided by when it is wanted. The occasions that most need a trace are the ones where the process cannot produce one: a hang holds the trace in memory and cannot be asked for it, and a termination runs no code at all. A panic handler covers neither. Worse, [ADR-0008](0008-input-suppression-and-watchdog.md) has the watchdog end a wedged process, so the action that gives the machine back would also destroy the record of why it was needed — unless something outside the process can already see it.

Two constraints bound the answer. The trace is a fixed-size structure rather than a growing one. And a trace of keystrokes is a keylog.

## Decision

The trace holds every `HostEvent`, every outbound call together with its result, and periodic checkpoints of `core`'s full state. Its vocabulary is the same `HostEvent` the simulator uses, so a trace loads directly as a script.

**The memory it occupies is provided by the host** ([ADR-0006](0006-host-boundary.md)), not allocated by `core`; `core` writes into a buffer it is handed and does not know what backs it. On a real host that memory is a shared region — not file-backed — that the supervising watchdog can read. Under `host-sim` it is ordinary process memory.

Eviction is by segment — a checkpoint plus the events following it — never by individual event, so what remains always begins at a checkpoint. A checkpoint is taken on whichever comes first of an event count, a byte budget, or one minute.

Source and sink traces are merged on the transport's sequence numbers. Wall clock is recorded for reading, never for ordering.

When the watchdog ends a process it retains what the region held and offers it. Nothing is written out or transmitted without an explicit user action, and what a trace contains is stated at that point. Checkpoints exclude key material.

## Consequences

- A field incident becomes a reproducible case, and then a regression test.
- The trace survives a hang and a termination, covering what a panic handler cannot reach.
- **Replay reproduces `core`'s behavior exactly; it does not reproduce a defect inside a host implementation.** On replay the host is replaced by its recording, so a host bug shows up as a record of what the host did, and the fix is in the host. The seam does not move.
- **Completeness is load-bearing.** Anything `core` reads that is absent from the trace makes replay diverge, which is a second reason for the rule that `core` reaches nothing impure directly. Recording outbound calls without their results would break it, since a rejected injection changes what `core` does next.
- Writing to memory `core` was handed costs nothing beyond a memory write. There is no message passing in the interactive input path, so mouse motion rates cost nothing extra, and there is no backpressure question — which matters, because dropping events would break the completeness replay depends on.
- Because the buffer comes from the host, the simulated suite needs no shared memory at all: `host-sim` hands over plain process memory and the same `core` code exercises the same path.
- The watchdog copies a region rather than interpreting it, so [ADR-0008](0008-input-suppression-and-watchdog.md)'s smallness is preserved.
- **A trace of keystrokes is a keylog.** It holds whatever was typed in the retained window, passwords included. This is inherent rather than incidental — replaying a conversion bug needs the actual keys — so it is handled rather than mitigated: nothing is persisted or uploaded on its own, and a user is never asked to attach a trace without being told what is in it.
- **Not being file-backed is load-bearing for the same reason.** Mapping the region to a file would leave a keylog on disk permanently, and saving automatically on every failure would leave one after every failure.
- A retained trace is lost on reboot, or if the watchdog exits. That is the price of keeping keystrokes off disk, and it is the right way round.
- The watchdog can read keystrokes, which makes it a sensitive process. Its privileges, and who else can map the region, belong to the same security surface as [ADR-0004](0004-peer-authentication.md).
- Excluding key material from checkpoints has to be settled when the checkpoint serialization is written. A checkpoint carrying the private half of the pinned identity from [ADR-0004](0004-peer-authentication.md) would turn a trace file into a credential leak.
- The retained window varies with what the user is doing. Mouse motion arrives at a rate far above typing, so an event-count and byte budget alongside the time interval is what keeps the window from collapsing during mouse use.

## Alternatives considered

### Merging the two traces on wall clock

Not taken, and it is the most tempting mistake here. Two machines' clocks skew, and the skew can exceed the intervals being investigated — the gap between a key being captured and the converted event being injected. Sorting by wall clock can therefore present causally ordered events in reverse, with nothing to indicate it happened. Sequence numbers give an exact order for anything causally related and leave genuinely concurrent events unordered, which is the honest result.

### `core` owning the buffer

Not taken. Allocating a shared region is an OS operation, and `core` reaching for one directly is exactly what [ADR-0006](0006-host-boundary.md) rules out. Taking the buffer from the host also removes shared memory from the test path entirely.

### Streaming the trace to the watchdog

Not taken. It puts a message write in the interactive input path, and forces a choice between blocking the loop and dropping events — and dropping events breaks replay. It also moves the ring buffer's eviction into the watchdog, which is the one component that should stay trivial.

### Dumping to a file from a panic handler

Not taken as the mechanism. It covers panics and misses terminations and hangs, which is most of what the watchdog exists for. It can still run as a convenience for the panic path; it cannot be what the design relies on.

### Mapping the region to a file so the contents land on disk unattended

Rejected. It is the most direct route to the trace surviving anything, and it makes a permanent on-disk keylog the normal state of the system.

### A fixed one-minute checkpoint interval alone

Not taken. Under mouse motion a minute holds orders of magnitude more events than a minute of typing, so a fixed interval makes the retained history depend on the input type rather than on a budget.

### Evicting individual events from the front

Not taken. It eventually drops the checkpoint the remaining events depend on, leaving a suffix from which no state can be computed.

### Recording only outbound calls

Enough to see a symptom, not enough to replay one. Without the inbound events there is no input to feed `core`.

### Redacting key identity from the trace

Not taken. It removes exactly what a conversion bug needs, which is most of what traces are for. The privacy problem is addressed by controlling where traces go, not by making them useless.
