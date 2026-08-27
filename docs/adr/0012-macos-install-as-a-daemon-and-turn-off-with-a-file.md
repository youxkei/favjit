# ADR-0012: On macOS, install as a launchd daemon and turn favjit off with a file

- **Status**: Accepted
- **Date**: 2026-08-29

**This ADR is about macOS only**, like [ADR-0011](0011-macos-output-through-a-virtual-hid-device.md):
it is how the sink's process is started and stopped on that platform. Nothing here
applies to the Windows side, which needs no privilege and seizes nothing.

## Context

A converter that runs only while a terminal is open is not one you can type on. The
privilege it needs is root, for the seize, and root on macOS means a launchd daemon
in the system context with no GUI session.

What that costs is settled rather than assumed
([virtual-hid-device.md](../platform/macos/virtual-hid-device.md),
[input-permissions.md](../platform/macos/input-permissions.md)):

- **A session-less root daemon drives the virtual HID device**, and its keystrokes
  reach the lock screen. Output as a device needs nothing from the window server,
  so the tension between "root has no session" and "posting is a window server
  call" does not arise.
- **launchd's system domain refuses a plist that is not root-owned in a directory
  only root can write.** It fails as `Bootstrap failed: 5: Input/output error`,
  with no job created and nothing logged about it.
- **The platform releases a seize when the process dies**
  ([input-suppression.md](../platform/macos/input-suppression.md)). That is what
  ADR-0008's watchdog rests on, and it is also the cheapest way to hand a keyboard
  back deliberately.

Being able to turn favjit off matters as much as installing it. A layout this far
from QWERTY is one nobody else can type on, and there are things — a game, someone
else borrowing the machine, debugging favjit itself — where the answer is to stop
converting for a minute. **"Off" has to mean the keyboards are the machine's own**,
not merely that conversion stopped: a process that held a seize while doing nothing
would be the failure [ADR-0008](0008-input-suppression-and-watchdog.md) rules out.

And turning it off must not need a password. A menu item that prompts every time is
a menu item nobody uses, which would leave the answer being "kill the daemon from a
terminal" — the thing that is hard to do when the keyboard is the problem.

## Decision

**`favjit --install` writes a `LaunchDaemon` that runs the watchdog, with favjit as
its argument**, `KeepAlive` and no time bound. The binaries are copied to a
root-owned directory rather than pointed at where they were built, and the plist and
the binaries are `root:wheel`. `favjit --uninstall` boots the job out and removes
them.

**Off and on is the presence of a file** in the console user's own
`Library/Application Support/favjit/`, written by `favjit --disable` and removed by
`favjit --enable`, neither of which needs privilege.

**The transition is favjit ending its run and launchd starting it again.** With the
file present it captures nothing and seizes nothing, and waits. When the file
appears mid-run it lets the loop return — which is what runs the destructors that
tell the virtual keyboard nothing is held and give the keyboards back — and launchd
restarts it into the waiting state.

## Consequences

Turning favjit off and on is two commands with no privilege, so a menu is a
two-line script in whatever menu-bar tool is already installed. There is no menu-bar
application here, and this is the mechanism one would call.

The off state is a process that holds nothing rather than no process, so it is
visible in `launchctl print` and it comes back by itself: the answer to "is favjit
running?" stays yes, and the answer to "is it converting?" is a file.

Any process running as that user can turn the converter off. That is a real
weakening of nothing much: the direction of the failure is the safe one, since off
means the keyboards are given back rather than taken, and a process that could write
that file could equally have taken the input another way.

A transition costs a launchd restart, on the order of a second. Long enough to
notice, short enough that it does not matter for something done a few times a day.

`KeepAlive` means a systematically broken build cycles — start, seize, wedge, get
killed, start again — bounded by launchd's own throttle. The keyboards come back in
each gap, which is the degradation ADR-0008 asks for, but a broken favjit installed
this way is worse than a broken favjit run from a terminal. `favjit --uninstall` is
the way out and it does not depend on favjit working.

Installing needs a second thing installed first: the DriverKit package ADR-0011
depends on. Nothing here sets that up, and favjit refuses to start rather than
seizing with nowhere to send keystrokes.

## Alternatives considered

### Release and re-seize in place, with no restart

Keep one process for both states and have it drop the devices when told. **Not
taken**: capture would have to release its queues and then find the keyboards again
without an attach notification to hang it on, and the seize-release path that is
actually measured is the process dying. This would be new machinery in the host, in
exchange for saving a second.

### `launchctl bootout` and `bootstrap` for each toggle

No new mechanism at all — the state is whether the job is loaded. **Not taken**: it
needs root every time, so a menu would prompt for a password or need a sudoers rule
written for it, and the state "off" would be indistinguishable from "not installed".

### A signal to the running daemon

`SIGUSR1` to toggle. **Not taken** for the same reason: signalling a root process
takes root. It also cannot answer "off" at startup, so a machine that rebooted while
favjit was off would come back converting.

### A menu bar application

The obvious shape of what was asked for. **Not taken now**: it is an AppKit
application, an app bundle, a LaunchAgent in the user's session and some way to talk
to the root daemon — and the thing it would talk to is this. The file is that
interface, and it is reachable from anything.
