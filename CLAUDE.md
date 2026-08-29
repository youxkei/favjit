# favjit

Forwards Windows keyboard and mouse input to macOS, and applies keyboard layout
conversion to the key events macOS receives.

Two machines, one Windows and one macOS, and input flows one way between them.
There is exactly one conversion pipeline and it lives on the macOS side: input
forwarded from Windows and input from the Mac's own keyboards, built-in and
Bluetooth, both pass through it. Linux, a third machine and the reverse direction
are out of scope, so a design that assumes any of them is a design for something
else. The layout it converts to was developed in a Karabiner-Elements configuration
and ported from there; that configuration is where its history lives, and the port is
not a place to rediscover it.

**This file describes directories, not files.** What an ADR decides is written in
that ADR: a summary here would be a second copy to keep in step, and citing an ADR
from this file is the sign of one.

- [crates/](crates/) — the cargo workspace: `core`, a host per platform, the
  binaries, and the end-to-end suite.

  ```
  cargo test --workspace
  cargo clippy --workspace --all-targets
  cargo fmt --all
  ```

  Those are run on the Mac: `bin-macos` names its host unconditionally, so the
  workspace as a whole builds only there. Each platform's host and binary are compiled
  away on the other platform, so they cover everything but the half belonging to the
  machine they are not run on; that half is checked by naming its crates:

  ```
  cargo test --target <target> -p favjit-host-windows -p favjit-bin-windows -p favjit-bin-watchdog
  ```

- [docs/adr/](docs/adr/) — the design decisions, one per file, with the reasoning
  that led to each. Its README describes the process; read it before writing one.
  Don't restate that reasoning at length in code comments — cite the ADR instead.
- [docs/platform/](docs/platform/) — what the OS input APIs actually do, one
  directory per platform. Established on real hardware and recorded with the OS
  version it was observed on, never asserted from memory.

## One thing to hold onto

**Don't change `favjit`'s default away from the dry run.** On either machine, a bare
command reads the keyboards and changes nothing outside its own process: the Mac
converts and injects nothing, and the Windows side reads and sends nothing.
`--dry-run false` is the run that takes this machine's keyboards, and asking for it
is the point. The failure the default avoids lands on the keyboard the person is
typing on, and it is not subtle.

**One flag decides the mode, so there is no combination to get wrong.** Taking the
keyboards and delivering are one thing on both machines: neither taking them without
delivering, which takes the keyboard away, nor delivering without taking them, which
types every key twice, is expressible. `crates/e2e/tests/asking_for_nothing.rs` is
the record of that for the sink, and it fails to compile if it stops being true.
