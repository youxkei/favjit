# Input permissions

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-27.

## Preflight without prompting

A process can ask whether it already has event access without triggering a
prompt. From a plain (unsigned, unbundled) `cargo`-built binary run from an
interactive terminal:

| Call | Result |
|---|---|
| `CGPreflightListenEventAccess()` | `true` |
| `CGPreflightPostEventAccess()` | `true` |
| `AXIsProcessTrusted()` | `false` before the terminal was granted Accessibility; `true` after |

All three link from `CoreGraphics` / `ApplicationServices` with no bundle and no
code signature.

**The grant belongs to the terminal, not to the binary.** These returned `true`
for a throwaway binary under `/private/tmp`, because the terminal application
running it holds the TCC grants and child processes inherit the decision. Nothing
here says what an installed, signed favjit would get on a machine where the user
has granted nothing.

## A daemon and `CGEventPost` pull in opposite directions

Not measured, and the reason it matters is structural rather than a detail. Seizing
a keyboard takes root, which on macOS means a LaunchDaemon — and a LaunchDaemon
runs in the system context with no GUI session, while `CGEventPost` is a call into
the window server. A LaunchAgent has the session and not the privilege.

Karabiner does not have this problem, and its shape shows why: its output is a
DriverKit virtual HID device, so the root daemon needs nothing from the window
server. Its `Karabiner-Core-Service` is a plain `KeepAlive` LaunchDaemon with
`ProcessType: Interactive`, registered through `SMAppService.daemon(plistName:)`
from a helper inside its app bundle — which is also why it needs a bundle at all,
since `SMAppService` reads the plist from `Contents/Library/LaunchDaemons` of a
signed one. An unsigned plain binary can still be a daemon by installing a plist
into `/Library/LaunchDaemons` and bootstrapping it once with root.

**A session-less root daemon does drive a virtual HID device**, so the tension is
answered by not needing the window server at all: a `LaunchDaemon` reached a ready
virtual keyboard and pointing device, and its keystrokes reached the lock screen
([virtual-hid-device.md](virtual-hid-device.md)). Whether it could have *posted*
events instead is not measured, and nothing here rests on it
([ADR-0011](../../adr/0011-macos-output-through-a-virtual-hid-device.md)).

Installing that plist has one requirement whose failure is not reported.
launchd's system domain refuses a job whose plist is not root-owned in a directory
only root can write. A plist in a scratch directory under `/private/tmp`
(`drwxrwxrwt`) fails as

```
Bootstrap failed: 5: Input/output error
```

with **no job created and nothing logged about it** — the reason has to be inferred
from the ownership. `install -o root -g wheel -m 644` into `/Library/LaunchDaemons`
and the same bootstrap succeeds. The program it names wants the same treatment.

Karabiner's driver is what favjit sends through.
`Karabiner-DriverKit-VirtualHIDDevice` is a separate package from
Karabiner-Elements, is installed on this machine, and its daemon accepts clients
over a UNIX socket at

```
/Library/Application Support/org.pqrs/tmp/rootonly/karabiner_virtual_hid_device_service.sock
```

That directory is `drwx------ root wheel`: **access is by filesystem permission
alone, with no check of the client's code signature**, and favjit needs root for
seizing anyway. The protocol is a one-byte request — `virtual_hid_keyboard_initialize`,
`post_keyboard_input_report`, `post_apple_vendor_top_case_input_report` among them
— followed by a packed HID report, inside a request/response transport described in
[virtual-hid-device.md](virtual-hid-device.md).

It also puts favjit below Secure Keyboard Entry. The cost is a dependency on a
component of the program favjit replaces — separable, since the driver installs on
its own and Karabiner-Elements' own agents can stay stopped.

## What an installed daemon gets, and how it is granted

Observed 2026-08-29, same machine and OS version.

**Root is not enough, and the window server's answer is the wrong question.** As a
LaunchDaemon, `CGPreflightListenEventAccess` returned `false` and every keyboard
refused to open with `0xe00002e2` (`kIOReturnNotPermitted`) — while the virtual HID
output device opened normally, so the two directions need different things.
`IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)` is the question that tracks it, and
it tracked it exactly: `Granted` in the runs where both keyboards came up `held
exclusively`, `Denied` in the runs where every open was refused.

The window server's answer does move with the grant — both preflights read `true` in
the daemon runs where the HID answer was `Granted` — but it is `false` whenever the
grant is missing *and* whenever there is no session, so it cannot tell the two apart.

**A daemon cannot ask.** `IOHIDRequestAccess` from a session-less root daemon
returned `false`, left the answer at `Unknown`, showed no dialog, and put no entry in
the Input Monitoring list. There is nothing for a person to switch on.

**A bundle asks, and Accessibility is what it should ask for.** With the binaries
inside an app bundle, opened as an application in the login session
(`launchctl asuser <uid> /usr/bin/open -n -a <bundle> --args …`),
`AXIsProcessTrustedWithOptions` with the prompt option put a dialog on screen and an
entry in the Accessibility list. Turning that switch on made
`IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)` answer `Granted` — on this version
the Accessibility grant covers input monitoring, which is also what
Karabiner-Elements' own source says it relies on for macOS 26. The same request for
input monitoring alone never produced a list entry.

**An ad hoc signature is enough to be granted, and not enough to stay granted.** No
Developer ID was involved, and no signing identity exists on this machine; the bundle
was signed `codesign --force --sign - --identifier <id>`. That was accepted. But a
rebuilt and re-signed bundle is a different identity: the switch in System Settings
still reads as on, while `IOHIDCheckAccess` answers `Denied` and no dialog appears,
because the question counts as answered. `tccutil reset Accessibility <bundle-id>`
and `tccutil reset ListenEvent <bundle-id>` clear that, after which asking prompts
again. Karabiner's core service is Developer ID signed with a team identifier and no
entitlements at all, which is what lets its grants survive an update.

Opening the bundle to ask takes two flags that are easy to miss, both of which
Karabiner also uses. `open` needs **`-n`**: the daemon and the menu bar item run from
inside the bundle, so Launch Services considers the application already running,
activates it and drops the arguments. And `open -W` cannot block on an `LSUIElement`
application — it prints `Unable to block on application` and returns — so the answers
have to be collected from a file the launched process writes.

What arrives in `argv` is exactly what followed `--args`, and nothing else. A bundled
binary opened as `open -n -a <bundle> --args --permission-check /a/file` saw
`argv[1..]` as those two arguments alone; Launch Services added no `-psn_…` or any
other argument of its own. Measured on macOS 26.6.2 (build 25G83) with a bundle
holding nothing but an `LSUIElement` `Info.plist` and a binary that writes its `argv`
to a file.

## Not established

- Whether the bundle has to be under `/Applications`. It is there because that is
  where an application goes; a bundle elsewhere was tested only with an identity
  that already carried a refusal, which cannot prompt.
- Whether posting events requires a grant separate from listening in practice.
  Both preflights were already `true` from a terminal, so neither prompt was seen.
