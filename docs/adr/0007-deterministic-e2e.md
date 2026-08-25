# ADR-0007: Run the end-to-end suite against a simulated host that owns the clock and event ordering

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

The question the end-to-end suite exists to answer is: press this on the Windows side, what reaches applications on the macOS side? Answering it needs both platforms in one process, exact assertions, and the ability to produce situations that real hardware will not produce on request:

- The link dropping between a key-down being forwarded and its key-up — the stuck-modifier failure that [ADR-0002](0002-input-topology.md) makes the sink responsible for
- A hold threshold crossed at exactly its boundary
- The clock jumping over a suspend
- Input permission revoked mid-run
- Packets arriving out of order, or a reconnection storm

Everything impure is already behind the host boundary, and `core` holds no concurrency of its own ([ADR-0006](0006-host-boundary.md)), so a substitute host is enough to place the whole system in any of those states.

## Decision

The suite runs against `host-sim`, which owns the simulated clock and the order things arrive in on both machines.

Each whole run is driven in turn, on the test's own thread: a source's run to its end, then what it sent handed to a sink's run. The watchdog's run is driven the same way, against a machine scripted to answer its probes or to stop answering them ([ADR-0008](0008-input-suppression-and-watchdog.md)). Nothing runs concurrently, so there is no quiescent point to wait for and no ordering to seed.

A situation is produced by scripting the machine, not by racing something against it: a key held across a threshold, a clock that jumps, a machine that answers differently after the second event.

Real threads under a real scheduler belong to the real-hardware tier, not here.

## Consequences

- No real time anywhere in the suite. Timing scenarios are exact and instant, including the ones that would otherwise need a three-hour wait.
- Every case is a sequence of calls with no waiting, so a failure is the same failure every time and needs nothing recorded to reproduce it.
- What crosses between the machines is visible in the test, because the messages a source sent are handed over by name — which is also the assertion that they are the only thing that crosses.
- Both ends share one clock, so a timeline that spans the two machines is meaningful. Separate clocks would make a shared "at 50ms" meaningless.
- The simulator's surface keeps three concerns apart: the inbound script (what the outside world does), the outbound faults (what our requests return), and the record of outbound calls (what we asked for). The third is what most assertions read.
- Records of outbound calls carry the simulated timestamp, because for a layout converter the timing of an injected event is part of the expected result, not incidental to it.
- Interleaving between the two roles is not explored. Each observes the other only through the messages that cross, so a case that needed an interleaving would be a case this decision has to be revisited for.
- **The simulator's fidelity is bounded by what has been established on real hardware.** A simulated behavior that mirrors a platform quirk cites the finding under [docs/platform/](../platform/) that it came from. Anything else is a simplification and is recorded as one. Without that discipline a green suite means only that the code agrees with our guesses, and the suite cannot detect the disagreement because the seam is inside the host.

## Alternatives considered

### A thread per role, serialized at `next_event()`, with ordering chosen by a seed

Reproducible, and able to explore orderings a real scheduler would not reach. Not taken: it is machinery for interleavings the design does not produce, and the cost is a scheduler, a quiescent point every assertion has to wait for, and a seed in every failure report — inside the crate the suite exists to keep simple. Driving whole runs in turn gives the same signal with none of it.

### Real threads under the real scheduler

A flaky failure under real threads is a real bug, and treating it as noise would be the worse mistake — so the signal is genuine. Not taken anyway, for three reasons. Such a failure is not reproducible, which is what turns a real race into a retry habit. A real scheduler samples interleavings narrowly, and the dangerous ones are precisely the rare ones it will not produce. And without a defined quiescent point every assertion needs a timeout, which adds failures that are indistinguishable from real hangs.

### A declarative script written up front

Reads like a specification, and cannot express reaction. Pairing and reconnection are inherently reactive — respond to what was sent — so they would fall outside the model, and a bespoke schedule language would have to be maintained for the cases that fit.

### A general deterministic scheduler over thread preemption points

Unnecessary. With no concurrency inside `core` there are no preemption points to schedule; an ordered event queue covers everything that remains.
