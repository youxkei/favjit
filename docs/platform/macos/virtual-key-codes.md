# Virtual key codes

Read from the SDK header on 2026-08-27, on macOS 26.6.2 (build 25G83):

```
/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/System/Library/Frameworks/
  Carbon.framework/Frameworks/HIToolbox.framework/Headers/Events.h
```

This is a header rather than a hardware observation, so it says what constants
exist, not what any keyboard reports. One value was cross-checked against a live
`CGEventTap`: posting `kVK_F16` (`0x6A` = 106) produced an event whose field 9
read 106 (see [event-tap.md](event-tap.md)).

## Japanese and ISO keys that do exist

| Constant | Value |
|---|---|
| `kVK_ISO_Section` | `0x0A` |
| `kVK_JIS_Yen` | `0x5D` |
| `kVK_JIS_Underscore` | `0x5E` |
| `kVK_JIS_KeypadComma` | `0x5F` |
| `kVK_JIS_Eisu` | `0x66` |
| `kVK_JIS_Kana` | `0x68` |

## The PC-JIS keys have no virtual key code

Searching the header for `nfer`, `xfer`, `katakana`, `henkan`, `muhenkan` and
`hiragana` returns nothing. There is no `kVK_` constant for 無変換, 変換 or
カタカナひらがな — the three keys a PC-JIS keyboard carries and an Apple-JIS one
does not.

**This bounds what a CGEvent-level implementation can do.** Those keys are named
in the Karabiner configuration favjit's layout was ported from — 変換 holds the
Henkan layer on the Dudrack-typed external keyboard, 無変換 is shift there, and
on a raw-JIS keyboard 無変換 and 変換 become 英数 and かな. A layer built on
`CGEventGetIntegerValueField(event, 9)` has no value to match those presses
against, and none to emit.

Karabiner reaches them below CGEvent, at HID usage level, through its own
DriverKit virtual device.

## Not established

- Whether they reach a CGEventTap as anything at all rather than not at all. Which
  HID usages they report is settled — each was pressed alone and read back
  ([hid-input-callbacks.md](hid-input-callbacks.md)).
- Whether `kVK_ISO_Section` (`0x0A`) is what a JIS keyboard's `]`/`}` position
  reports, or whether that position has no virtual key code either.
