//! Set-1 make code → [`Key`], for keys arriving from a keyboard.
//!
//! The make code is the position the key sits in, which is the vocabulary
//! [`Key`] is written in — the virtual key beside it in every Windows input
//! structure is the character the *current* input locale would produce, so a
//! layout switch on the Windows side would move every rule the sink applies.
//!
//! The numbers are the set-1 make codes Windows reports in `MakeCode`, and the
//! HID usage beside each one is what that position reports over USB. The usages
//! are the column to check this table against: they are what the sink's own
//! table in `host-macos` is written in, so the two ends agree exactly where the
//! two columns line up.

use favjit_core::Key;

/// The bits in `RAWKEYBOARD.Flags`.
///
/// Here rather than beside the structure declarations, because what they mean is
/// this module's subject: `E1` is the difference between the pause key and left
/// control, and `BREAK` is the difference between a press and a release. Both are
/// read by [`pressed`] and by nothing else.
pub const RI_KEY_BREAK: u16 = 0x01;
pub const RI_KEY_E0: u16 = 0x02;
pub const RI_KEY_E1: u16 = 0x04;

/// What one `RAWKEYBOARD` says, in `core`'s vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Down(Key),
    Up(Key),
    /// A position this layout has no key for, as the report named it.
    Unnamed {
        extended: bool,
        code: u16,
    },
    /// Not a key being pressed at all.
    NotAKey,
}

/// Read one `RAWKEYBOARD`'s fields.
///
/// The whole of what Windows' keyboard encoding means, in one place with one set
/// of tests: three things arrive on this stream that are not a key, and each of
/// them would become one if it were read as a make code.
///
/// - The `E1` prefix belongs to the pause key alone, and the make code under it
///   is left control's — so pressing pause would hold control on the other
///   machine.
/// - A make code of zero is Windows saying it has no scan code for this.
/// - A virtual key of `0xFF` is the filler event a keyboard sends alongside an
///   extended key.
pub fn pressed(flags: u16, make_code: u16, vkey: u16, ansi: bool) -> Pressed {
    if flags & RI_KEY_E1 != 0 || make_code == 0 || vkey == 0xFF {
        return Pressed::NotAKey;
    }
    let extended = flags & RI_KEY_E0 != 0;
    let Some(named) = key(extended, make_code, ansi) else {
        return Pressed::Unnamed {
            extended,
            code: make_code,
        };
    };
    match flags & RI_KEY_BREAK == 0 {
        true => Pressed::Down(named),
        false => Pressed::Up(named),
    }
}

/// The key at this make code, if this layout has a name for it.
///
/// `extended` is the `E0` prefix, which is what separates the two keys that
/// share a make code — right control from left, the arrow cluster from the
/// keypad.
///
/// `None` for a position no rule can mention: the function row, the keypad,
/// `Insert`, the page keys and the media keys are all here, exactly as they are
/// absent from the sink's usage table. Suppression takes them along with
/// everything else, so a key that lands here is a key that stops working while
/// favjit is relaying — [`crate::WindowsHost::unknown`] is where they are
/// reported so the gap is visible rather than guessed at.
pub fn key(extended: bool, code: u16, ansi: bool) -> Option<Key> {
    if extended {
        return extended_key(code);
    }
    Some(match code {
        0x01 => Key::Escape, // HID 0x29

        0x02 => Key::Digit1, // HID 0x1E
        0x03 => Key::Digit2, // HID 0x1F
        0x04 => Key::Digit3, // HID 0x20
        0x05 => Key::Digit4, // HID 0x21
        0x06 => Key::Digit5, // HID 0x22
        0x07 => Key::Digit6, // HID 0x23
        0x08 => Key::Digit7, // HID 0x24
        0x09 => Key::Digit8, // HID 0x25
        0x0A => Key::Digit9, // HID 0x26
        0x0B => Key::Digit0, // HID 0x27

        0x0C => Key::Hyphen,            // HID 0x2D
        0x0D => Key::EqualSign,         // HID 0x2E
        0x0E => Key::DeleteOrBackspace, // HID 0x2A
        0x0F => Key::Tab,               // HID 0x2B

        0x10 => Key::Q,             // HID 0x14
        0x11 => Key::W,             // HID 0x1A
        0x12 => Key::E,             // HID 0x08
        0x13 => Key::R,             // HID 0x15
        0x14 => Key::T,             // HID 0x17
        0x15 => Key::Y,             // HID 0x1C
        0x16 => Key::U,             // HID 0x18
        0x17 => Key::I,             // HID 0x0C
        0x18 => Key::O,             // HID 0x12
        0x19 => Key::P,             // HID 0x13
        0x1A => Key::OpenBracket,   // HID 0x2F
        0x1B => Key::CloseBracket,  // HID 0x30
        0x1C => Key::ReturnOrEnter, // HID 0x28
        0x1D => Key::LeftControl,   // HID 0xE0

        0x1E => Key::A,         // HID 0x04
        0x1F => Key::S,         // HID 0x16
        0x20 => Key::D,         // HID 0x07
        0x21 => Key::F,         // HID 0x09
        0x22 => Key::G,         // HID 0x0A
        0x23 => Key::H,         // HID 0x0B
        0x24 => Key::J,         // HID 0x0D
        0x25 => Key::K,         // HID 0x0E
        0x26 => Key::L,         // HID 0x0F
        0x27 => Key::Semicolon, // HID 0x33
        0x28 => Key::Quote,     // HID 0x34
        0x29 => Key::Grave,     // HID 0x35
        0x2A => Key::LeftShift, // HID 0xE1

        // The one position two keyboards disagree about, and the reason
        // [`crate::Config::ansi`] exists: a JIS keyboard puts `]`/`}` here and
        // reports HID 0x32, an ANSI one puts `\`/`|` here and reports HID 0x31.
        // The make code cannot tell them apart, and the sink has a separate rule
        // for each.
        0x2B if ansi => Key::Backslash, // HID 0x31
        0x2B => Key::NonUsPound,        // HID 0x32

        0x2C => Key::Z,          // HID 0x1D
        0x2D => Key::X,          // HID 0x1B
        0x2E => Key::C,          // HID 0x06
        0x2F => Key::V,          // HID 0x19
        0x30 => Key::B,          // HID 0x05
        0x31 => Key::N,          // HID 0x11
        0x32 => Key::M,          // HID 0x10
        0x33 => Key::Comma,      // HID 0x36
        0x34 => Key::Period,     // HID 0x37
        0x35 => Key::Slash,      // HID 0x38
        0x36 => Key::RightShift, // HID 0xE5

        0x38 => Key::LeftOption, // HID 0xE2, the left alt key
        0x39 => Key::Spacebar,   // HID 0x2C
        0x3A => Key::CapsLock,   // HID 0x39

        // The PC-JIS keys. The three thumb keys' usages were established on the
        // Mac by pressing each one alone and reading back what arrived; the other
        // two were observed arriving from the JIS keyboard and from no other. The
        // usage column is where the two ends check against each other rather than
        // against a document.
        0x70 => Key::JapanesePcKatakana, // HID 0x88, カタカナ/ひらがな
        0x73 => Key::International1,     // HID 0x87, the `_`/`\` key
        0x79 => Key::JapanesePcXfer,     // HID 0x8A, 変換
        0x7B => Key::JapanesePcNfer,     // HID 0x8B, 無変換
        0x7D => Key::International3,     // HID 0x89, the `￥`/`|` key

        _ => return None,
    })
}

/// The make code this key arrives on, if an unprefixed position produces it.
///
/// Searched over [`key`] rather than written out again: a second table is a second thing
/// to keep in step, and this is wanted twice while a run starts rather than once per
/// keystroke. Unprefixed only, which is what the chord that moves the keyboard is made
/// of ([`favjit_core::source::SWITCH_TO_THE_SINK`]).
///
/// The keyboard shape is not a parameter: the one position the two disagree about is
/// neither of the chord's keys, so both answers are the same.
pub fn code_for(named: Key) -> Option<u16> {
    (0..=0xFF).find(|code| key(false, *code, false) == Some(named))
}

/// What a key arriving from a low-level hook says, in the flags [`pressed`] reads.
///
/// Two structures spell the same two facts differently: `KBDLLHOOKSTRUCT` has the prefix
/// and the transition as `LLKHF_EXTENDED` and `LLKHF_UP`, and `RAWKEYBOARD` has them as
/// `RI_KEY_E0` and `RI_KEY_BREAK`. Translated here rather than read twice, so there is
/// one table and one interpretation of it however a key arrived.
///
/// There is no `E1` to translate: the pause key reaches a hook as its own make code
/// rather than as left control behind a prefix, so the position this table has no name
/// for is the one it reports.
pub fn from_a_hook(flags: u32, vkey: u16) -> u16 {
    /// `LLKHF_EXTENDED` and `LLKHF_UP`. Named here rather than beside the other Win32
    /// declarations because this module is compiled everywhere and those are not, and
    /// because this is the one place they are read.
    const EXTENDED: u32 = 0x01;
    const UP: u32 = 0x80;

    let mut out = 0;
    // Right shift arrives with the extended bit set and is not an extended position.
    // Read as one it is `E0 0x36`, which no key is named for, so the key does nothing at
    // all on the other machine. Cleared for this one key and no other, because the bit
    // is what separates right control from left and the arrow cluster from the keypad.
    if flags & EXTENDED != 0 && vkey != VK_RSHIFT {
        out |= RI_KEY_E0;
    }
    if flags & UP != 0 {
        out |= RI_KEY_BREAK;
    }
    out
}

/// `VK_RSHIFT`, the one virtual key whose extended bit is a lie.
pub const VK_RSHIFT: u16 = 0xA1;

/// The keys that only exist with the `E0` prefix.
///
/// Split out rather than folded into one match on a pair, because the prefix is
/// what makes two arms with the same number two different keys: a single match
/// would put `0x1D` twice in a row and the second would be unreachable.
fn extended_key(code: u16) -> Option<Key> {
    Some(match code {
        0x1D => Key::RightControl, // HID 0xE4
        0x38 => Key::RightOption,  // HID 0xE6, the right alt key

        // The Windows keys, which report as HID's GUI modifiers — the same
        // usages the Mac's command keys report, so this is the position the
        // layout means by `command` and not an approximation of it.
        0x5B => Key::LeftCommand,  // HID 0xE3
        0x5C => Key::RightCommand, // HID 0xE7

        0x47 => Key::Home,          // HID 0x4A
        0x48 => Key::UpArrow,       // HID 0x52
        0x4B => Key::LeftArrow,     // HID 0x50
        0x4D => Key::RightArrow,    // HID 0x4F
        0x4F => Key::End,           // HID 0x4D
        0x50 => Key::DownArrow,     // HID 0x51
        0x53 => Key::DeleteForward, // HID 0x4C

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every make code a set-1 keyboard can send, as a range rather than a list
    /// of the ones this table names: the point of the sweep is to cover the
    /// positions the table says nothing about too.
    const CODES: core::ops::RangeInclusive<u16> = 0x00..=0xFF;

    #[test]
    fn no_two_positions_are_the_same_key() {
        // A make code copied onto the wrong arm is the failure this catches, and
        // it is invisible from a run: the key simply produces what its neighbour
        // produces. Both keyboard shapes are swept, because the arm that differs
        // between them is exactly the kind that gets edited by hand.
        for ansi in [false, true] {
            let mut seen: Vec<(bool, u16, Key)> = Vec::new();
            for extended in [false, true] {
                for code in CODES {
                    let Some(key) = key(extended, code, ansi) else {
                        continue;
                    };
                    if let Some(other) = seen.iter().find(|(_, _, known)| *known == key) {
                        panic!("{key:?} is at {other:?} and also at ({extended}, {code:#04x})");
                    }
                    seen.push((extended, code, key));
                }
            }
        }
    }

    #[test]
    fn the_prefix_is_what_separates_the_two_keys_that_share_a_number() {
        // Right control and left control report the same make code, and so do
        // the arrow cluster and the keypad. A host that dropped the prefix would
        // put every one of them on the left-hand key.
        assert_eq!(key(false, 0x1D, false), Some(Key::LeftControl));
        assert_eq!(key(true, 0x1D, false), Some(Key::RightControl));
        assert_eq!(key(false, 0x38, false), Some(Key::LeftOption));
        assert_eq!(key(true, 0x38, false), Some(Key::RightOption));
    }

    #[test]
    fn the_key_beside_enter_is_the_one_the_keyboard_has() {
        // The sink has a rule for each of these two and they are not
        // interchangeable, so getting this wrong is a key that converts as
        // something else rather than one that fails to arrive.
        assert_eq!(key(false, 0x2B, false), Some(Key::NonUsPound));
        assert_eq!(key(false, 0x2B, true), Some(Key::Backslash));
    }

    #[test]
    fn the_windows_keys_are_the_command_keys() {
        // Not a convenience remap: they report HID's GUI usages, which is what
        // the Mac's command keys report, so this is the same position under two
        // names.
        assert_eq!(key(true, 0x5B, false), Some(Key::LeftCommand));
        assert_eq!(key(true, 0x5C, false), Some(Key::RightCommand));
    }

    #[test]
    fn the_pc_jis_keys_are_the_ones_the_layout_reads() {
        // The Henkan layer hangs off 変換, so a wrong number here is the layer
        // never being taken up.
        assert_eq!(key(false, 0x79, false), Some(Key::JapanesePcXfer));
        assert_eq!(key(false, 0x7B, false), Some(Key::JapanesePcNfer));
        assert_eq!(key(false, 0x70, false), Some(Key::JapanesePcKatakana));
        assert_eq!(key(false, 0x73, false), Some(Key::International1));
        assert_eq!(key(false, 0x7D, false), Some(Key::International3));
    }

    #[test]
    fn the_break_bit_is_what_separates_a_press_from_a_release() {
        assert_eq!(pressed(0, 0x1E, 0x41, false), Pressed::Down(Key::A));
        assert_eq!(
            pressed(RI_KEY_BREAK, 0x1E, 0x41, false),
            Pressed::Up(Key::A)
        );
    }

    #[test]
    fn the_pause_key_is_not_left_control() {
        // Pause reports left control's make code behind an `E1` prefix. Read
        // without the prefix it is a control key going down and never coming up,
        // which on the other machine is every later keystroke chorded with it.
        assert_eq!(pressed(RI_KEY_E1, 0x1D, 0x13, false), Pressed::NotAKey);
        assert_eq!(
            pressed(RI_KEY_E1 | RI_KEY_BREAK, 0x1D, 0x13, false),
            Pressed::NotAKey
        );
        assert_eq!(
            pressed(0, 0x1D, 0x11, false),
            Pressed::Down(Key::LeftControl)
        );
    }

    #[test]
    fn the_filler_a_keyboard_sends_beside_an_extended_key_is_not_a_key() {
        // Both shapes of "this is not a key": the virtual key Windows fills in as
        // `0xFF`, and a report with no make code at all.
        assert_eq!(pressed(0, 0x2A, 0xFF, false), Pressed::NotAKey);
        assert_eq!(pressed(0, 0x00, 0x41, false), Pressed::NotAKey);
    }

    #[test]
    fn a_position_with_no_key_is_reported_as_the_position_it_was() {
        // So that a key which does nothing while suppression is on can be named in
        // the report rather than guessed at. F1, and the extended `Insert`.
        assert_eq!(
            pressed(0, 0x3B, 0x70, false),
            Pressed::Unnamed {
                extended: false,
                code: 0x3B
            }
        );
        assert_eq!(
            pressed(RI_KEY_E0 | RI_KEY_BREAK, 0x52, 0x2D, false),
            Pressed::Unnamed {
                extended: true,
                code: 0x52
            },
            "a release of an unnamed position is still that position"
        );
    }

    #[test]
    fn right_shift_is_right_shift_even_when_the_hook_calls_it_extended() {
        // Windows delivers right shift with the extended bit set, and right shift is not
        // an extended position: read as one it is `E0 0x36`, which no key is named for,
        // so the key does nothing at all on the other machine. Deskflow's hook clears
        // the same bit for the same reason.
        assert_eq!(
            pressed(from_a_hook(0x01, VK_RSHIFT), 0x36, VK_RSHIFT, false),
            Pressed::Down(Key::RightShift)
        );
        assert_eq!(
            pressed(from_a_hook(0x81, VK_RSHIFT), 0x36, VK_RSHIFT, false),
            Pressed::Up(Key::RightShift)
        );
    }

    #[test]
    fn the_extended_bit_still_separates_the_two_keys_that_share_a_number() {
        // Cleared for right shift alone: a hook that dropped it for everything would put
        // right control on the left-hand key and the arrow cluster on the keypad.
        assert_eq!(
            pressed(from_a_hook(0x01, 0x11), 0x1D, 0x11, false),
            Pressed::Down(Key::RightControl)
        );
        assert_eq!(
            pressed(from_a_hook(0x00, 0x11), 0x1D, 0x11, false),
            Pressed::Down(Key::LeftControl)
        );
    }

    #[test]
    fn the_chord_that_moves_the_keyboard_is_read_like_any_other_key() {
        // Nothing here treats it specially: what makes it a chord is `favjit_core`
        // seeing the two together, and this table's job is only to name the positions.
        assert_eq!(
            key(false, 0x31, false),
            Some(favjit_core::source::SWITCH_TO_THE_SINK)
        );
        assert_eq!(
            key(false, 0x1F, false),
            Some(favjit_core::source::SWITCH_BACK)
        );
    }

    #[test]
    fn a_position_this_table_has_no_name_for_is_nothing_rather_than_a_guess() {
        // The function row and the keypad. Naming them as their nearest
        // neighbour would convert a key into one the user did not press, which
        // is worse than the key not working.
        assert_eq!(key(false, 0x3B, false), None); // F1
        assert_eq!(key(false, 0x53, false), None); // keypad `.`
        assert_eq!(key(true, 0x52, false), None); // insert
        assert_eq!(key(false, 0x00, false), None);
    }
}
