# What favjit needs on Windows, and what it does not

Observed on **Windows 11 25H2, build 10.0.26200.9168**, 2026-08-31 to 2026-09-06, from
unsigned `cargo`-built binaries run from an ordinary terminal.

## Nothing here needs elevation

Every mode has run as the person's own account, with no prompt and nothing denied:

| what was run | what it needed |
|---|---|
| `--identity`, `--devices` | reading the device list, and a file under `%LOCALAPPDATA%` |
| `--pair <digits>` | a UDP socket for mDNS, a TCP connection, and a file to write |
| `--dry-run false` | a low-level hook, a raw input registration, and the link |
| `favjit-watchdog -- favjit …` | two anonymous pipes, a job object, and a child it can end |
| `--install` | two logon tasks registered, started and queried ([logon-tasks.md](logon-tasks.md)) |

**This is the whole difference in shape from the Mac's half**, which needs root twice
over: the seize is refused as `kIOReturnNotPrivileged` without it
([../macos/input-suppression.md](../macos/input-suppression.md)), and the socket the
virtual HID device is reached through is under a `rootonly` directory
([../macos/input-permissions.md](../macos/input-permissions.md)). Neither has an
equivalent here — a hook and a raw input registration are things any process may ask
for, and what favjit writes goes under the profile of whoever ran it.

`%LOCALAPPDATA%` rather than a machine-wide directory is what keeps it that way, and it
is not only convenience: the identity is the whole of what makes the Mac accept
keystrokes from this machine, so keeping it out of other users' reach is worth having
for its own sake.

## Both halves belong to a login session

A low-level hook is called on the thread that installed it, and raw input is delivered
to a window. Both are session objects, so favjit here cannot be a service in the way the
Mac's half is a root daemon: what it wants is to be started once a person has logged in.
What that costs and what it takes to do invisibly is
[logon-tasks.md](logon-tasks.md).

## Not established

- Whether a hook installed by an unelevated process sees input going to an **elevated**
  window. If it does not, keys typed into anything running as administrator would be
  neither refused here nor relayed there, and the symptom would be one application in
  which the keyboard behaves as though favjit were not running. Nothing here has
  measured it, and every window used so far has been an ordinary one.
- What a shell hotkey does while input is being refused — `Win`+`r`,
  `Ctrl`+`Alt`+`Del`. The `alt` chords were measured because they broke
  ([hooks-and-raw-input.md](hooks-and-raw-input.md)); these were not.
