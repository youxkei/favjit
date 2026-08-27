# ADR-0015: Install into an ad hoc signed application bundle and ask for Accessibility in the user's session

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

favjit cannot read a keyboard without being granted input monitoring. Root is not
enough: as a LaunchDaemon every keyboard refused to open with `kIOReturnNotPermitted`
while the virtual output device opened normally
([docs/platform/macos/input-permissions.md](../platform/macos/input-permissions.md)).

The grant is recorded against a code identity, and the converter cannot ask for it
itself. A request from a session-less daemon prompts nobody and leaves nothing in the
settings list, so there is not even a switch to turn on. A bare binary has no
identity a person can be asked about.

What the platform notes establish: a bundle opened as an application in the login
session can ask for Accessibility, which puts a dialog on screen and an entry in the
list — and granting Accessibility makes the input monitoring answer `Granted` as
well. An ad hoc signature is enough to be asked about, which matters because no
Developer ID is available to sign with.

## Decision

`favjit --install` lays the three binaries out in `/Applications/favjit.app`, signs
the bundle ad hoc under one identifier, points both launchd jobs at the binaries
inside it, and then opens the bundle as an application in the console user's session
to ask for Accessibility.

The converter decides whether it may run by asking IOKit — not the window server —
and ends the run when the answer is no, leaving the supervisor's restart to ask again
until it changes.

## Consequences

One identity covers everything favjit does, so one grant covers the converter, the
watchdog and the menu bar item. `--uninstall` resets it, leaving nothing switched on
for software that is gone.

**Every update needs the permission granted again.** An ad hoc signature ties the
identity to the binary's hash, so a rebuilt favjit inherits a refusal rather than a
question: the switch still reads as on while the answer is `Denied`, and no dialog
appears. The install clears the record and asks again, which turns an update into one
dialog. A Developer ID signature would match by identifier and team and would not
have this cost.

Granting is picked up without anything else being done: the converter exits when it
cannot read the keyboards and launchd starts it again about ten seconds later, so
answering the dialog is enough. The cost is a restart loop while the answer is no,
which the log states on every pass.

The window server's answer — `CGPreflightListenEventAccess` — is logged and not
acted on. It is always `false` for a daemon, and it turned `true` once Accessibility
was granted, so it says something about the grant but nothing about whether the
keyboards can be opened.

## Alternatives considered

### Have the person add the binary in Input Monitoring

The path a bare binary leaves open: press `+` in System Settings and pick
`/usr/local/libexec/favjit/favjit`. **Not taken**: it is a file dialog and a path to
type for something an install can do, and the request that would have created the
entry never produced one, so the list was the only way in.

### Ask for input monitoring rather than Accessibility

The permission actually needed. **Not taken**: asking for it produced no dialog and
no entry even from a bundled application in the session, while Accessibility produced
both — and the Accessibility grant covers input monitoring as well.

### Register the daemon with `SMAppService`, as Karabiner does

Plists inside the bundle, registered by the application. **Not taken**: it wants a
Developer ID signed bundle, and there is none to sign with; the plists in
`/Library/LaunchDaemons` and `/Library/LaunchAgents` already work
([ADR-0012](0012-macos-install-as-a-daemon-and-turn-off-with-a-file.md)).

### Carry on with no permission instead of ending the run

A process that stays up and converts nothing. **Not taken**: it looks alive, never
asks again, and the answer arrives from outside the process — so exiting is what
makes the grant take effect at all.
