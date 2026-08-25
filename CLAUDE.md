# favjit

Forwards Windows keyboard and mouse input to macOS, and applies keyboard layout conversion to the key events macOS receives. See [README.md](README.md) for the full picture.

## Assumptions to hold onto

- **Input flows one way: Windows → macOS.** The reverse direction is deliberately out of scope, so don't propose designs that assume bidirectional sync.
- **There is exactly one conversion pipeline, and it lives on the macOS side.** Both forwarded input and input from the Mac's local keyboards (built-in and Bluetooth) pass through it.
- Only two machines: one Windows, one macOS. Linux and 3+ machine setups are out of scope.
- **Every impure operation goes behind the per-platform `host` facade** — OS input, transport, clock, and the supply of events ([ADR-0006](docs/adr/0006-host-boundary.md)). `core` never touches `std::thread`, `std::time`, or the network. Hosts stay thin: whatever lives inside one is reachable only by real-hardware testing.
- **Input suppression never outlives the ability to process input** ([ADR-0008](docs/adr/0008-input-suppression-and-watchdog.md)). A failure must degrade to "favjit stopped working", never "the keyboard stopped working" — this points the opposite way from the authentication default, deliberately.
- **`core` holds no concurrency.** Each role is a single loop over one event stream ([ADR-0006](docs/adr/0006-host-boundary.md)). This is what makes the end-to-end suite deterministic and traces replayable — don't propose spawning threads inside `core`.
- **The sink refuses input from any peer it has not been explicitly paired with, and fails closed** ([ADR-0004](docs/adr/0004-peer-authentication.md)). There is no "trust the local network" mode to fall back on when something doesn't connect.

## Repository state

There is no implementation code yet — only design documents, so **there are no build, test, or lint commands.**

The implementation language is Rust ([ADR-0001](docs/adr/0001-use-rust.md)) and the workspace layout is settled ([ADR-0005](docs/adr/0005-crate-layout.md)).

## Documentation conventions

Design judgments go in [docs/adr/](docs/adr/) as ADRs. The process is described in [docs/adr/README.md](docs/adr/README.md).

Don't restate the reasoning at length in code comments — link to the ADR instead.

## Platform notes

Platform-specific behavior — what the OS input APIs actually do, which permissions are involved, what has and has not been confirmed on hardware — lives under [docs/platform/](docs/platform/), one directory per platform.

Don't assert this kind of behavior from memory. Verify it on real hardware, then record it there along with the OS version it was observed on. A decision that follows from a finding goes in an ADR; the finding itself goes in the platform doc.
