# ADR-0005: Lay the workspace out by platform for hosts and binaries and by role for features, and put the boundary itself in a crate no host can see past

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
crates/host  hid  noise  link-wire  pairing-exchange  engine  host-windows  host-macos  host-sim  bin-windows  bin-macos  bin-watchdog  e2e
```

Roles are cargo features on `engine` — `source` and `sink` — gating role logic and nothing else. `bin-windows` enables `source`, `bin-macos` enables `sink`, and both produce a binary named `favjit`.

**`hid`, `noise`, `link-wire` and `pairing-exchange` hold the arithmetic the two machines' `engine`s have to agree on with each other, not with a host.** A key's usage number, a HID report's byte layout, the Noise construction, and the frame the source relays over the link are not this machine's decision the way a device filter or a held-key policy is — they are settled once, the same on both sides, the way the pairing exchange already was. `engine` depends on each and re-exports what it needs, so nothing about `engine`'s own public API moves. `host-sim` depends on the same crates directly, because standing in for the machine on the other end of a socket or a keyboard means running its arithmetic for real — the same reason `host-sim` already depended on `pairing-exchange` — and it still depends on `engine` in neither direction. A platform host reaches none of them: a page and a usage stay numbers IOKit or raw input handed over, because `host-macos` and `host-windows` have no more use for the table that names them than they ever did.

**`host` states the traits each role needs of a machine, and the types their signatures are written in.** It holds nothing else — no functions, no methods, no constant a call could be made against. `engine` depends on it and so does every host crate, and no dependency runs between `engine` and a host crate in either direction: the concrete host is constructed by whatever owns `main` — a binary or the end-to-end harness — and handed to the role loop as `&mut dyn`, so neither names the other.

**A host crate cannot reach `engine`, because it does not depend on it.** That is what `host` is for. [ADR-0006](0006-host-boundary.md)'s rule — an operation is one platform call and never a decision — is a rule about what may go inside a function body, and a rule of that shape is kept by whoever is reading. A host that borrows a name from `engine` to compare, to filter, to assemble reads as a convenience at the call site and as an untestable decision from the suite, and nothing distinguishes the two readings in review. Removing the dependency is the only form of the rule a compiler checks.

**What `host` holds is data, so everything computed over it is `engine`'s.** Naming a HID usage or a scancode, deciding whether a device matches, assembling a report out of the element values that describe it, working out which modifiers are down: each is a function in `engine`, over types defined in `host`, reached from the entry point the run came in through. Rust puts an `impl` in the crate that defines the type, so a method on this data would be logic in the crate that must hold none.

**Nothing between `engine` and a platform host holds behaviour.** Each host wraps its own platform's calls, against constants and formats it neither states nor reads — it is handed what a call needs — because what two hosts would otherwise share belongs in `engine` where the end-to-end suite reaches it ([ADR-0006](0006-host-boundary.md)).

**What `engine` may depend on is any crate widely enough used to be worth trusting, that brings no runtime and reaches no machine.** The line it must not cross is [ADR-0006](0006-host-boundary.md)'s — the OS, the clock, the network, a thread — and a crate is on the wrong side of that only if it does one of those. Arithmetic over bytes is not: a construction both machines have to agree on belongs here whatever library performs it, since the alternative is the same agreement written once per platform, where the two copies fail as a value that will not open rather than as an error.

Which host a binary links is therefore its own dependency table: `bin-macos` depends on `host-macos` and on nothing else that reaches an OS. Nothing in the workspace depends on two hosts, so no feature unification can pull a second one in.

`bin-watchdog` produces the supervising process of [ADR-0008](0008-input-suppression-and-watchdog.md), and its binary is named `favjit-watchdog`. It depends on `host` and on `engine` with the `watchdog` feature alone, and on no host crate: what it needs of a machine is few enough calls to live in the crate itself, one module per platform.

`e2e` declares its simulator-enabled dependencies under `[dev-dependencies]` only, and keeps its harness in `tests/` rather than in a lib target.

## Consequences

- Each shipped binary links only its own platform's host, verified for both debug and release workspace builds.
- One definition of the boundary, in a crate that is nothing but the boundary. A host cannot disagree with it, because it compiles against it, and cannot step past it, because it has nothing else to compile against. What is paid for the `dyn` is an indirect call per operation, at the boundary where the process is about to enter the OS anyway.
- **One more hop to find out what a `HostEvent` is.** The types `engine`'s loops are written in are not in `engine`, so reading either role means following the traits out to `host` and back. That is the price, and what it buys is that the same hop is not available to a host in the other direction.
- **The check is a line that is not in a file.** A host crate's `Cargo.toml` names `favjit-host` and no `favjit-engine`, and that absence is the whole enforcement — `cargo tree -p favjit-host-macos` not reaching `engine` says every decision in that crate was written there rather than borrowed, without anybody reading a function body to find out.
- **What keeps the simulator out of a shipped binary is that binary's own dependency table.** `bin-macos` names `host`, `engine` and `host-macos` and nothing else, so no build can link `host-sim` into `favjit`. The `[dev-dependencies]` placement in `e2e` says something narrower — that nothing in that crate ships, checked by `cargo tree -p favjit-e2e -e normal` staying empty — and it does not decide what `favjit` contains: `host-sim` is a member, so `cargo build --workspace` builds it and unifies `engine`'s role features into the one `engine` every member links, wherever `e2e` declares its own dependencies. Measured with those two lines in either table: the `favjit` binary is byte-identical, with no `host-sim` and no `engine::source` in it.
- `cargo test --workspace` does unify the simulator into the binaries' test builds. Nothing is lost to it: `engine` is pure, `bin-*` is a thin shell, and the real host implementations can only be exercised on real hardware regardless.
- Naming the binaries after platforms survives a move to bidirectional forwarding — each machine would run one binary with both role features enabled. Role-named binaries would have to be renamed or merged, because a binary would no longer correspond to one role.
- The two roles share the wire format, the HID vocabulary, the Noise construction and the pairing machinery as `hid`, `noise`, `link-wire` and `pairing-exchange` — crates `engine` depends on and re-exports from, not modules with no existence outside it. What makes each one worth its own crate rather than a module is the same reader `host-sim` is: a module inside `engine` is a module `host-sim` cannot depend on without depending on the whole of `engine`.
- `bin-watchdog` holds its platform halves itself rather than reaching them through a host crate, which is the one place the layout above is departed from. A host crate would bring that platform's whole surface — the capture, the injection, the link — into the component that must not fail, where what it needs is a pipe and a way to end a process. What it can be trusted to do is bounded instead by `engine`'s `watchdog` feature, which is the judgement and the clock and nothing else.
- The boundary between the pure part of `engine` and the part that drives a host is a module discipline rather than a crate boundary. A check that the pure modules do not reference `std::thread`, `std::time`, or `std::net` keeps it honest.

## Alternatives considered

### Two binaries under `src/bin/` in one package

Not possible. One package means one dependency table, so each binary would link both platforms' hosts.

### Separate `source` and `sink` crates

Keeps the role separation in the dependency graph rather than in features. Not taken: it forces every type the two roles share into `engine`'s public API, and adds two crates for what is a loop plus a host handle.

### A role feature on `engine` pulling in that platform's host as an optional dependency

The obvious way to make the role choose the host, and it cannot hold. `engine` and every host have to agree on one vocabulary — the event type, the key and modifier types, the shape of an injected event — and that vocabulary is what the boundary *is*. Cargo has no cyclic dependencies, optional ones included, so the vocabulary lives in whichever crate the other one depends on, and `engine` depending on the hosts puts it in a host: three copies selected by `cfg`, or one host crate that every other one depends on and that therefore holds the boundary while also holding a platform.

### A host crate shared by both platforms, between `engine` and each platform's host

A crate holding *behaviour* the two platforms share, which is what sets this apart from `host`. The two hosts do the same things to different APIs, and the wire format is one format: a crate above them would hold the parts neither platform decides, and remove what each would otherwise restate. Not taken. Whatever it held would be as far out of the suite's reach as a platform host is ([ADR-0006](0006-host-boundary.md)), and what belongs there belongs in `engine`, where the suite drives it. With that in `engine` there is nothing left for such a crate: what would remain is a wrapper over one library, written twice at a few lines each, against constants neither copy states.

This is not `hid`, `noise`, `link-wire` or `pairing-exchange`'s situation, and not `host-sim`'s either. Those crates sit beside `engine`, not between it and a platform host — a platform host still reaches none of them. What they hold is not a platform host's behaviour shared across platforms; it is the one arithmetic two *engines*, on two machines, already have to agree on, which `host-sim` also has to run for real to stand in for the machine on the other end of it.

### The vocabulary and the traits in `engine`, with each host depending on it

One crate fewer, and the definition of the boundary in the crate whose boundary it is, so a reader finds out what a `HostEvent` is where the loop that reads one lives. What it cannot do is stop a host from calling `engine`. Every name `engine` exports is in scope inside a host operation once that dependency exists: the table that turns a usage into a key, the arithmetic over a report, the comparison a device filter is made of. Each of those is one honest-looking line at the call site, and each puts a decision where [ADR-0007](0007-deterministic-e2e.md)'s suite cannot drive it. Not taken: it leaves [ADR-0006](0006-host-boundary.md)'s central rule to review, and a rule that has to be reread at every call site is one a compiler could have been holding instead.

### `host` holding the operations over its own types as well

The obvious home for them, and the only one both sides can call: a method reads better beside the type it belongs to than as a function somewhere else, and `host` is what `engine` and the hosts share. Not taken, and it is the failure this layout exists to prevent — a host crate depends on `host`, so a method there is reachable from inside a host operation exactly as one in `engine` would be, and the arrow `host` exists to do without buys nothing. `host` holds definitions because anything else in it is logic in a crate no entry point runs, which is the same place out of the suite's reach by another name.

### Duplicating the vocabulary in each host crate, selected by `cfg`

Not taken: three structurally identical definitions of the event type, kept in step by hand, with a divergence surfacing only in whichever crate happens to be compiled. The end-to-end suite could not detect it, because it only ever compiles one of the three.

### `e2e` in its own workspace

Makes feature unification between the harness and the binaries structurally impossible rather than merely bounded. Not taken: the unification that actually happens — `engine` built with both role features, because `host-sim` is a member that asks for them — leaves a shipped binary byte-identical, and a separate workspace costs a second target directory, a second lock file and a separate test command for a hazard that has nothing to bite.

### Role-named binaries (`bin-source` / `bin-sink`)

Not taken. Correct only while platform and role are one-to-one, and the naming would have to change the moment that stops being true.
