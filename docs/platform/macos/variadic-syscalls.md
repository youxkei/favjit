# Variadic C functions on arm64

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-28,
with rustc 1.96.0.

## `fcntl` declared non-variadic applies the wrong flag

`fcntl` is variadic in the header:

```c
int fcntl(int fildes, int cmd, ...);
```

Declared in Rust with a fixed third argument, it compiles and links and returns
success:

```rust
extern "C" { fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32; }   // wrong
```

**The flag actually applied is not the one passed.** On arm64 the fixed-argument
convention puts the third argument in a register, while a variadic callee reads
its variable arguments off the stack, so `fcntl` sees whatever was there.

Measured through the watchdog's two inherited pipes, whose descriptors are
`probe_read` 3, `probe_write` 4, `beat_read` 5, `beat_write` 6. The parent sets
`FD_CLOEXEC` on its own ends, 4 and 5, and clears it on the child's ends, 3 and 6,
from `pre_exec`. With the wrong declaration, `ls -l /dev/fd` in the child shows:

```
3  dr--------  directory      <- the child's probe end, closed
4  p-w--w----  pipe, write    <- the parent's end, inherited
5  dr--r--r--  directory
6  p-w--w----  pipe, write    <- position taken by something else
```

Exactly inverted: the sets did nothing and the clears set the bit. With the
declaration corrected to `fn fcntl(fd: i32, cmd: i32, ...) -> i32;`, the same
command in the same child shows what was intended:

```
3  pr--r-----  pipe, read     <- the probe end, with a probe already in it
6  p-w--w----  pipe, write    <- the heartbeat end
```

Two consequences followed from the wrong declaration, both of which looked like
something else entirely:

- **`O_NONBLOCK` was never set.** A `read` that was meant not to block did block,
  which is a supervisor sitting in a call that looks exactly like a child
  answering.
- **The watchdog killed a healthy favjit after two seconds**, every time, because
  the heartbeat's descriptor in the child was not the pipe. Writes to it
  succeeded, so favjit saw nothing wrong; the watchdog read EOF and timed out.

Neither failure produced an error at the call site. `fcntl` returned 0 both
times.
