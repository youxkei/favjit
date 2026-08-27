# Starting favjit at logon, and what it takes to start it invisibly

Observed on **Windows 11 25H2, build 10.0.26200.9168**, 2026-09-06, from `cargo`-built
binaries registered by `favjit --install` and run as the person's own account. Windows
Terminal is this machine's default terminal.

## Registering a logon task needs no elevation

`schtasks /create /xml <file> /tn <name> /f`, from an ordinary process, for a task whose
principal is the person and whose `RunLevel` is `LeastPrivilege`: both tasks were created,
started and queried with no prompt and nothing denied. This is what
[privileges.md](privileges.md) records for the rest of what favjit does, and it holds for
putting favjit in place as well.

`/create ... /f` **replaces the whole definition**, which includes
`<Settings><Enabled>`: a task previously turned off with `schtasks /change /disable` comes
back enabled. So re-installing is one command and needs no `/enable` before it.

`/create` does not start what it registers. Starting it is `schtasks /run /tn <name>`.

## The XML says three things the command-line flags cannot

`schtasks /create`'s own flags have no way to express any of these, and two of them are
wrong by default — so the definition is XML rather than flags:

| setting | the default, and why it is wrong |
|---|---|
| `<ExecutionTimeLimit>` | 72 hours, after which the task is stopped. `PT0S` is no limit |
| `<DisallowStartIfOnBatteries>`, `<StopIfGoingOnBatteries>` | true: on a laptop the task neither starts nor keeps running |
| `<RestartOnFailure>` | absent. This is what brings the pair back after a wedge was ended, the way launchd's `KeepAlive` does on the Mac |

## `<Hidden>true</Hidden>` does not stop a console window

Measured directly: with the setting true, starting the console-subsystem supervisor
through the task produced a `WindowsTerminal.exe` whose window title was the supervisor's
own path.

```
"WindowsTerminal.exe","39788",...,"C:\Users\…\favjit\bin\favjit-watchdog.exe"
"favjit-watchdog.exe","33784",...,"N/A"
```

A console program started by a task is given no console to inherit, so the system makes
one for it — and with Windows Terminal as the default that is a terminal window on the
desktop for the whole session. The task's `Hidden` flag does not reach it.

**What does stop it is the subsystem.** A window-subsystem program is never given a
console. Its console-subsystem *children* still are, unless spawned with
`CREATE_NO_WINDOW` (`0x08000000`). With the supervisor built as a window-subsystem program
and the process it supervises spawned with that flag, no process named `favjit` appears
with a window title and no terminal is created:

```
"favjit-watchdog.exe",...,"N/A"
"favjit-tray.exe",...,"N/A"
"favjit.exe",...,"favjit-capture"     ← its message-only window, which never draws
```

What that costs is a window-subsystem program printing nothing when run by hand, which is
why `favjit-watchdog` takes `--log`.

## A job object is what ends the child when the supervisor is killed

`schtasks /end` on the supervisor's task, before this was in place, left the process it
supervised running:

```
"favjit.exe","32456",...        ← the only one left
```

That is the outcome [ADR-0008](../../adr/0008-input-suppression-and-watchdog.md) rules
out — a process holding this machine's input refused with nothing left that would end it —
and the log of that orphan says so from its own side:

```
the heartbeat is not reaching the watchdog (パイプを閉じています。 os error 232);
it will end this process for a broken link rather than for a fault
```

A job object created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, held by the supervisor for
its whole life and assigned the child, closes when the supervisor's last handle does —
which is when the supervisor ends, however it ends. With it, the same `schtasks /end`
leaves nothing:

```
"favjit-tray.exe","10892",...   ← the other task, untouched
```

`JOBOBJECT_EXTENDED_LIMIT_INFORMATION` is 144 bytes on the 64-bit build, of which
`JOBOBJECT_BASIC_LIMIT_INFORMATION` is 64 and `IO_COUNTERS` is 48. The length is handed to
`SetInformationJobObject` beside the structure and is part of what says which class of
information it is, so a wrong one is refused — and a refused limit leaves a job that ends
nothing, silently.
