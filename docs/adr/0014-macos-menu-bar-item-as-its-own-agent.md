# ADR-0014: Turn converting off from a menu bar item that is its own binary and its own launchd job

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

The failure the supervisor does not cover is a favjit that is alive and converting
wrongly. It keeps heartbeating, so it is never killed ([ADR-0008](0008-input-suppression-and-watchdog.md)),
and the way to turn it off — `favjit --disable` — is a command typed on the keyboard
that is producing the wrong keys. What is missing is an escape that does not go
through the keyboard at all.

[ADR-0012](0012-macos-install-as-a-daemon-and-turn-off-with-a-file.md) settled what
"off" means: a file in the console user's own directory, writable without privilege,
which the converter watches. It deferred the menu bar item on the grounds that it
would be an application, an agent in the user's session and some way to talk to the
root daemon. The first two remain true; the third is not, because the file is that
way to talk to it.

The constraints are fixed by where each half can run. The converter needs root and
runs as a launchd daemon with no session — a menu bar item cannot live there,
because a status item exists only inside a login session's window server. A menu bar
item needs no privilege, because writing that file is all it does.

Whether the item is *drawn* is not something the application decides. Creating one
succeeds on a menu bar that has no room for it, and nothing in the API distinguishes
that from a normal creation; on a crowded bar it is simply not shown, and freeing
one place does not necessarily give it to the item that was waiting
([docs/platform/macos/menu-bar-status-items.md](../platform/macos/menu-bar-status-items.md)).

## Decision

A separate binary, `favjit-menu`, installed and registered by `favjit --install` as
a system LaunchAgent limited to `Aqua` sessions and bootstrapped into the console
user's GUI domain. It holds no state of its own: what "off" means is the control
file, through the same `host-macos` code the converter reads it with.

## Consequences

GUI dependencies — `tao` for the event loop, `tray-icon` for the status item — enter
the workspace, and only into this binary. `core` and the converter are unaffected,
which is what [ADR-0005](0005-crate-layout.md)'s layout is for. They also bring the
first dependency that asks the linker for a library outside the compiler's own
world, so the workspace pins Apple's `cc`.

Installing and uninstalling now cover two jobs in two domains: a daemon in `system`
and an agent in `gui/<uid>`. `--status` answers for both.

The item cannot be relied on. On a bar with no room it is not drawn, and the person
who most needs it is the one who cannot type — so the CLI stays the mechanism and
this stays the convenience. A run says in its log where the item landed, which is
the only thing that separates "created and not drawn" from "not created".

Two processes now write the control file. Neither owns it: both read it back to
decide what to show, so the state on screen is the state on disk rather than the
last thing either of them did.

## Alternatives considered

### A thread inside the converter

One process, one binary. **Not taken**: the converter is a root daemon with no login
session, and a status item created there has no window server to appear in. This is
not a preference about layout — it is why the split exists.

### A login item rather than a LaunchAgent

The conventional shape for a menu bar application: register the bundle to be opened
at login and let it stay up. **Not taken**: it is a second registration with a second
place to be switched off, and it brings nothing the agent does not have — the item
behaves the same either way, and the agent's `KeepAlive` also brings it back if it
dies mid-session, which is when an escape hatch would be missed.

### Double-clickable scripts for turning it off

Two files on the Desktop calling `favjit --disable` and `--enable`. **Not taken**:
an escape hatch that has to be found in the Finder, cannot say which state favjit is
in, and puts two more things in a place the person did not choose.
