# Pointer acceleration

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-29.

## How far a count carries the cursor is a property of the device

Every pointing device the HID event system knows about carries two properties that
decide how far its reports move the cursor. Read through
`IOHIDEventSystemClientCreate` → `IOHIDEventSystemClientCopyServices` →
`IOHIDServiceClientCopyProperty`, and written with
`IOHIDServiceClientSetProperty`. All four are exported from IOKit — they are in the
SDK's `IOKit.tbd` — and they worked as the console user, with no root and no
entitlement.

Both are `IOFixed`: a **32-bit** integer of 65536ths.

- `HIDPointerResolution` — counts per inch. **A lower number is a faster cursor**:
  the OS divides a device's counts by its resolution to get a distance, so a device
  claiming fewer counts per inch is claiming each count is further.
- the acceleration factor — but *which* property holds it differs per device.
  `HIDPointerAccelerationType` names it. On this machine the TrackPoint keyboard and
  the virtual *pointing* device said `HIDMouseAcceleration`, while the virtual
  *keyboard* device said `HIDPointerAcceleration`. Writing the other name is accepted
  and changes nothing.

What the machine reported before anything was changed:

| Device | Resolution | Acceleration |
|---|---|---|
| TrackPoint Keyboard II (6127/24801) | 400 dpi | 0.6875 in `HIDMouseAcceleration` |
| Karabiner virtual keyboard | 400 dpi | 0.6875 in `HIDPointerAcceleration` |
| Karabiner virtual pointing device | 400 dpi | 0.6875 in `HIDMouseAcceleration` |
| Apple Internal Keyboard / Trackpad | 400 dpi | none |

So 400 dpi and 0.6875 are what a device gets here by default.

**A resolution written on its own does not take effect.** An acceleration has to be
written after it; writing back the value the device already had is enough.
LinearMouse's source does the same and says as much.

The virtual devices are two separate services, one for the keyboard and one for the
pointing device, and they carry the vendor and product of whichever client
initialised them: 5824/10203 and 5824/10202 while favjit is running, and 1452/591
after Karabiner's own client has had them
([virtual-hid-device.md](virtual-hid-device.md)). The one whose properties move the
cursor is the pointing device.

## They do not survive the device being initialised again

A run that had set 80 dpi and 0.8 was replaced by another, and the new run found the
defaults back — 400 dpi and 0.6875 — before writing its own.

## The ranges

LinearMouse clamps resolution to 10–1995 and acceleration to 0–40, with -1 meaning
linear scaling. Those bounds are from its source, not from anything measured here;
values inside them were accepted, and nothing outside them was tried.

## What LinearMouse's pointer speed is in these units

Its speed of 0 to 1 maps to a resolution by
`1 / (speed × (1/40 − 1/1200) + 1/1200)`, so a speed of 0.8 is about 49.6 dpi —
eight times the default's distance per count. From its source, like the ranges
above.

## Not established

- Whether they survive a reboot with the device left alone. Nothing depends on the
  answer, since they are written at every start anyway.
- What the acceleration factor means numerically. Only its direction is known:
  larger accelerates more.
