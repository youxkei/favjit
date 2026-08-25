# ADR-0001: Use Rust as the implementation language

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

favjit needs two binaries — one capturing input on Windows, one capturing, converting, and injecting input on macOS — and both sit directly on top of privileged OS input APIs.

Constraints that drove the choice:

- **Latency is user-visible.** Mouse motion forwarded from Windows and keystrokes rewritten on macOS both sit in the interactive path. A stall is felt immediately.
- **The code is a long-lived privileged process.** It holds input-monitoring rights on macOS, so a memory-safety bug is a security bug, not just a crash.
- **Both platforms are reached through C APIs.** Whatever the language, there is an FFI boundary.
- **Distribution should be a plain binary per platform.** Requiring the user to install a runtime on both machines to fix their keyboard layout is not acceptable.

## Decision

Rust, for both the Windows and the macOS binary.

## Consequences

- FFI bindings are needed for the input APIs on both platforms. `unsafe` gets confined to a thin per-platform layer, with the conversion pipeline and transport written in safe Rust above it.
- Two build targets to cross-compile and release, with a shared core crate between them.
- No runtime to ship, no GC in the latency path.
- The platform layers cannot be exercised on CI hardware alone — capturing and injecting real input needs real machines. Testing strategy has to account for that.

## Alternatives considered

### Go

Pleasant for the network layer and for concurrency, which is a real part of this project. Not taken because it does not avoid the interop boundary — the platform input APIs are C APIs either way, so the ergonomic win is smaller than it looks — and because it puts a garbage-collected runtime in an interactive input path. Rust gives up nothing here that Go provides.

### C / C++

The most direct access to the platform APIs, and no FFI boundary at all. Not taken because a privileged, always-running process that touches every keystroke is exactly where memory-safety guarantees are worth the most. Dependency and build management across Windows and macOS is also markedly more work than Rust's toolchain gives for free.

### TypeScript / Node

Rejected outright: shipping a runtime to both machines, and running an interactive input path on a language with no story for low-level input capture.
