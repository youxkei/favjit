# ADR-0002: Forward input one way, from Windows to macOS

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

Two machines sit on the desk: a Windows machine and a Mac. The Windows machine holds the keyboard and mouse that are actually used; the Mac is operated alongside it.

Existing tools in this space are typically symmetric — any machine can be the source, and the pointer crossing a screen edge decides who is in control. That symmetry costs something: both machines need capture *and* injection, both need the arbitration logic, and both need to agree on who currently owns the input.

The actual need here is asymmetric. Only one set of physical input devices matters, and it is on the Windows side.

## Decision

Input flows one way: Windows captures, macOS injects. macOS never sends input events to Windows.

## Consequences

- The Windows binary needs capture only; the macOS binary needs capture (for its own local keyboards, see [ADR-0003](0003-unified-conversion-pipeline.md)) and injection. Neither needs the other half.
- No ownership arbitration protocol between the machines. There is still a local question of whether Windows input is currently going to macOS or staying on Windows, but that is one machine's state, not a negotiation.
- Reversing this later is not a small change — it would mean adding injection to Windows and a real arbitration protocol. Accepted knowingly: the asymmetry is a property of the desk, not a temporary simplification.
- Being sink-only costs the Mac no capability it would not need anyway: it has to capture its own keyboards for local conversion regardless of whether anything is forwarded to it.
- Because control over the input can move between the two machines, a modifier held down at the moment control moves can be left stuck on the sink. The sink is responsible for releasing held keys when it stops receiving forwarded input; it cannot rely on ever seeing the matching key-up.

## Alternatives considered

### Symmetric, bidirectional forwarding

The conventional shape for cross-machine input sharing. Not taken because it doubles the platform surface and adds an arbitration protocol to serve a case that does not exist here: there is no second keyboard worth driving Windows from.

### macOS as the source, Windows as the sink

Rejected on the facts — the input devices in use are attached to the Windows machine.
