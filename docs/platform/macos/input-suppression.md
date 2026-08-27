# Input suppression

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-27 and
after, from unsigned `cargo`-built binaries in a terminal holding Input Monitoring,
several of them under `sudo`. Two keyboards were in play: the MacBook's internal one
and a Lenovo TrackPoint Keyboard II over Bluetooth.

The section below is the only one that is not a measurement: it is the SDK's
documented contract, read from

```
/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/System/Library/Frameworks/
  IOKit.framework/Headers/hid/{IOHIDKeys.h,IOHIDDevice.h,IOHIDManager.h}
```

## The HID layer offers exclusive access

`IOHIDKeys.h`:

```
kIOHIDOptionsTypeNone        = 0x00
kIOHIDOptionsTypeSeizeDevice = 0x01
kIOHIDOptionsTypeMaskPrivate = 0xff0000
```

with `kIOHIDOptionsTypeSeizeDevice` documented as:

> Used to open exclusive communication with the device. This will prevent the
> system and other clients from receiving events from the device.

That is what suppression means here, and it is reachable from the call this host
already makes. Both `IOHIDManagerOpen(manager, options)` and
`IOHIDDeviceOpen(device, options)` take it, the manager form applying to
"both current and future devices that are enumerated".

**The per-device form matters more than it looks.** Seizing through the manager
takes every keyboard at once, including the one the user would need in order to
recover; seizing one device leaves the others working.

## Seizing is refused without privilege

`IOHIDDeviceOpen(trackpoint, kIOHIDOptionsTypeSeizeDevice)` from an unsigned,
unbundled process holding Input Monitoring returned **`0xE00002C1`**, which is
`kIOReturnNotPrivileged` — the value read back by compiling against
`<IOKit/IOReturn.h>` and printing the constants, not from recall. The manager had
been opened without the seize option, and only the external keyboard was
targeted; the internal one was never touched.

The error matters for being that one specifically. It is not
`kIOReturnUnsupported` and not `kIOReturnExclusiveAccess` (`0xE00002C5`, which is
what `IOHIDManagerOpen` returned while Karabiner held the keyboards). So the call
is supported and the device was available — **what was missing was privilege.**

`IOHIDDeviceClose` then returned `kIOReturnSuccess` even though the open had
failed.

Karabiner's own `Karabiner-Core-Service` — the component that grabs devices —
runs as root in the system launchd domain rather than as a user agent, which was
visible in `ps` while its user agents were stopped. That is consistent with
grabbing needing privilege there too.

## Privilege clears that check; the next refusal is contention

The same call under `sudo`, with Karabiner-Elements running, returned
**`0xE00002C5`** — `kIOReturnExclusiveAccess` — for both the internal keyboard
and the external one.

**A different error is the result here.** Root is past the privilege check, and
what refuses the seize now is that something else already holds the devices
exclusively. That is corroborated by the plain, non-seizing `IOHIDManagerOpen`:
it returned `0xE00002C5` while Karabiner's user agents were running and
`0x00000000` once they were stopped, so the exclusivity travels with the agents
rather than with the root daemon alone.

## The seize succeeds, and does not suppress

Under `sudo` with Karabiner-Elements stopped, `IOHIDDeviceOpen(device,
kIOHIDOptionsTypeSeizeDevice)` returned **`kIOReturnSuccess`** for both the
internal keyboard and the external one. The three results together say which
obstacle each error code was:

| process | Karabiner | result |
|---|---|---|
| unprivileged | running | `kIOReturnNotPrivileged` |
| root | running | `kIOReturnExclusiveAccess` |
| root | stopped | `kIOReturnSuccess` |

**But a successful seize did not stop the keystrokes.** With both keyboards held
and a conversion running over them, typing `asdfg` in a terminal produced this,
interleaved:

```
would send A + Modifiers(0)
awould send O + Modifiers(0)
swould send E + Modifiers(0)
dwould send U + Modifiers(0)
fwould send I + Modifiers(0)
g
```

The converted keys are right — `asdfg` is `aoeui` in this layout — and the `a`,
`s`, `d`, `f`, `g` between them are the **physical** characters, echoed by the
tty as they were typed. So the application received the original keystrokes while
the device was held exclusively.

Two things follow:

- **Capture survives a seize.** The holding process still gets input values, so
  suppressing and converting can live in one process.
- **`kIOHIDOptionsTypeSeizeDevice` returning success is not suppression**, whatever
  its documented wording says about preventing the system from receiving events.
  Something else carries keystrokes to the window server on this version.

Seizing more devices does not help. A run that seized **every** HID device
sharing a keyboard's `LocationID` — 15 devices, every one returning
`kIOReturnSuccess`, including all four of the internal keyboard's — still let the
keystrokes through: 533 HID values reached the probe and 118 key events reached a
listen-only tap over the same 8 seconds.

## What Karabiner actually does

Read off `Karabiner-Core-Service` with `nm -u`, `strings` and `codesign`, since
it suppresses successfully on this machine and the mechanism was not guessable
from the outside.

**It has no entitlements at all** — `codesign -d --entitlements` prints none. So
nothing here is reachable only with a private grant.

**It imports no IOHIDManager symbol.** Its capture path is:

```
IOServiceMatching / IOServiceGetMatchingServices / IOServiceAddMatchingNotification
IORegistryEntryGetRegistryEntryID / IORegistryEntryCreateCFProperty
IOHIDDeviceCreate            -- from the io_service_t, not from a manager
IOHIDDeviceOpen / IOHIDDeviceClose
IOHIDQueueCreate / IOHIDQueueAddElement / IOHIDQueueStart
IOHIDQueueCopyNextValueWithTimeout / IOHIDQueueRegisterValueAvailableCallback
```

Its own symbol names say seizing is the suppression mechanism:
`device_grabber_details::entry::needs_to_seize_device()`,
`iokit_hid_queue_value_monitor::seized()`,
`device_grabber::update_devices_disabled()`.

**So the seize is right and the way in is not**, and that is the whole of it. The
same seize, on the same devices, through the registry instead of a manager:

```
IOServiceMatching("IOHIDDevice") / IOServiceGetMatchingServices / IOIteratorNext
IOHIDDeviceCreate(NULL, io_service_t)
IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice)     -> kIOReturnSuccess
IOHIDDeviceCopyMatchingElements
IOHIDQueueCreate / IOHIDQueueAddElement / IOHIDQueueStart
IOHIDQueueRegisterValueAvailableCallback / IOHIDQueueCopyNextValueWithTimeout
```

Over 8 seconds of typing on both keyboards: **802 HID values to the queue, 0 key
events to a listen-only tap**, and nothing echoed into the terminal.

**`IOHIDManager` is what breaks the seize.** It opens every device
non-exclusively before any per-device seize can be asked for, so the seize
becomes a second open on a device already held and takes hold of nothing — while
still returning `kIOReturnSuccess`. Taking the `io_service_t` from the registry
and creating the device by hand makes the seize the only open there is, and then
it means what it says.

The capture path moves with it: values arrive from an `IOHIDQueue` per device
rather than from a manager's input-value callback. The queue's callback reports
only that something is available, so it is drained with
`IOHIDQueueCopyNextValueWithTimeout` at a zero timeout until empty.

**The registry path cannot coexist with another remapper.** While Karabiner
holds the keyboards, `IOHIDDeviceOpen` on a registry-created device is refused
with `kIOReturnExclusiveAccess` **even without the seize option** — a plain open
fails too. An `IOHIDManager` in the same situation opens happily and delivers
values, which is why the manager route could convert Karabiner's output and this
one cannot. Two remappers sharing a keyboard was never wanted, but it does mean
the two are now mutually exclusive rather than merely inadvisable.

One hypothesis this rules out: `IOHIDServiceClientSetProperty` is **not** the
suppression lever, despite being the most suggestive symbol in the list. The
binary's log format string `hid_event_system_monitor set_caps_lock_delay_override
for {0}` places it on the caps-lock delay instead.

## The private event-system symbols are reachable

Every name taken from Karabiner's imports resolved through
`dlsym(RTLD_DEFAULT, …)` in a plain build: `IOHIDEventSystemClientCreate`,
`IOHIDEventSystemClientCreateSimpleClient`, `IOHIDEventSystemClientCopyServices`,
`IOHIDEventSystemClientSetMatching`,
`IOHIDEventSystemClientRegisterEventCallback`,
`IOHIDServiceClientSetProperty`, `IOHIDServiceClientCopyProperty`,
`IOHIDServiceClientConformsTo`.

They appear in no SDK header, so using them means naming them by hand and
accepting that a release may move them.

Device ids were handed out in a different order between two runs of the same
program, so enumeration order is not stable and nothing may key off it.

## The seize ends when the process is killed

The question
[ADR-0008](../../adr/0008-input-suppression-and-watchdog.md) turns on, since
`SIGTERM` and `SIGKILL` run no destructor and nothing on favjit's side closes the
device.

Run under the watchdog with `--dry-run false --skip-built-in`, wedged on purpose after
five keystrokes on the seized external keyboard:

```
watching device 0 built_in=false vendor=Some(6127) product=Some(24801), held exclusively
foo                                     <- converted, while seized
wedged on purpose; the watchdog should kill this shortly
no heartbeat for 2.12990725s: killing pid 29200 so the keyboard comes back
```

and then, on the same keyboard, `barbaz` typed normally. **The platform releases
the seize when the process dies**, so the watchdog killing a wedged favjit does
give the keyboard back — the failure degrades to "favjit stopped working" rather
than "the keyboard stopped working", which is the property ADR-0008 asks for.

A seized device also keeps delivering values to the client that holds it: the
`foo` above was captured and converted under the seize.

## A seized device delivers its pointer too

The TrackPoint Keyboard II carries its keyboard and its pointer as elements of one
node ([hid-device-enumeration.md](hid-device-enumeration.md)), so a seize takes the
pointer with it. What the queue then carries decides whether the pointer can be
relayed at all. With that keyboard seized and the internal one left alone, watching
every element and naming no key:

```
watching device 0 built_in=false vendor=Some(6127) product=Some(24801), held exclusively
  device 0 page 0x0001 usage 0x30 (48)      <- X
  device 0 page 0x0001 usage 0x31 (49)      <- Y
  device 0 page 0x0001 usage 0x38 (56)      <- wheel
  device 0 page 0x0009 usage 0x01 (1)       <- button 1
  device 0 page 0x0009 usage 0x02 (2)       <- button 2
  device 0 page 0x0009 usage 0x03 (3)       <- button 3
```

Two runs, because scrolling was not exercised in the first: the wheel came from the
second, where scrolling on this keyboard also produced button 3 — its middle button
is held to scroll.

So **the same queue that suppresses the keyboard delivers the pointer's motion,
wheel and buttons**, and a relay has everything it needs at the capture end. That is
also the set a virtual pointing device's report carries
([virtual-hid-device.md](virtual-hid-device.md)): buttons, x, y, vertical and
horizontal wheel.

## Getting the privilege is a one-time install, not a password each launch

Karabiner-Elements needs root for the same reason and does not ask for a password
at launch. Its `Karabiner-Core-Service` runs as `root`, visible in
`ps -o pid,user,command`, and so does its `VirtualHIDDevice-Daemon`.

The privilege comes from a launchd daemon registered by an `SMAppService` helper,
not from `/Library/LaunchDaemons/` — that directory holds nothing of theirs. The
plists live inside a helper app bundle:

```
/Library/Application Support/org.pqrs/Karabiner-Elements/
  Karabiner-Elements Privileged Daemons v2.app/Contents/Library/LaunchDaemons/
    org.pqrs.service.daemon.Karabiner-Core-Service.plist
    org.pqrs.service.daemon.Karabiner-VirtualHIDDevice-Daemon.plist
```

The Core Service one is four keys:

```
Label            = org.pqrs.service.daemon.Karabiner-Core-Service
ProgramArguments = [ .../Karabiner-Core-Service.app/Contents/MacOS/Karabiner-Core-Service ]
KeepAlive        = true
ProcessType      = Interactive
```

The helper bundle is `LSUIElement = true`, and
`launchctl print-disabled system` reports both labels as `enabled` — readable
without privilege, which is how this was checked.

A root LaunchDaemon and a DriverKit system extension are separate things: only the
second involves signing, notarisation and an approval in System Settings' extension
panes. Karabiner-Elements has both — it suppresses through a DriverKit driver of its
own and injects through a DriverKit virtual HID keyboard, and both were observed
running here (see [hid-device-enumeration.md](hid-device-enumeration.md)) — so
needing root says nothing about needing a driver.

## Not established

- Whether a device seized by a process that is suspended rather than killed comes
  back, which is the same question for a machine that sleeps mid-run.
- Whether an entitlement rather than root would do it.
- What happens to a seize when the device disconnects and returns, which for a
  Bluetooth keyboard is routine.
- Whether the horizontal wheel reports at all on this keyboard. Only page 0x01
  usage 0x38 appeared, and the pointing report has a field for both.
