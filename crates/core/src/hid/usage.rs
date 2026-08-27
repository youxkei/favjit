//! HID page and usage ↔ [`Key`], both directions.
//!
//! Capture on macOS happens at HID usage level rather than at CGEvent level, for
//! the reason recorded in `docs/platform/macos/hid-input-callbacks.md`: this is
//! where the originating keyboard is available, and where keys that have no `kVK_`
//! constant at all still have a value. So this is the vocabulary that machine's
//! keyboards arrive in, and the one its output device is spoken to in.

use super::page;
use crate::Key;

/// The controls the layout can name, each with the page and usage that carries it.
///
/// One table for both directions, so the way in and the way out cannot come to
/// disagree about which usage a control is.
///
/// Four pages between twelve controls, and each needs its own report on the output
/// device — which is why a key carries its page here rather than a usage alone.
const CONTROLS: &[(Key, u32, u16)] = &[
    (Key::BrightnessDown, page::CONSUMER, 0x0070),
    (Key::BrightnessUp, page::CONSUMER, 0x006F),
    (Key::Dictation, page::CONSUMER, 0x00CF),
    (Key::Rewind, page::CONSUMER, 0x00B4),
    (Key::PlayPause, page::CONSUMER, 0x00CD),
    (Key::FastForward, page::CONSUMER, 0x00B3),
    (Key::Mute, page::CONSUMER, 0x00E2),
    (Key::VolumeDown, page::CONSUMER, 0x00EA),
    (Key::VolumeUp, page::CONSUMER, 0x00E9),
    (Key::MissionControl, page::APPLE_VENDOR_KEYBOARD, 0x0010),
    (Key::Spotlight, page::APPLE_VENDOR_KEYBOARD, 0x0001),
    (Key::DoNotDisturb, page::GENERIC_DESKTOP, 0x009B),
];

/// Whether a queue should carry this element.
///
/// A whitelist per page rather than a page filter: Apple's top-case page carries
/// `Fn` and also `reserved_mouse_data`, so admitting the page would put data
/// nothing reads into the interactive path at motion rates.
///
/// The pointer's own elements are admitted, and they do arrive at motion rates:
/// a seize is per device and the TrackPoint keyboard is one device, so the queue
/// that suppresses its keys is the only way its pointer can come back out
/// (`docs/platform/macos/input-suppression.md`).
pub fn watched(page: u32, usage: u32) -> bool {
    named(page, usage).is_some() || page == self::page::KEYBOARD_OR_KEYPAD || pointer(page, usage)
}

/// Whether this element is part of a pointer.
///
/// Named individually rather than by page: the generic desktop page also carries
/// the system power and sleep controls, and a relay has no business repeating
/// those.
pub fn pointer(page: u32, usage: u32) -> bool {
    if page == self::page::GENERIC_DESKTOP {
        return matches!(usage, POINTER_X | POINTER_Y | POINTER_WHEEL);
    }
    page == self::page::BUTTON
}

/// Generic desktop `X`.
pub const POINTER_X: u32 = 0x30;
/// Generic desktop `Y`.
pub const POINTER_Y: u32 = 0x31;
/// Generic desktop `Wheel`.
pub const POINTER_WHEEL: u32 = 0x38;

/// The key an element's page and usage stand for.
///
/// `None` for a usage on a page favjit reads but has no name for, which is what
/// `favjit --usages` reports so the gap can be closed.
pub fn named(page: u32, usage: u32) -> Option<Key> {
    // Compared rather than matched, because a `match` arm on one of these
    // constants binds the name instead of testing it — it matches every page and
    // the arms below it become unreachable.
    if page == self::page::KEYBOARD_OR_KEYPAD {
        return key(usage);
    }
    if page == self::page::APPLE_VENDOR_TOP_CASE && usage == super::KEYBOARD_FN {
        return Some(Key::Fn);
    }
    // A keyboard with its own volume keys sends the control itself rather than a
    // function key, so the same names have to be readable as well as sendable.
    // Looked up rather than listed again: one table, walked both ways.
    CONTROLS
        .iter()
        .find(|(_, control_page, control_usage)| {
            *control_page == page && u32::from(*control_usage) == usage
        })
        .map(|(key, _, _)| *key)
}

/// The key at this usage on page 7, if this layout has a name for it.
///
/// `None` for a usage no rule can mention — the error and rollover reports a
/// keyboard sends alongside real presses land here, as does the keypad.
fn key(usage: u32) -> Option<Key> {
    Some(match usage {
        0x04 => Key::A,
        0x05 => Key::B,
        0x06 => Key::C,
        0x07 => Key::D,
        0x08 => Key::E,
        0x09 => Key::F,
        0x0A => Key::G,
        0x0B => Key::H,
        0x0C => Key::I,
        0x0D => Key::J,
        0x0E => Key::K,
        0x0F => Key::L,
        0x10 => Key::M,
        0x11 => Key::N,
        0x12 => Key::O,
        0x13 => Key::P,
        0x14 => Key::Q,
        0x15 => Key::R,
        0x16 => Key::S,
        0x17 => Key::T,
        0x18 => Key::U,
        0x19 => Key::V,
        0x1A => Key::W,
        0x1B => Key::X,
        0x1C => Key::Y,
        0x1D => Key::Z,

        0x1E => Key::Digit1,
        0x1F => Key::Digit2,
        0x20 => Key::Digit3,
        0x21 => Key::Digit4,
        0x22 => Key::Digit5,
        0x23 => Key::Digit6,
        0x24 => Key::Digit7,
        0x25 => Key::Digit8,
        0x26 => Key::Digit9,
        0x27 => Key::Digit0,

        0x28 => Key::ReturnOrEnter,
        0x29 => Key::Escape,
        0x2A => Key::DeleteOrBackspace,
        0x2B => Key::Tab,
        0x2C => Key::Spacebar,
        0x2D => Key::Hyphen,
        0x2E => Key::EqualSign,
        0x2F => Key::OpenBracket,
        0x30 => Key::CloseBracket,
        0x31 => Key::Backslash,
        0x32 => Key::NonUsPound,
        0x33 => Key::Semicolon,
        0x34 => Key::Quote,
        0x35 => Key::Grave,
        0x36 => Key::Comma,
        0x37 => Key::Period,
        0x38 => Key::Slash,
        0x39 => Key::CapsLock,

        // The function row. A MacBook's top row reports these and nothing else:
        // the brightness and volume icons printed on it are what the OS makes of
        // them for its own keyboard, which is why the layout has to say what they
        // mean once the keyboard is held here.
        0x3A => Key::F1,
        0x3B => Key::F2,
        0x3C => Key::F3,
        0x3D => Key::F4,
        0x3E => Key::F5,
        0x3F => Key::F6,
        0x40 => Key::F7,
        0x41 => Key::F8,
        0x42 => Key::F9,
        0x43 => Key::F10,
        0x44 => Key::F11,
        0x45 => Key::F12,

        0x4A => Key::Home,
        0x4C => Key::DeleteForward,
        0x4D => Key::End,
        0x4F => Key::RightArrow,
        0x50 => Key::LeftArrow,
        0x51 => Key::DownArrow,
        0x52 => Key::UpArrow,

        // The JIS-only positions. Observed arriving only from the JIS keyboard,
        // never from the US internal one.
        0x87 => Key::International1,
        0x89 => Key::International3,

        // The PC-JIS thumb keys, each established by pressing it and reading
        // back the usage nothing could name. The names in the SDK's usage table
        // say nothing about which key they are, so the association is this
        // observation and nothing else.
        0x88 => Key::JapanesePcKatakana,
        0x8A => Key::JapanesePcXfer,
        0x8B => Key::JapanesePcNfer,

        0xE0 => Key::LeftControl,
        0xE1 => Key::LeftShift,
        0xE2 => Key::LeftOption,
        0xE3 => Key::LeftCommand,
        0xE4 => Key::RightControl,
        0xE5 => Key::RightShift,
        0xE6 => Key::RightOption,
        0xE7 => Key::RightCommand,

        _ => return None,
    })
}

/// The page and usage that stand for this key, for sending it.
///
/// The inverse of [`key`], written out rather than derived from it: a derived
/// inverse would have to decide what to do about two keys sharing a usage, and
/// writing both directions means the pair can be walked against each other in a
/// test instead.
///
/// `None` where nothing on any page favjit can post to stands for this key. `Fn`
/// is the one this layout reads and cannot send: the layout only ever matches on
/// it.
pub fn of(key: Key) -> Option<(u32, u16)> {
    if let Some((_, page, usage)) = CONTROLS.iter().find(|(control, _, _)| *control == key) {
        return Some((*page, *usage));
    }
    Some((self::page::KEYBOARD_OR_KEYPAD, on_page_seven(key)?))
}

/// The usage on page 7 that stands for this key.
fn on_page_seven(key: Key) -> Option<u16> {
    Some(match key {
        Key::A => 0x04,
        Key::B => 0x05,
        Key::C => 0x06,
        Key::D => 0x07,
        Key::E => 0x08,
        Key::F => 0x09,
        Key::G => 0x0A,
        Key::H => 0x0B,
        Key::I => 0x0C,
        Key::J => 0x0D,
        Key::K => 0x0E,
        Key::L => 0x0F,
        Key::M => 0x10,
        Key::N => 0x11,
        Key::O => 0x12,
        Key::P => 0x13,
        Key::Q => 0x14,
        Key::R => 0x15,
        Key::S => 0x16,
        Key::T => 0x17,
        Key::U => 0x18,
        Key::V => 0x19,
        Key::W => 0x1A,
        Key::X => 0x1B,
        Key::Y => 0x1C,
        Key::Z => 0x1D,

        Key::Digit1 => 0x1E,
        Key::Digit2 => 0x1F,
        Key::Digit3 => 0x20,
        Key::Digit4 => 0x21,
        Key::Digit5 => 0x22,
        Key::Digit6 => 0x23,
        Key::Digit7 => 0x24,
        Key::Digit8 => 0x25,
        Key::Digit9 => 0x26,
        Key::Digit0 => 0x27,

        Key::ReturnOrEnter => 0x28,
        Key::Escape => 0x29,
        Key::DeleteOrBackspace => 0x2A,
        Key::Tab => 0x2B,
        Key::Spacebar => 0x2C,
        Key::Hyphen => 0x2D,
        Key::EqualSign => 0x2E,
        Key::OpenBracket => 0x2F,
        Key::CloseBracket => 0x30,
        Key::Backslash => 0x31,
        Key::NonUsPound => 0x32,
        Key::Semicolon => 0x33,
        Key::Quote => 0x34,
        Key::Grave => 0x35,
        Key::Comma => 0x36,
        Key::Period => 0x37,
        Key::Slash => 0x38,
        Key::CapsLock => 0x39,

        // Sendable as well as readable, because a keyboard whose top row the
        // layout leaves alone passes its function keys straight through.
        Key::F1 => 0x3A,
        Key::F2 => 0x3B,
        Key::F3 => 0x3C,
        Key::F4 => 0x3D,
        Key::F5 => 0x3E,
        Key::F6 => 0x3F,
        Key::F7 => 0x40,
        Key::F8 => 0x41,
        Key::F9 => 0x42,
        Key::F10 => 0x43,
        Key::F11 => 0x44,
        Key::F12 => 0x45,

        Key::Home => 0x4A,
        Key::DeleteForward => 0x4C,
        Key::End => 0x4D,
        Key::RightArrow => 0x4F,
        Key::LeftArrow => 0x50,
        Key::DownArrow => 0x51,
        Key::UpArrow => 0x52,

        Key::International1 => 0x87,
        Key::JapanesePcKatakana => 0x88,
        Key::International3 => 0x89,
        Key::JapanesePcXfer => 0x8A,
        Key::JapanesePcNfer => 0x8B,

        // `LANG1` and `LANG2`, which the SDK's usage tables do not name; the
        // numbers are from Karabiner's vendored `pqrs/hid`. Which of the pair is
        // かな and which is 英数 is settled by pressing them, not by the names.
        Key::JapaneseKana => 0x90,
        Key::JapaneseEisuu => 0x91,

        Key::LeftControl => 0xE0,
        Key::LeftShift => 0xE1,
        Key::LeftOption => 0xE2,
        Key::LeftCommand => 0xE3,
        Key::RightControl => 0xE4,
        Key::RightShift => 0xE5,
        Key::RightOption => 0xE6,
        Key::RightCommand => 0xE7,

        // `Fn` lives on Apple's top-case page, which the layout only ever matches
        // on and never emits, and the twelve controls live on three pages of their
        // own — they are reached through `CONTROLS` above rather than from here. A
        // key added to the vocabulary lands here too, and the table walk in
        // [`super::tables`] is what makes that a failing test rather than a key
        // that quietly never arrives.
        Key::Fn | _ => return None,
    })
}

/// Keys this layout reads that no usage is known for.
///
/// Empty, and the list stays because the gap it names is the hardest kind to
/// notice: a rule whose `from` key never arrives simply never fires, and nothing
/// about the run says so.
///
/// `JapaneseEisuu` and `JapaneseKana` are not in it even though no usage names
/// them, because the layout only ever *emits* those; nothing reads them.
/// `favjit --usages` reports anything unnamed that does arrive, which is how a
/// gap here gets closed.
pub const UNMAPPED: &[Key] = &[];
