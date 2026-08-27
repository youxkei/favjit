# Reading and asking across processes, through a window

Observed on **Windows 11 25H2, build 10.0.26200.9168**, 2026-09-01, from unsigned
`rustc`-built binaries run from a terminal. One process made a message-only window of a
known class and put a value on it as a window property; a second process, started
separately and under the same account, found that window by class name, read the
property, and posted it a `WM_APP` message.

This is what the tray item's link to a running favjit rests on
([the tray item](tray-item-as-its-own-program.md)), so it was measured
rather than assumed.

## A window property crosses the process boundary, in both directions

| call | from the other process | what came back |
|---|---|---|
| `FindWindowW(class, NULL)` | the message-only window's handle | `0x460f42` |
| `GetPropW(window, name)` | the value the holder had set | `2`, then `1` after an ask |
| `PostMessageW(window, WM_APP + 1, 1, 0)` | non-zero | the message arrived |

`HWND_MESSAGE` as the parent does not hide the window from `FindWindowW`: it has no
screen presence, takes no input focus and appears in no task switcher, and is still
found by its class name.

Neither call sends a message to the holder's thread, so **neither waits on it**. That is
the property that matters here: the thread the tray reads across is the one holding the
keyboards, and a read that blocked on it would make the escape unavailable in exactly the
state it exists for.

`GetLastError` read 6 after each of the three succeeding calls, which is a value left
over from something earlier — every one of them reported success in its return value.

## What the holder stops holding, it stops holding

With the first process killed, `FindWindowW` returned null. The window is destroyed with
the process and the property with the window, so **there is no state left behind for a
second process to read**.

This is the whole reason the state lives on a window rather than in a file: a file says
which machine the keyboard was driving when it was last written, and goes on saying it
after the process that wrote it has gone. What a tray item drawn from that file would
show is a keyboard that is somewhere, when in fact nothing is forwarding at all.
