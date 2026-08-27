# ADR-0019: Convert the Mac keyboard's function row into the controls its icons stand for

- **Status**: Accepted
- **Date**: 2026-09-01

## Context

The MacBook's top row is printed with brightness, volume and media icons, and the keyboard does not send any of them: pressing those keys produces `F1`, `F2`, `F11` and `F12` on the keyboard page, measured from favjit itself while it held the keyboard ([hid-input-callbacks.md](../platform/macos/hid-input-callbacks.md)). The icons are the OS's reading of the row of its own keyboard.

Two things follow. A run that seizes that keyboard and drops what its tables cannot name loses the whole row — which is what happened: the keys did nothing at all. And handing the key back as `F1` through the virtual device does not restore the icons, because the reading belongs to the keyboard the OS knows, not to a virtual keyboard favjit created.

The output device can say the controls directly. Beside its keyboard report it declares one report per page for consumer, Apple vendor top case, Apple vendor keyboard and generic desktop, each carrying 32 sixteen-bit usages ([virtual-hid-device.md](../platform/macos/virtual-hid-device.md)).

Karabiner-Elements, which held this keyboard before favjit did and whose configuration this layout was ported from, does not rely on the OS for any of this either: its `make_default_fn_function_keys_json` maps `f1` to the consumer `display_brightness_decrement` and so on for all twelve, and it maps `fn`+`f1` to the same controls in the other direction of the "use F1, F2 as standard function keys" preference. Reproducing the row in full is what it does.

## Decision

`F1` to `F12` are keys in favjit's vocabulary, and twelve controls are too — brightness, Mission Control, Spotlight, dictation, do-not-disturb, the transport keys, mute and volume — each named for what it does. The layout converts the row to those controls on the Mac's own keyboard, with the same pairing Karabiner uses by default. Injection sends a control on the output device's report for its page.

The pairing of control to HID usage is one table in the macOS host, walked in both directions, so a keyboard that sends a control itself reads as the same key that the layout can emit.

## Consequences

- The row does what its icons say while favjit holds the keyboard, without depending on how the OS treats a virtual keyboard's function keys.
- An external keyboard's top row passes through as `F1` to `F12`. That row is printed `F1`, and the rule that converts is scoped to the built-in keyboard.
- **There is no way to press a plain `F1` on the built-in keyboard.** Karabiner leaves `fn`+`f1` for that; here `Fn` is converted to a command key before any rule could match it, so this costs the function keys on that keyboard until the layout gives them somewhere else to live.
- The row is converted in every layer, since no layer covers the top row. A layer that wanted to could take a position back by adding a rule ahead of these.
- A control's modifiers ride on the keyboard report, the descriptor declaring those eight bits there and nowhere else, so a modified control is two reports — the modifier down first and let go last, as for a shifted character.
- Twelve controls is a table, and a keyboard with a thirteenth (a calculator key, say) still has it dropped and reported as an unnamed usage. Closing that gap is adding a name, which is what `favjit --usages` exists to make findable.
- Nothing reads the "use F1, F2, etc. keys as standard function keys" preference. The row is the controls unconditionally, which is that preference's default and what this machine has.

## Alternatives considered

### Relay whatever the keyboard sends on the control pages, unconverted

Attractive because it needs no names: capture what arrives on the consumer and Apple pages, hand it back on the matching report, and a keyboard whose function row favjit has never seen works anyway. It cannot fix this, and measurement is why — **that row sends nothing on those pages.** There is nothing to relay, and a relay of the `F1` it does send is an `F1`.

### Hand the function key back as a function key and let the OS read the row

One line, and it is what the layout would do by leaving the row alone. Rejected on the evidence above: the reading is of the OS's own keyboard, and Karabiner — which suppresses this keyboard successfully on this machine — reproduces the row itself rather than relying on that reading in either direction of the preference.

### Convert the row on every keyboard, as Karabiner's profile-wide default does

It is what this machine did before favjit, so it has the weight of the setup being reproduced. Not taken because the two rows are printed differently: the icons are on the MacBook's keys, and an external keyboard's `F1` is labelled `F1`. This is a scope, so a rule can be widened later without anything else moving.

### Name the controls after their HID usages rather than after what they do

`ConsumerDisplayBrightnessDecrement` says exactly what goes on the wire. Rejected because it puts a page in the layout's vocabulary: the twelve controls live on three different pages, two of them Apple's own, and what a rule wants to say is "brightness down".
