# favjit

Drive macOS from the Windows keyboard and mouse, and apply keyboard layout conversion to every key event macOS receives — in one place.

## The problem

Sitting in front of a Windows machine and a Mac, two separate annoyances show up:

1. **Input devices are tied to a machine.** Touching macOS means moving your hands to the Mac's keyboard.
2. **Keyboard layouts differ per machine.** Modifier positions and symbol placement never line up between Windows and macOS.

favjit treats these as one problem. Windows input is forwarded to macOS, and every key event arriving on macOS — forwarded or locally generated — goes through the same conversion rules.

## Topology

```
  Windows                                    macOS
┌──────────────┐                     ┌──────────────────────────────┐
│ keyboard     │                     │  built-in keyboard           │
│ mouse        │                     │  Bluetooth keyboard(s)       │
└──────┬───────┘                     └──────────────┬───────────────┘
       │ capture                                    │ capture
       ▼                                            ▼
┌──────────────┐    network      ┌──────────────────────────────────┐
│    source    │ ──────────────▶ │  sink                            │
└──────────────┘                 │  ┌────────────────────────────┐  │
                                 │  │ layout conversion pipeline │  │
                                 │  └─────────────┬──────────────┘  │
                                 │                ▼                 │
                                 │            injection             │
                                 └──────────────────────────────────┘
```

Input flows one way: Windows → macOS. Driving Windows from the Mac's keyboard is out of scope.

The conversion pipeline lives on the macOS side only, and both forwarded and local input pass through it. Keeping the rules in exactly one place is the point.

## Scope

In scope:

- Forwarding Windows keyboard and mouse input to macOS
- Layout conversion for the Mac's built-in keyboard
- Layout conversion for Bluetooth keyboards connected to the Mac
- Layout conversion for key events forwarded from Windows

Out of scope:

- Driving Windows from macOS input (reverse direction)
- Linux support
- More than two machines
- Clipboard sharing, file transfer, or anything other than input
- Screen sharing or streaming

## Status

This repository holds design documents. There is no implementation.

## Documentation

Architecture decisions and the reasoning behind them live in [docs/adr/](docs/adr/).

Platform-specific behavior of Windows and macOS lives under [docs/platform/](docs/platform/).

## Development

The implementation language is Rust ([ADR-0001](docs/adr/0001-use-rust.md)), and the
workspace layout is settled ([ADR-0005](docs/adr/0005-crate-layout.md)).
