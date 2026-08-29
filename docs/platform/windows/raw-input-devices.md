# Raw input's device list

Observed on **Windows 11 25H2, build 10.0.26200.9168**, with `favjit --devices`,
which calls `GetRawInputDeviceList` and then `GetRawInputDeviceInfoW` with
`RIDI_DEVICENAME` for each entry.

## One keyboard is several devices

A single physical keyboard appears in the list **more than once**, one entry per
HID collection. Three entries of this shape are one keyboard:

```
keyboard  \\?\HID#VID_xxxx&PID_yyyy&MI_01&Col01#…#{884b96c3-…}
keyboard  \\?\HID#VID_xxxx&PID_yyyy&MI_02#…#{884b96c3-…}
mouse     \\?\HID#VID_xxxx&PID_yyyy&MI_00#…#{378de44c-…}
```

The `MI_nn` is the USB interface and `Col_nn` the HID collection inside it. A
keyboard that also carries media keys, or a mouse, publishes each as its own
collection, and raw input hands out a device handle per collection rather than per
piece of hardware.

**Every one of them carries the same vendor and product.** So a rule matched on the
pair applies to all of a keyboard's collections, which is what makes per-device
layout rules work here — but a device handle, and therefore a `DeviceId` from this
host, is a collection and not a keyboard.

## Input reaches a message-only window with no foreground

A window created with `HWND_MESSAGE`, registered for usages 1/6 and 1/2 with
`RIDEV_INPUTSINK | RIDEV_DEVNOTIFY`, receives keystrokes typed into whatever does
have the foreground. Which is what the flag is for, and why the window is never
shown.

## The device on an event can be nothing

Keystrokes arrive with `RAWINPUTHEADER.hDevice` set to **zero**, which is Windows
saying the input came from no device: `GetRawInputDeviceInfoW` has no path for such
a handle, so there is no vendor and product on it either. What decides whether an
event carries a device is not known.

A device that arrives this way still converts — it is announced with neither id,
which is the case [`DeviceInfo`](../../../crates/core/src/device.rs)'s optional ids
exist for — but no rule can single it out, so it takes the layout's scope for an
unclaimed external keyboard rather than the one a vendor and product would put it
in.

## The vendor and product are in the path, not in the device info

`RIDI_DEVICEINFO` fills in `RID_DEVICE_INFO`, whose union carries a vendor and a
product only in its `RID_DEVICE_INFO_HID` shape. A keyboard gets
`RID_DEVICE_INFO_KEYBOARD` instead — type, subtype, function key count, number of
indicators — and none of that identifies the hardware.

The identity is in the interface path from `RIDI_DEVICENAME`, as `VID_xxxx` and
`PID_xxxx`, in upper-case hex on every entry seen. A path for a device that is not
on a USB bus carries neither: a laptop's own keyboard is behind `ACPI#PNP0303`,
which names a class rather than a product.
