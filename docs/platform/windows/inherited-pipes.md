# Handing a pipe down to a child

Observed on **Windows 11 25H2, build 10.0.26200.9168**, 2026-08-31, from an unsigned
`cargo`-built binary run from a terminal. The parent made a pipe, spawned a copy of
itself with the handle's value in an environment variable, and the child wrote a byte
to that handle.

This is what the watchdog's link to favjit rests on
([ADR-0008](../../adr/0008-input-suppression-and-watchdog.md)), so it was measured
rather than assumed.

## `SECURITY_ATTRIBUTES` is 24 bytes

On the 64-bit build: `DWORD nLength`, four bytes of padding, `LPVOID
lpSecurityDescriptor`, `BOOL bInheritHandle`, four more of padding. A structure laid
out without the padding asks `CreatePipe` for the opposite of what was meant — a pipe
no child can inherit — and returns success.

## `std::process::Command` passes on a handle marked inheritable

```
CreatePipe(&read, &write, &{ nLength: 24, lpSecurityDescriptor: NULL, bInheritHandle: TRUE }, 0)
                                                            -> 1
SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0)          -> 1
Command::new(current_exe()).env("...", (write as usize).to_string()).status()
```

The child, given only the handle's numeric value, wrote one byte to it with
`WriteFile` — `ok=1 written=1` — and the parent read that byte back off the read end.
So **a handle is inherited with the same value it has in the parent**, and telling the
child a number is enough; there is no need to call `CreateProcessW` directly in order
to set `bInheritHandles`.

`SetHandleInformation` clearing the flag on the parent's own end is what keeps that end
out of the child. It matters for the read end of the heartbeat pipe: a write end the
child inherited would hold the pipe open after the child had gone, so the read never
ends and a dead process looks like a quiet one.

## `PIPE_NOWAIT` is accepted on an anonymous pipe

`SetNamedPipeHandleState(handle, &(PIPE_READMODE_BYTE | PIPE_NOWAIT), NULL, NULL)`
returned **1** on a handle from `CreatePipe`, in the child, on the inherited end.

That is the equivalent of `O_NONBLOCK` on the macOS side, and it is what keeps neither
direction of the watchdog link able to stall the loop that is holding the keyboards.
Anonymous pipes are named pipes underneath, which is why a call named for the latter
works on the former.

## Not established

- What `ReadFile` answers on a `PIPE_NOWAIT` pipe with nothing in it. The reader here
  treats both "true with zero bytes" and "false" as nothing to read, so the difference
  has not had to be measured.
- Whether a full `PIPE_NOWAIT` pipe reports a partial write or a failure. Either is
  read as the heartbeat not getting through, which is the same answer.
