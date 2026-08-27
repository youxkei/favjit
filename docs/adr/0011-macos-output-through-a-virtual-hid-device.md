# ADR-0011: On macOS, send output through a virtual HID device rather than synthesised events

- **Status**: Accepted
- **Date**: 2026-08-28

**This ADR is about macOS only.** The decision is scoped to the sink's output path
on that platform: which OS mechanism carries a converted keystroke or a relayed
pointer movement to the rest of the system. The Windows side captures and forwards
and has no output path of this kind, and nothing here constrains it.
[ADR-0003](0003-unified-conversion-pipeline.md)'s single pipeline is unaffected —
this is what the end of that pipeline writes to.

## Context

`CGEventPost` carries every keystroke this layout emits, and it has been measured
doing so: it reaches every tap, fires flag-only shortcuts, enters a Secure Keyboard
Entry field, and unlocks the lock screen
([event-injection.md](../platform/macos/event-injection.md)). What it cannot do is
answer for the pointer, and the reason is not a detail.

A seize is per device, and the TrackPoint Keyboard II is one device — every
registry node for it reports keyboard as its primary usage, with the pointer on
further collections. Suppressing its keyboard takes its TrackPoint with it,
observed as a dead cursor while favjit held it. So a converter for that keyboard
has to relay the pointer, and relaying it through `CGEventPost` means reproducing
the system's acceleration. That is a curve, measured through a virtual device at 25
reports per run:

| per report | asked | cursor moved | ratio |
|---|---|---|---|
| `x = 1` | 25 | 98.73 | 3.95 |
| `x = 8` | 200 | 1677.44 | 8.39 |
| `CGEventPost`, absolute position | any | the amount asked | 1.00 |

Three further facts are established, all in
[virtual-hid-device.md](../platform/macos/virtual-hid-device.md):

- **The virtual HID device service admits any root client.** An unsigned probe
  under `sudo`, built against the driver's published client library, reached
  `driver_connected` with both a virtual keyboard and a virtual pointing device
  ready. Admission is by the socket's directory permissions and nothing else — no
  check of the client's signature.
- **A keystroke through it arrives while Secure Keyboard Entry is held**, including
  by the receiving process itself.
- **`LMGetKbdType()` follows it.** Read in a freshly spawned unprivileged process
  it went from `91` to `3` across a virtual keystroke, and `3` translates like ANSI
  where the JIS type puts `+` on the shifted `0x29` this layout's `:` comes from.

The daemon that owns the service is a launchd daemon of its own and keeps running
when Karabiner-Elements is stopped, which is what makes depending on it separable
from the program favjit replaces.

## Decision

**On macOS, favjit's output goes out as HID reports to the DriverKit virtual HID
device**, keyboard and pointing, over the root-only service socket. Suppression
stays a seize ([ADR-0008](0008-input-suppression-and-watchdog.md)).

favjit depends on the `Karabiner-DriverKit-VirtualHIDDevice` package being
installed. It does not depend on Karabiner-Elements, which must in fact be stopped,
since two remappers cannot both hold the keyboards
([ADR-0005](0005-crate-layout.md)'s host is where that exclusion lives).

## Consequences

The pointer becomes relayable at all, and it arrives accelerated by the OS instead
of by favjit's reproduction of a curve. The TrackPoint keyboard becomes usable
under suppression, which is the gap that blocks daily use.

A password typed in the converted layout arrives converted. This is the one place
where the alternatives differ in kind rather than in cost, and it is decided by
measurement rather than by preference.

A consumer that resolves a chord by character agrees with the key codes favjit
sends, because the device favjit sends through is the one `LMGetKbdType()` answers
with.

It commits us to a third party's system extension as an install-time prerequisite.
favjit itself stays a plain binary, so [ADR-0001](0001-use-rust.md)'s distribution
survives, but "install favjit" now means "install that package too", and its
protocol version is a compatibility surface favjit does not control.

It also commits us to that protocol's transport. The request buffer is
`[u16 version][u8 request][payload]`, but it is handed to a request/response layer
with correlated ids and heartbeats, and the virtual keyboard's readiness arrives as
a response — a client that writes only the documented bytes to the socket posts
reports into nothing. Either that layer is reimplemented in Rust or the driver's
header-only C++ client is linked, bringing `asio`, `nod`, `type_safe` and `spdlog`
with it. The host is where this lives, and hosts are allowed to be thick
([ADR-0006](0006-host-boundary.md)) precisely because what is inside one is only
reachable by real-hardware testing.

Root is required, which it already was for the seize.

## Alternatives considered

### A consuming `CGEventTap`, with `CGEventPost` for output

Swallow the physical keystroke by returning `NULL` from the callback instead of
seizing the device. Measured to work: 29 physical presses swallowed, 29
replacements typed, no leak and no tap disabled in 20 s. It needs no root and no
driver, leaves a pointer on the same device untouched, could run as a LaunchAgent,
and degrades by the platform disabling a slow tap — unconverted keystrokes flow,
which is the direction [ADR-0008](0008-input-suppression-and-watchdog.md) asks for.

**Not taken because it suppresses nothing in a password field.** With Secure
Keyboard Entry held by another process the same tap received zero events across a
window where the same prompt had produced fifteen: five replacements typed, then
five raw keystrokes straight through, `ooooosssss` on screen
([event-tap.md](../platform/macos/event-tap.md)). A password typed by muscle memory
in the converted layout would arrive as the raw one. It also cannot relay a
pointer faithfully, since its output is still `CGEventPost`.

### `CGEventPost` with a seize, and the pointer reproduced

Keep the path that already works for the keyboard and compute the acceleration
curve from `HIDPointerAcceleration`. **Not taken**: that parameter is a coefficient
and the behaviour is a curve, per the ratios above, so this is a reimplementation
of an OS behaviour with no way to verify it beyond feel — and it would still leave
the seized TrackPoint dead until the reproduction were finished.

### favjit's own virtual device through `IOHIDUserDevice`

Would remove the dependency entirely. `IOHIDUserDeviceCreate` and its neighbours
have no SDK header but are exported by the shipped IOKit, and the mechanism is in
use on this machine — the TrackPoint keyboard itself appears in the registry as an
`IOHIDUserDevice`. **Not taken because it is refused.** Called with a report
descriptor taken from a virtual keyboard that does work, it returns NULL as the
user and as root alike, and the framework logs a ref that bound to nothing.

### favjit's own DriverKit extension

The mechanism the working device uses. **Not taken**: it requires signing,
notarisation and an Apple-granted entitlement, which ends the plain-binary
distribution [ADR-0001](0001-use-rust.md) rests on, in exchange for a device
already installed on the machine.
