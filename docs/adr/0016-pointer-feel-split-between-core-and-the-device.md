# ADR-0016: Turn the wheel over in `core` and set the speed on the output device

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

The TrackPoint keyboard's pointer comes out through favjit, because seizing its keys
takes its pointer with it ([ADR-0011](0011-macos-output-through-a-virtual-hid-device.md)).
Relayed as it stands, it is slower than the same hardware was under the tool favjit
replaces, and its wheel scrolls the wrong way.

Two things are being decided, and they are not the same kind of thing:

- **Which way the wheel turns** is a conversion: the report says one thing and the
  machine should be told another. macOS has a single scroll direction switch for
  every device, so a device relayed through favjit cannot have its own that way.
- **How far the pointer travels** is not a conversion. macOS scales a pointing
  device by two properties of the device — its resolution and its acceleration — and
  the default here is 400 dpi with a factor of 0.6875
  ([docs/platform/macos/pointer-acceleration.md](../platform/macos/pointer-acceleration.md)).
  The configuration being replaced amounted to about 49.6 dpi.

## Decision

`core` inverts the wheel, as part of the one conversion pipeline
([ADR-0003](0003-unified-conversion-pipeline.md)), with an axis at a time.

Speed and acceleration are set on favjit's own output device through the HID event
system, after the device comes up and on every run. `--pointers` reads and writes
them from the command line, so a number can be tried without an install.

## Consequences

The end-to-end suite pins the direction of the wheel, which is where a mistake would
be silent and constant. Nothing in the suite covers the speed, and nothing could: it
is a property of a device on a machine.

Movement reaches macOS exactly as the hardware reported it, so the OS's own curve is
what a person feels — slow movement stays precise. A multiplier in the pipeline would
have had to be about eight times to reach the speed asked for, and would have
multiplied the single-unit reports a TrackPoint mostly makes into steps.

The properties belong to the device and are lost when it is initialised again, so
they are written at every start rather than once at install. What they were set to
lives in the launchd job's arguments, where it can be read.

There is one place to change the speed and one place to change the direction, and
neither can fight the other.

## Alternatives considered

### A multiplier on the relayed reports

Scale `dx` and `dy` in the sink, which the end-to-end suite could pin exactly. **Not
taken**: reaching the speed asked for needs a factor around eight, which quantises
the single-unit reports a TrackPoint mostly makes, and it stacks with the OS curve
rather than replacing it — two speed controls, the coarser one winning.

### Setting the properties on the TrackPoint keyboard instead

Tune the device the movement comes from. **Not taken**: that device is seized, so
nothing but favjit sees its reports — what macOS accelerates is the virtual device
they arrive on.

### Leaving both to the system's own settings

Ask the person to raise the tracking speed in System Settings. **Not taken**: it
moves every pointing device on the machine, and the TrackPoint is the one that needs
it.

### A settings file the converter re-reads

Somewhere to keep the numbers without an install. **Not taken**: they are settled
once by feel and then left alone, and the launchd job's arguments already say what is
in force. `--pointers` covers trying a value.
