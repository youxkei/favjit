# ADR-0005: Lay the workspace out by platform for hosts and binaries, and by role for features

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

Rust ([ADR-0001](0001-use-rust.md)) with two binaries for two platforms ([ADR-0002](0002-input-topology.md)). Three requirements pull on the layout:

- The valuable end-to-end test spans both machines — press a key on the Windows side, assert what is injected on the macOS side — and has to run in one process on whichever machine a developer is sitting at.
- Each shipped binary must link only its own platform's impure layer, so a Mac's frameworks are not a dependency of the Windows build.
- The two roles share the wire format and the pairing state machine intimately.

A package in Cargo has one dependency table and one feature set, so per-platform binaries cannot be two files under `src/bin/` — both would link both platforms.

Two toolchain behaviors were established by experiment under `resolver = "2"`, because the layout rests on them:

- Features requested on a shared dependency by one workspace member are unified into other members built in the same invocation. A member that asked for none of them was built with one enabled by `cargo build --workspace`.
- That unification does not happen for `[dev-dependencies]` when tests are not being built. With the same request moved there, `cargo build --workspace` and `cargo build --release --workspace` both produced the binary with the feature off.

## Decision

One workspace:

```
crates/core  host-windows  host-macos  host-sim  bin-windows  bin-macos  bin-watchdog  e2e
```

Roles are cargo features on `core` — `source` and `sink` — gating role logic and nothing else. `bin-windows` enables `source`, `bin-macos` enables `sink`, and both produce a binary named `favjit`.

**The dependency runs from each host crate to `core`, never the other way.** `core` states the vocabulary and the traits a role needs of a machine; each host implements them. The concrete host is constructed by whatever owns `main` — a binary or the end-to-end harness — and handed to the role loop as `&mut dyn`, so `core` names no host crate.

**Nothing sits between `core` and a platform host.** Each host wraps its own platform's calls, against constants and formats it does not restate, because what two hosts would otherwise share belongs in `core` where the end-to-end suite reaches it ([ADR-0006](0006-host-boundary.md)).

**What `core` may depend on is any crate widely enough used to be worth trusting, that brings no runtime and reaches no machine.** The line it must not cross is [ADR-0006](0006-host-boundary.md)'s — the OS, the clock, the network, a thread — and a crate is on the wrong side of that only if it does one of those. Arithmetic over bytes is not: a construction both machines have to agree on belongs here whatever library performs it, since the alternative is the same agreement written once per platform, where the two copies fail as a value that will not open rather than as an error.

Which host a binary links is therefore its own dependency table: `bin-macos` depends on `host-macos` and on nothing else that reaches an OS. Nothing in the workspace depends on two hosts, so no feature unification can pull a second one in.

`bin-watchdog` produces the supervising process of [ADR-0008](0008-input-suppression-and-watchdog.md), and its binary is named `favjit-watchdog`. It depends on `core` with the `watchdog` feature alone and on no host crate: what it needs of a machine is few enough calls to live in the crate itself, one module per platform.

`e2e` declares its simulator-enabled dependencies under `[dev-dependencies]` only, and keeps its harness in `tests/` rather than in a lib target.

## Consequences

- Each shipped binary links only its own platform's host, verified for both debug and release workspace builds.
- One definition of the boundary, in the crate the boundary exists to protect. A host cannot disagree with it, because it compiles against it. What is paid for the `dyn` is an indirect call per operation, at the boundary where the process is about to enter the OS anyway.
- **What keeps the simulator out of a shipped binary is that binary's own dependency table.** `bin-macos` names `core` and `host-macos` and nothing else, so no build can link `host-sim` into `favjit`. The `[dev-dependencies]` placement in `e2e` says something narrower — that nothing in that crate ships, checked by `cargo tree -p favjit-e2e -e normal` staying empty — and it does not decide what `favjit` contains: `host-sim` is a member, so `cargo build --workspace` builds it and unifies `core`'s role features into the one `core` every member links, wherever `e2e` declares its own dependencies. Measured with those two lines in either table: the `favjit` binary is byte-identical, with no `host-sim` and no `core::source` in it.
- `cargo test --workspace` does unify the simulator into the binaries' test builds. Nothing is lost to it: `core` is pure, `bin-*` is a thin shell, and the real host implementations can only be exercised on real hardware regardless.
- Naming the binaries after platforms survives a move to bidirectional forwarding — each machine would run one binary with both role features enabled. Role-named binaries would have to be renamed or merged, because a binary would no longer correspond to one role.
- The two roles share the wire format and pairing machinery as modules inside `core`, not across a crate boundary.
- `bin-watchdog` holds its platform halves itself rather than reaching them through a host crate, which is the one place the layout above is departed from. A host crate would bring that platform's whole surface — the capture, the injection, the link — into the component that must not fail, where what it needs is a pipe and a way to end a process. What it can be trusted to do is bounded instead by `core`'s `watchdog` feature, which is the judgement and the clock and nothing else.
- The boundary between the pure part of `core` and the part that drives a host is a module discipline rather than a crate boundary. A check that the pure modules do not reference `std::thread`, `std::time`, or `std::net` keeps it honest.

## Alternatives considered

### Two binaries under `src/bin/` in one package

Not possible. One package means one dependency table, so each binary would link both platforms' hosts.

### Separate `source` and `sink` crates

Keeps the role separation in the dependency graph rather than in features. Not taken: it forces every type the two roles share into `core`'s public API, and adds two crates for what is a loop plus a host handle.

### A role feature on `core` pulling in that platform's host as an optional dependency

The obvious way to make the role choose the host, and it cannot hold. `core` and every host have to agree on one vocabulary — the event type, the key and modifier types, the shape of an injected event — and that vocabulary is what the boundary *is*. Cargo has no cyclic dependencies, optional ones included, so either `core` depends on the hosts and the vocabulary lives outside `core`, or the hosts depend on `core` and it lives inside. Only the second leaves the definition of `core`'s boundary in `core`.

### A host crate shared by both platforms, between `core` and each platform's host

The two hosts do the same things to different APIs, and the wire format is one format: a crate above them would hold the parts neither platform decides, and remove what each would otherwise restate. Not taken. Whatever it held would be as far out of the suite's reach as a platform host is ([ADR-0006](0006-host-boundary.md)), and what belongs there belongs in `core`, where the suite drives it. With that in `core` there is nothing left for such a crate: what would remain is a wrapper over one library, written twice at a few lines each, against constants neither copy states.

### A separate crate holding the vocabulary, with `core` and the hosts both depending on it

Breaks the cycle properly. Not taken: the crate's entire content would be the types `core` is written in, so the definition of `core`'s boundary would live outside `core` — and every reader would have to follow one more hop to find out what a `HostEvent` is. It buys a dependency arrow nothing needs.

### Duplicating the vocabulary in each host crate, selected by `cfg`

Not taken: three structurally identical definitions of the event type, kept in step by hand, with a divergence surfacing only in whichever crate happens to be compiled. The end-to-end suite could not detect it, because it only ever compiles one of the three.

### `e2e` in its own workspace

Makes feature unification between the harness and the binaries structurally impossible rather than merely bounded. Not taken: the unification that actually happens — `core` built with both role features, because `host-sim` is a member that asks for them — leaves a shipped binary byte-identical, and a separate workspace costs a second target directory, a second lock file and a separate test command for a hazard that has nothing to bite.

### Role-named binaries (`bin-source` / `bin-sink`)

Not taken. Correct only while platform and role are one-to-one, and the naming would have to change the moment that stops being true.
