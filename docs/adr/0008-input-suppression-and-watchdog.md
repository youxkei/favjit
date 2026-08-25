# ADR-0008: Input suppression never outlives the ability to process input, and a watchdog process enforces it

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

Both roles suppress input at the OS level. The source suppresses locally while input is directed at the Mac, and the sink captures the Mac's own keyboards in order to convert them.

Suppression is the one capability in this project that can take a machine away from the person using it. A process that holds it and stops processing leaves a keyboard that does nothing — and the recovery anyone would reach for, opening a terminal and killing the process, needs a keyboard.

The failures that produce that state are ordinary ones: a panic in the conversion pipeline, a deadlock, a host call that never returns, an out-of-memory kill.

The safe behavior here points the opposite way from [ADR-0004](0004-peer-authentication.md), and for the same reason. Both protect the user's control of their own machine; what threatens it differs. There the danger is someone else injecting input, so the answer is to refuse. Here the danger is the user being locked out, so the answer is to let go.

Letting go is not something a failed process can be relied on to do. A crash runs no code, and a hung process is alive and still holding suppression. Nothing inside a wedged process can be trusted to notice — and liveness cannot be measured by a heartbeat from a separate thread, because [ADR-0006](0006-host-boundary.md) makes each role a single loop, and a wedged loop coexists comfortably with a heartbeat thread reporting that all is well.

## Decision

Suppression never outlives the ability to process input. On any failure — panic, hang, termination — suppression ends and keystrokes reach the machine they were typed on.

A separate watchdog process supervises each role and enforces this. Liveness is the event loop itself: the loop reports each return to `next_event()`, and the watchdog can inject a probe event through the host and require it to come back. When reports stop arriving within their bound, the watchdog terminates the process.

The watchdog does nothing else. Being small enough to trust is the whole of its value.

**Its judgement is `core`'s, and it reaches the machine through a host of its own.** When a probe is due, how long a silence may last, that a silence longer than the bound ends the process, and that a process which exits on its own is not a failure are `core::watchdog`'s; `WatchdogHost` carries the calls — start the process, wait for the next beat, put a probe in, end the process, keep what the run recorded. Both machines suppress, so both need supervising, and the judgement is the part that must not be wrong: one copy the end-to-end suite drives ([ADR-0007](0007-deterministic-e2e.md)) beats one copy per platform that nothing reaches. What is left in `bin-watchdog` is the platform half, one module per machine.

The probe path is a distinct type at the host surface, not an input event, so nothing arriving on it can become an injected keystroke. Access to it is restricted to the supervising watchdog.

## Consequences

- A failure degrades to "favjit stopped working", never "the keyboard stopped working". It is loud, and recoverable with the tools already in front of the user.
- Suppression may not be held across an operation that can block indefinitely, which is a second reason for [ADR-0006](0006-host-boundary.md)'s requirement that outbound host operations never block.
- Where the platform ends a process's suppression when that process dies, that covers the paths where the process is gone; where it does not, releasing it is part of the implementation. The requirement does not change either way.
- **The injection mechanism must not leave keys held down when the process ends.** Releasing mid-keypress otherwise strands a modifier inside applications, and the paths that matter most — a kill, a crash — have no code left to emit the matching key-ups. This is a constraint on which injection mechanism is chosen, the same shape as the constraint on suppression itself, rather than work handed to the watchdog.
- Probe-and-ack exercises the real path rather than a side channel, so a loop that is spinning but no longer delivering is caught, not just one that has stopped entirely.
- Probes are part of the host surface rather than a test-only affordance.
- Terminating the process is the entire mechanism, which works because suppression does not outlive the process.
- Two processes per machine to install, launch, and keep running — hence `bin-watchdog` in [ADR-0005](0005-crate-layout.md).
- `bin-watchdog` depends on `core` with the `watchdog` feature alone: the judgement and the clock, and no conversion pipeline, no link, no pairing. What the component that must not fail contains is bounded by that gate rather than by an absent dependency.
- What is left per platform is calls, which is the part no suite reaches either way — so a second machine costs the calls and not the judgement.
- The trace ([ADR-0009](0009-trace-and-replay.md)) is one of those calls, since the memory it lives in is the platform's. A machine with no trace answers by doing nothing, and says so.
- Two ways to end a process, because the platforms do not agree on whether there is a way to ask before insisting. Which one a machine does is the host's; that the process is ended is `core`'s.
- The heartbeat is read on a thread of the watchdog's own, so that the bounded wait needs no per-platform polling. A reader thread that dies stops the heartbeats and the process is killed, which errs towards giving the keyboard back — the direction this decision requires.
- The watchdog's own failure is the gap that remains. It stays trivial so that the gap stays narrow; every responsibility added to it widens the gap.
- A bound on loop iteration time becomes a real constraint on the pipeline. Work that can exceed it has to be divided or moved off the loop.

## Alternatives considered

### Keep suppressing, and let the user recover by other means

Log in over the network, or force-quit with the mouse. Not taken: it assumes a second working input path and unhurried troubleshooting at the exact moment the machine looks broken. A tool that can render a laptop unusable while it is away from a network is not one worth shipping.

### Ask before releasing

Not taken. A prompt has to be answered, and the input needed to answer it is the thing that is unavailable.

### Have the watchdog emit the key-ups itself

Not taken. It would give the watchdog the ability to inject input, which is the largest capability that could be added to the one component whose value is that it does almost nothing. Constraining the injection mechanism achieves the same outcome without growing it.

### A watchdog per platform, each with its own judgement

Reach the OS directly in each one, with no boundary in between. Not taken: what must not be wrong is the judgement, and this is the shape that has one of it per machine, neither reachable from the suite. The saving is a trait; the cost is that the bound, the probe rate and the kill are agreed by two implementations reading each other.

### One portable channel: the supervised process's own stdin and stdout

`Stdio::piped()` is std on every platform, so the channel would need no platform half at all. Not taken: favjit's stdout is what a mode exists to produce — the device list, the machine's key, a run's report — and any of it would read as a heartbeat. A liveness channel anything else can write to is one that vouches for a wedged process.

### A loopback socket instead of inherited pipes

Portable through std, with no descriptor or handle to hand down. Not taken: it is reachable by every other process on the machine, so answering heartbeats on behalf of a wedged favjit becomes something a local process can do. The probe path is restricted to the supervising watchdog, and a port with a token to check is more machinery inside the watchdog than a pipe the parent already holds.

### Supervise from inside the process

A monitor thread that resets or kills the loop. Not taken: it shares the fate of whatever wedged, and the failures that matter most — a deadlock involving the runtime, a host call that never returns — are exactly the ones capable of taking it down too.

### Rely on the service manager

Not taken. A service manager notices a process that exited. The case that matters is a process that is running and has stopped working, which looks healthy from the outside.

### Give the watchdog more to do

Owning conversion state, or the transport, or the pairing. Not taken: each responsibility is another way for the watchdog to fail, and the watchdog is the component that must not.
