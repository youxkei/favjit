# The virtual HID device service

Read off the published source of `Karabiner-DriverKit-VirtualHIDDevice` at
`client_protocol_version` 7, on this machine (macOS 26.6.2, build 25G83), where
that package is installed. The protocol below is read off that source; the
behaviour under [What it buys](#what-it-buys) is measured, from a root probe built
against the driver's own client library.

## Reaching it

```
/Library Application Support/org.pqrs/tmp/rootonly/karabiner_virtual_hid_device_service.sock
```

(one path, no space — written with one here only to keep the line short).
`drwx------ root wheel`, so **access is by filesystem permission and nothing
else** — no check of the client's code signature. A UNIX **stream** socket.
Messages are at most 1024 bytes.

## Framing

Two layers, and only the inner one is the service's own. Every request is a header
followed by a fixed-layout payload, all native-endian, `memcpy`ed with no padding
between the two:

```
u16  client protocol version   (7)
u8   request
...  payload, if the request has one
```

That buffer is then **handed to `pqrs::unix_domain_stream::client::async_request`**,
which is a request/response transport of its own — correlated request ids,
heartbeats every 30 s, read and write timeouts of 15 s — not a bare write of those
bytes to the socket. Responses arrive the same way, as pairs of
`(response type, value)` bytes carrying `driver_activated`, `driver_connected`,
`driver_version_mismatched`, `virtual_hid_keyboard_ready` and
`virtual_hid_pointing_ready`; a client that never reads them does not know when the
virtual keyboard exists, and a report posted before then goes nowhere.

**So the byte layout above is not sufficient to speak to this service.** The
transport around it is small enough to state in full, and **big-endian where the
service's own payloads are native-endian**:

```
frame = u32 body_size (big-endian) | u8 message_type | body

message_type: 0 heartbeat, 1 user_data, 2 health_check,
              3 health_check_response, 4 request, 5 response

body for request and response = u64 request_id (big-endian) | payload
body_size counts the type byte and the request id
```

A client has to send a heartbeat every few seconds — the library's peers send one
every 3 s and tear down a peer whose deadline passes — and has to answer a
`health_check` with a `health_check_response`. Posting reports is not a substitute:
a run with nothing to type would go quiet and be dropped.

The payload sizes are `sizeof` on the client's own types rather than a reading of
their fields, because each is `memcpy`ed and its padding is part of the format:

| payload | bytes |
|---|---|
| keyboard parameters | 24 |
| keyboard report | 67 |
| pointing report | 8 |
| consumer, apple vendor, generic desktop reports | 65 each |

**That is the whole of it, and a client of about 250 lines with no dependency
beyond the standard library reaches the same place as the C++ one:**

```
connected
driver_activated true driver_connected true version_mismatched false
keyboard_ready true pointing_ready true
o                                     <- the 67-byte keyboard report
カーソルは 474 px 動いた              <- the 8-byte pointing report
```

The 474 px is the cross-check that matters, because a report of the right length
with its fields in the wrong places would still be accepted: 20 reports of `x = 4`
asked for 80 points and moved the cursor 474, a ratio of 5.93 — the same ratio the
C++ client produced at the same delta. So the field layout is right and not merely
the length. Two runs produced 474 to the pixel.

A probe that both types and moves the cursor cannot check its characters by
watching them arrive, mind: moving the cursor can take the focus off the window the
earlier keystrokes were going to. The keyboard type is read system-wide and is
unaffected.

The requests, in the order the enum declares them (so the numbers are 0-based):

| # | request | payload |
|---|---|---|
| 0 | `virtual_hid_keyboard_initialize` | keyboard parameters, 24 bytes |
| 1 | `virtual_hid_keyboard_terminate` | — |
| 2 | `virtual_hid_keyboard_reset` | — |
| 3 | `virtual_hid_pointing_initialize` | — |
| 4 | `virtual_hid_pointing_terminate` | — |
| 5 | `virtual_hid_pointing_reset` | — |
| 6 | `post_keyboard_input_report` | keyboard report, 67 bytes |
| 7 | `post_consumer_input_report` | consumer report |
| 8 | `post_apple_vendor_keyboard_input_report` | vendor-page report |
| 9 | `post_apple_vendor_top_case_input_report` | vendor-page report |
| 10 | `post_generic_desktop_input_report` | generic-desktop report |
| 11 | `post_pointing_input_report` | pointing report, 8 bytes |

## Payloads

Keyboard parameters — three `uint64_t`, so 24 bytes and no padding:

```
u64  vendor id      (0x16c0 by default)
u64  product id     (0x27db by default)
u64  country code
```

Keyboard report, packed, 67 bytes:

```
u8    report id = 1
u8    modifiers            bitmask
u8    reserved
u16   keys[32]             HID usages, zero-terminated in the sense that unused
                           slots are 0
```

**The keys are 16-bit**, not the byte-wide usages a plain keyboard report carries.
That is what lets the same report reach usages above 0xFF, and it means a report
written against the ordinary HID keyboard descriptor would be laid out wrongly.

Pointing report, packed, 8 bytes:

```
u32   buttons              bit 0 is button 1
u8    x                    delta
u8    y                    delta
u8    vertical wheel
u8    horizontal wheel
```

Deltas are a single byte each, so one report carries at most ±127 of motion and a
faster movement is several reports.

The control reports — consumer, Apple vendor top case, Apple vendor keyboard,
generic desktop — are all the same 65 bytes, and the same shape as the keyboard
report without its modifier and reserved bytes:

```
u8    report id
u16   keys[32]             HID usages on that report's page
```

**Each has a report id of its own, and it is not the request number.** The ids are
the ones the report structs are constructed with, and the pairing with the request
that posts them is:

| page | request | report id |
|---|---|---|
| keyboard/keypad | 6 `post_keyboard_input_report` | 1 |
| consumer | 7 `post_consumer_input_report` | 2 |
| apple vendor top case | 9 `post_apple_vendor_top_case_input_report` | 3 |
| apple vendor keyboard | 8 `post_apple_vendor_keyboard_input_report` | 4 |
| generic desktop | 10 `post_generic_desktop_input_report` | 7 |

Note that requests 8 and 9 are in the opposite order to report ids 3 and 4: the
top-case report is posted by the *later* request and carries the *lower* id.

The `keys` array is a set, with the same insert-and-erase semantics as the
keyboard's: a usage goes into the first free slot, is not added twice, and is
cleared from every slot it appears in. So a report is state — what is down on that
page — and not an event.

Which usages each report can carry is bounded by the usage maximum its collection
declares in the device's report descriptor, and the four differ:

| report | usage minimum | usage maximum |
|---|---|---|
| consumer | 0 | `0x300` |
| apple vendor top case | 0 | `0xff` |
| apple vendor keyboard | 0 | `0xff` |
| generic desktop | 0 | `0xff` |

The consumer collection's own comment in the descriptor gives the reason for its
wider range: "Consumer usage 29d-ffff is reserved".

## The service takes any root client

An unsigned probe run under `sudo`, with Karabiner-Elements' own agents not
involved in the connection:

```
uid 0
connected to the service
driver_activated 1
driver_connected 1
driver_version_mismatched 0
keyboard ready 1, pointing ready 1
```

So **being root is the whole of the admission**, as the directory permissions
imply, and a virtual keyboard and a virtual pointing device both reach the ready
state for a client that is not Karabiner. The daemon is a launchd daemon of its own
(`Karabiner-VirtualHIDDevice-Daemon`), separate from Karabiner-Elements, and it
keeps running when Karabiner-Elements is stopped.

## What it buys

To the OS the result is a keyboard and a mouse, not synthesised events.

**A keystroke arrives, and it arrives through Secure Keyboard Entry.** The probe
enabled the protection itself, so it was both the protected application and the
receiver — which is the arrangement that matters for an output path, since the
application taking the password is the one holding it:

```
secure input on elsewhere beforehand: no
secure input now reported on: yes
arrived under the protection: YES
```

**This is where a consuming event tap fails.** With the protection held, a tap
receives no events at all and the physical keystrokes go past it unconverted
([event-tap.md](event-tap.md)). A virtual device is below that layer.

Without the protection the same keystroke did **not** reach the probe's stdin,
which is the focused window consuming it rather than the keystroke being lost: a
later run typed three of them into a focused terminal and three `o` characters
appeared. The protection routes input to the process that enabled it, bypassing
focus, so a probe can only check delivery this way while it holds the focus or the
protection.

**The pointer is accelerated, on a curve.** Twenty-five reports per run, 8 ms
apart, cursor warped to a known point first, none of them reaching the screen edge:

| per report | asked | cursor moved | ratio |
|---|---|---|---|
| `x = 1` | 25 | 98.73 | 3.95 |
| `x = 2` | 50 | 223.93 | 4.48 |
| `x = 4` | 100 | 592.77 | 5.93 |
| `x = 8` | 200 | 1677.44 | **8.39** |
| `CGEventPost`, absolute position | any | the amount asked | 1.00 |

The ratio climbs with the delta, so this is a curve and not a scale factor. A
posted mouse event moves the cursor one point per point whatever the delta, which
is what makes relaying a pointer through `CGEventPost` a matter of reproducing this
curve rather than of applying a coefficient.

**A control report acts, and the request-and-id pairing above is right.** Observed
2026-09-01, with favjit installed as the daemon, holding the internal keyboard and
converting: its function row moved the screen brightness and the volume. Those go
out as the consumer report — report id 2, request 7 — built from the usages
Karabiner's vendored `pqrs/hid/usage.hpp` names, so the whole of that path is
confirmed on hardware and not only read off the source. A wrong report id or a
wrong request has no error to show for it, which is what makes this worth stating:
the failure it rules out is silence.

Which control each key of that row is, is
[ADR-0019](../../adr/0019-convert-the-function-row-to-its-icons.md); why it has to
be sent as a control at all is in
[hid-input-callbacks.md](hid-input-callbacks.md).

## `LMGetKbdType()` cannot be read from the posting process

Read from the root probe it answered `91` before and after a virtual keystroke,
while an unprivileged process on the same machine, at the same time, read `46`:

```
LMGetKbdType() = 46        # unprivileged
LMGetKbdType() = 91        # the same call, as root
```

`91` is what an event created from a `kCGEventSourceStateHIDSystemState` source
carries when nothing has stamped a type on it, so the root process is not reading
the session's value at all. **Whether a virtual keyboard moves the value therefore
cannot be measured from inside the process posting to it** — and it is the reading
an unprivileged process gets that matters, since the consumer that resolves a chord
by character runs as the user. The two readings have to come from two processes.

Read that way it **does** follow the virtual keyboard, including out of the state
where the mismatch actually bites. A process spawned unprivileged immediately
before and after each virtual keystroke, with no physical key in between:

```
before:                       LMGetKbdType() = 42      <- JIS, the failing state
after a virtual keystroke:    LMGetKbdType() = 3
```

Starting at `42` is what makes this conclusive rather than suggestive: that is the
value the TrackPoint keyboard leaves behind, and the one under which a chord written
as `:` is looked up at the wrong position. A separate run started from `91` and
reached `3` as well, three keystrokes in a row.

**The country code is not what selects the type.** Re-created with
`not_supported (0)`, `japan (15)` and `us (33)` in one run, the reading after each
keystroke was `3` every time.

**A long-lived reader holds its first answer.** One sampling every 20 ms reported
`46` unchanged across eleven minutes that included Karabiner-Elements being
stopped, physical keystrokes and virtual keystrokes — while events in that same
window carried three different types. So this value must be read in a process that
did not exist before the keystroke, or it says nothing at all.

A listen-only tap over the same window read `kCGKeyboardEventKeyboardType` off each
event:

| event | `kCGKeyboardEventKeyboardType` | `LMGetKbdType()` |
|---|---|---|
| the virtual keyboard's `o` (three of them) | **3** | 46 (stale) |
| this machine's own keyboard, Karabiner-Elements stopped | **91** | 46 (stale) |
| this machine's own keyboard, Karabiner-Elements running | 46 | 46 (stale) |

The virtual keyboard's `3` is what the fresh reading then answers, so the event's
type and `LMGetKbdType()` agree once the reading is taken in a new process. The
type Karabiner's own remapping produces is `46`, on the same machine and the same
physical keyboard — the country code in the parameters is the obvious candidate for
the difference and has not been varied.

**The characters do not hang on this as much as the numbers suggest.**
`UCKeyTranslate` over the current ABC layout, key code `0x29` with shift:

| keyboard type | character |
|---|---|
| 40 (ANSI) | `:` |
| **42 (JIS)** | **`+`** |
| 3 (the virtual keyboard) | `:` |
| 91 (this machine's own keyboard) | `:` |
| 46 (what `LMGetKbdType()` answers) | `:` |

Only JIS puts `+` there. A consumer resolving a chord written as `:` disagrees with
an ANSI-shaped key code **only while the last physical keyboard was the JIS one** —
the everyday case with the TrackPoint keyboard, and why it was seen at all. Output
through the virtual keyboard makes that keyboard the last one used, and `3`
translates like ANSI, so the disagreement does not arise on this path.

## A wrong protocol version fails silently

Connecting and sending `virtual_hid_keyboard_initialize` with the version field set
to 6 and then to 8, where the daemon expects 7:

```
version 6: responses 3  version_mismatched false  keyboard_ready false  connected true
version 8: responses 3  version_mismatched false  keyboard_ready false  connected true
```

The connection stays open, status responses keep arriving, `driver_version_mismatched`
stays false — **and no virtual keyboard appears**. So the request is ignored rather
than refused, and there is no error to check: pin the version, and treat "readiness
never arrived" as the only signal that something is wrong.

## A daemon with no session drives it, and reaches the lock screen

The tension this settles is specific to macOS: seizing takes root, root there means
a launchd system daemon with no GUI session, and `CGEventPost` is a call into the
window server. Output through a device should be free of that, and is.

The same Rust client, run from a `LaunchDaemon` rather than from a terminal under
`sudo` — a root process started from a terminal inherits the user's bootstrap
namespace, so it would not answer the question:

```
daemon mode, uid 0
session type: Some("org.favjit.feasibility")
connected
keyboard_ready true driver_connected true pointing_ready true
posted 'o' #1 … posted 'o' #15
```

The `session type` is the launchd job's own name rather than a login session's.
And with the screen locked over those fifteen keystrokes, **dots appeared in the
password field**: a session-less root daemon's keystrokes reach `loginwindow`.

Karabiner-Elements was running throughout, so this also shows favjit's output
coexisting with another client of the same service — the exclusion between the two
is at the capture end, not here.

Converted keystrokes reach the password field as well, not only a probe's. With
favjit installed as a daemon, seizing both keyboards and converting, the lock screen
took typing — so the whole path is below the layer that protection sits at.

## favjit's own output through it

With the TrackPoint keyboard seized and favjit converting for real, its own client
wrote 590 reports over 30 s with no error and nothing unsendable, and the keystrokes
and the pointer both behaved on screen. Only that one keyboard was captured, which
is the arrangement the section below is about.

**`option` + `:` came out right in that run**, which is the case that fails on the
posting path: the chord is resolved by character against `LMGetKbdType()`, and on
the seized JIS keyboard that reads JIS, so an ANSI-shaped key code is looked up at
the wrong position and the keystroke falls through as `Ú`. Sent through the device
it does not, because the device is what that call then answers with.

What favjit's own path costs, in microseconds, from the same run:

| segment | n | p50 | p90 | max |
|---|---|---|---|---|
| HID stamp → capture | 235 | 108.8 | 144.0 | 365.3 |
| capture → report ready | 590 | 25.2 | 57.5 | 1949.6 |
| writing the report | 590 | 32.8 | 43.2 | 155.2 |

The two counts differ because the arrival figure is measured on key values only,
while a report goes out for every pointer movement as well — and a key-down and its
release are two reports, with a third when a modifier has to be let go after the
key.

## The device holds what it was last told, after the client is gone

A run killed with a key down leaves that key down. The device belongs to the
daemon, not to the process that wrote to it, so the last report stands: the client
socket closing does not reset it, and `SIGKILL` runs no destructor to send an
all-released report.

Observed with the watchdog killing a favjit wedged on purpose while a letter was
held, which is the arrangement ADR-0008 exists for: the keyboard came back — and
the letter went on repeating, because the OS repeats a key the device says is down
([key-repeat.md](key-repeat.md)).

**Which key was left down, and why, came out of the trace** — its first use on real
hardware, and it replayed faithfully. The key events recorded were down, up, down,
up, down, and then nothing but the supervisor's probes: the release that followed
that last press is **absent from the recording**, because the wedge parks before an
event is taken from the stream, so that release was never recorded and never handled.
The five injections end with a key-down and no matching release.

So two things hold the key down here, and only one of them is the platform's:

- the release never reaches the conversion, which is the wedge and not the OS, and
- the device goes on holding what the last report said, which is the OS.

**The key stops when the process dies**, and that is in the same logs rather than a
run of its own: the repeats continue through the two seconds the supervisor waits,
carry on for six more characters after it says it is killing, and end there. In that
run favjit sent no release — the log line for one is absent — so what ended it was
the process going away.

Which mechanism that is has not been separated: the daemon may clear the virtual
keyboard when a client disconnects, or may take the device away entirely once its
last client has gone. favjit was the only client, so the two are indistinguishable
here, and it matters only where something else is connected at the same time.

What follows for the design is that **a hang needs no deliberate release**: the kill
the supervisor already performs is the whole of the recovery, and its timeout is the
bound on the damage — a held key repeating at 28 a second for two seconds is around
sixty characters into whatever had focus. Releasing on the way out still matters for
the stops that are not hangs, where there is no kill to rely on.

**This is a second shape of the failure ADR-0008 rules out.** Not "the keyboard
stopped working" but "a key is stuck down", which is worse: a dead keyboard is
obvious and recoverable, a stuck modifier is neither. Whatever ends the process has
to let the device's keys go on the way out, and the kill it is being ended by is
one that runs no code — so the release has to happen while it still can.

Sending an all-released report from another process clears it, which is what makes
recovery possible at all: the device is shared, so anything that can reach the
service can put it back.

## The device enumerates with the identity its client gave it

Initialised with vendor `0x16c0` and product `0x27db`, the virtual keyboard turns
up in the registry as an ordinary keyboard reporting exactly those:

```
watching device 1 built_in=false vendor=Some(5824) product=Some(10203)
```

`5824` and `10203` are those two numbers in decimal. So a converter that captures
by enumeration **will find its own output device**, and on the way in it looks like
any other external keyboard.

That is worse than a loop. A run that delivers seizes what it captures, so it takes
its own output device exclusively and every converted keystroke comes back to
itself: nothing reaches an application at all, and the run looks healthy from the
inside — 597 reports written, 898 values read, no error anywhere, and not one
character typed. The empty slots of its own keyboard reports even come back as an
unnamed usage, `page 0x0007 usage 0x00`.

Identifying it by the numbers the client itself passes is what makes ignoring it
sound: a device left over from another client carries that client's identity
instead, which is why Karabiner-Elements' one is a different pair.

## A second client does not disturb the first

With one client holding a ready virtual keyboard, a second connected and initialised
one with a different vendor and product id. It reached `keyboard_ready true`, the
first client stayed connected, still typed, and the keyboard type stayed `3`.

## Not established

- Why the virtual keyboard's type is `3` here and `46` under Karabiner's own
  remapping. The country code is ruled out; nothing else has been varied.
