---
name: trace
description: Taking the trace out of the favjit run on this Mac and reading it — asking the supervisor for the ring over its socket with sudo, reporting on it, replaying it into the bytes it would have written and a host-sim script, and turning a field incident into a red end-to-end test. Use whenever the user says something misbehaved (a key did the wrong thing, alt+n did not switch, a keyboard went dead), asks to look at "the trace", or wants a regression case from a real run.
---

# Taking and reading a trace

The run keeps a bounded ring of everything it saw and did (ADR-0009). It is in
memory the watchdog and supervisor can read, never in a file, so **the first thing
to do is take it**: the ring evicts from the start, and every keystroke typed since
the incident pushes it closer to gone. Take it before reading code.

## 1. Take it

Only root may take one, because a trace is a keylog. sudo here is Touch ID, so run
it yourself with a spoken cue (see the `sudo` skill), and put the file **outside the
repo** — the scratchpad directory if the session has one, else `$TMPDIR`. Never
under `/Users/youxkei/repo/favjit`, and never commit one.

```
say "トレースを取ります。Touch ID をお願いします" ; sudo favjit --trace-out "$OUT/favjit.trace"
```

`favjit` on PATH is the installed binary; `--trace-out` only copies bytes off the
socket at `/var/run/favjit-trace.sock`, so the installed build is fine here.

**Two recordings come back, and the second one is usually the one you want.** The
answer carries the run going on now and the run before it, so `--trace-out` writes
`$OUT/favjit.trace` and `$OUT/favjit.trace.previous`. A run that failed — the
output device's connection going, a wedge the watchdog killed, a link that would
not come back — is the *previous* one, because the failure is what started the run
you are now talking to. Read that one first. It is absent only when the supervisor
has come up since and has nothing kept.

- `nothing is supervising a run to ask` — no favjit is running under the supervisor
  (`pgrep -fl favjit` shows what is up).
- `the supervisor had no trace to give` — the run was started without a region;
  there is nothing to recover, say so.
- Only the live file written, no `.previous` — the supervisor came up without a
  kept region, or the run before it recorded nothing. The recording is not
  recoverable from anywhere else, so say that rather than reading the live one as
  though it covered the incident.
- `only root may take a trace` — the sudo did not take; retry per the `sudo` skill.

## 2. Read it

Summary first, from the file, no root needed:

```
favjit --trace-report "$OUT/favjit.trace"
```

Check `records dropped from the start` and `span` against when the incident was:
if the span does not reach back to it, the incident is evicted and the trace cannot
answer — report that instead of reading tea leaves in what remains.

Then replay, **with the repo's build**, so the conversion under test is the current
code and not the installed one:

```
cargo run -q -p favjit-bin-macos -- --replay "$OUT/favjit.trace" [--from <checkpoint>]
```

It prints every record (this is the keystrokes — keep it in the scratchpad, not in
the reply beyond the lines that matter), then per checkpoint the bytes that would
have crossed to the device, and `as a script:` — a `SimHost` script in the
end-to-end suite's vocabulary. Find the segment around the incident, and narrow
with `--from` to the checkpoint just before it.

## 3. Make it a test

Paste the `as a script` block into a new test in `crates/e2e/tests/` (pick the file
by subject; `switching.rs` is alt+n), state what the run *should* have written —
that is the one thing the trace cannot say — and run it red before touching
`engine`. The test is the record of the incident; the trace file is not kept.

## Do not

- Do not skip the report and go to code: the report says whether the trace even
  covers the incident.
- Do not paste the raw record dump or the trace file anywhere outside the machine.
- Do not read platform behaviour off the trace and write it into `docs/platform/`
  as observed fact — a trace shows what `engine` decoded, not what the OS did.
