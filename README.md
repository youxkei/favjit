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

The macOS side works. Keys are captured per keyboard, the built-in one and a
Bluetooth keyboard get their own rules, the original keystroke is suppressed
rather than arriving alongside the converted one, and the converted input goes out
as a virtual HID device — which is also what lets the TrackPoint keyboard's pointer
be relayed, since suppressing that keyboard takes its pointer with it. Held keys
repeat. All of it has been checked on hardware.

The MacBook's top row is converted to the controls its icons stand for
([ADR-0019](docs/adr/0019-convert-the-function-row-to-its-icons.md)): that row sends
`F1` to `F12` and nothing else, and the brightness and volume the icons promise are
the OS's reading of its own keyboard — which a key taken from that keyboard no
longer gets. So favjit sends the control itself, on the output device's report for
its page, and the row works with the keyboard held. An external keyboard's top row
is printed `F1` and is left alone.

`favjit` still runs as a dry run unless told otherwise: it converts for real and
sends nothing, so a bare command changes nothing outside its own process. One flag
decides the mode — `--dry-run false` is the run that takes the keyboards exclusively
and injects — so there is no combination to assemble wrongly, and the two that would
have landed on the keyboard you are typing on cannot be asked for.

The relayed pointer is favjit's too: the wheel can be turned over, and how far the
cursor travels is set on the output device itself rather than by scaling what the
hardware said, so the machine's own acceleration curve is what you feel.

`sudo favjit --install` puts it in place as a launchd daemon supervised by the
watchdog, so it converts from boot, and registers a menu bar item for turning
converting off and on without a password — which is the escape for a favjit that is
alive and converting wrongly. On a menu bar with no room to spare the item is not
drawn, and macOS gives no way to insist; `favjit --disable` does the same thing from
a terminal.

Installing asks for Accessibility, which is what lets a daemon read the keyboards.
favjit signs itself ad hoc, so that grant does not survive an update: an install
clears it and asks again, and answering the dialog is all it takes — the converter
retries until the answer changes.

The macOS end of the link is there too: it advertises itself over mDNS and accepts
input from one paired machine, over a Noise session on TCP. An unpaired source is
refused before a single keystroke is read, and `--no-listen` is how a machine is asked
not to serve the link at all.

The Windows side is what connects to that link. It reads the keyboards and mice
through a low-level hook and the mouse through raw input, which is where a mouse's
actual movement is. The hook both reads a key and refuses it, in that order, because
that is the only order in which both can happen on Windows: a key it turns down reaches
nothing at all, favjit included. Keys are read by position rather than by the character the Windows layout
would produce, so switching layouts there moves nothing. It finds the Mac over mDNS
with no address to configure.

**A chord moves the keyboard between the two machines**
([ADR-0018](docs/adr/0018-a-chord-moves-the-keyboard.md)): option and `n` sends it to
the Mac, option and `s` brings it back. While it is the Windows machine's, that chord
is the only thing refused there — everything else is typed where it was typed — and
coming back releases whatever the Mac still believes is held, so no modifier is left
down on the machine you just left.

`favjit` on Windows is a dry run too, and one flag decides the mode there as well:
`--dry-run false` opens the link and forwards, refusing this machine's own copy of
what it sends. Relaying without refusing would put every keystroke on both screens,
so it is not a thing that can be asked for.

Pairing is the exchange [ADR-0004](docs/adr/0004-peer-authentication.md) decides, and
both halves are here: the Mac shows six digits and serves exactly one attempt, the
Windows machine is given those digits and spends them once, and the two static keys
cross under the secret they agree on. Nothing is switched off for it, because pairing
is advertised under a name of its own: `sudo favjit --pair` on the Mac and
`favjit --pair <those digits>` on the other machine is the whole of it. Nothing has
answered a code on hardware yet.

Both machines are supervised. `favjit-watchdog` starts favjit, probes it through a
pipe and requires the answer to come back out of the loop itself, so a wedge ends the
process and the keyboards come back rather than staying refused — which is what
[ADR-0008](docs/adr/0008-input-suppression-and-watchdog.md) asks for wherever
suppression is held. The judgement it makes is one piece of code for both platforms
and the end-to-end suite drives it; what is per machine is the pipes and the way to
end a process.

The driver package favjit sends its output through has to be installed separately,
and the Windows side has no equivalent of `--install`, so both it and its watchdog
are started by hand.

## Documentation

Architecture decisions and the reasoning behind them live in [docs/adr/](docs/adr/).

Platform-specific behavior of Windows and macOS lives under [docs/platform/](docs/platform/).

## Development

Rust, one cargo workspace under [crates/](crates/).

```
cargo test --workspace
```

On the Mac. `bin-macos` names its host unconditionally, so the workspace as a whole
builds only there; on the Windows machine the crates are named instead. `core`, the
simulator and the suite run on either machine, and each platform's own host is
compiled away on the other one, so the parts behind a platform gate — the Win32
structures, above all — are checked by building for that platform:

```
cargo test --target <the other machine's target> -p favjit-host-windows -p favjit-bin-windows -p favjit-bin-watchdog
```

What is installed is built per package rather than by building the workspace, because
both binaries are named `favjit` and one output path cannot hold two of them — cargo
says so and carries on, leaving whichever was compiled last:

```
cargo build --release -p favjit-bin-macos -p favjit-bin-watchdog -p favjit-bin-menu
```

The end-to-end suite is where the intended behaviour is written down: press a
key on a given keyboard, assert what reaches applications.
