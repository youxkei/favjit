//! The physical key vocabulary.

/// A physical key, identified by position rather than by the character it
/// produces.
///
/// Names follow Karabiner-Elements' `key_code` vocabulary so that the layout
/// tables in [`crate::layout`] can be read against the Karabiner configuration
/// they were ported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,

    /// The number row, in its physical order: `1` through `9` then `0`.
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Digit0,

    Hyphen,
    EqualSign,
    OpenBracket,
    CloseBracket,
    Backslash,
    /// HID 0x32. The position a JIS keyboard labels `]`/`}`.
    NonUsPound,
    Semicolon,
    Quote,
    Grave,
    Comma,
    Period,
    Slash,

    Tab,
    CapsLock,
    Spacebar,
    Escape,
    ReturnOrEnter,
    DeleteOrBackspace,
    DeleteForward,

    LeftArrow,
    RightArrow,
    UpArrow,
    DownArrow,
    Home,
    End,

    LeftShift,
    RightShift,
    LeftControl,
    RightControl,
    LeftOption,
    RightOption,
    LeftCommand,
    RightCommand,
    Fn,

    /// Apple-JIS 英数.
    JapaneseEisuu,
    /// Apple-JIS かな.
    JapaneseKana,
    /// PC-JIS 無変換.
    JapanesePcNfer,
    /// PC-JIS 変換.
    JapanesePcXfer,
    /// PC-JIS カタカナひらがな.
    JapanesePcKatakana,

    /// HID 0x87. The JIS-only key labelled `_`/`\`.
    International1,
    /// HID 0x89. The JIS-only key labelled `￥`/`|`.
    International3,

    /// The function row's keys, as the keyboard reports them.
    ///
    /// A MacBook's top row sends these and nothing else — the icons printed on it
    /// are what the OS makes of them for its own keyboard, not what the keyboard
    /// says (`docs/platform/macos/hid-input-callbacks.md`).
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,

    /// The controls the icons on that row stand for.
    ///
    /// Named after what they do rather than after the HID usage that carries each
    /// one, because the usages are on four different pages and two of them are
    /// Apple's own — a name like `ConsumerDisplayBrightnessDecrement` would put a
    /// page in the layout's vocabulary, where what a rule wants to say is
    /// "brightness down".
    BrightnessDown,
    BrightnessUp,
    MissionControl,
    Spotlight,
    Dictation,
    DoNotDisturb,
    Rewind,
    PlayPause,
    FastForward,
    Mute,
    VolumeDown,
    VolumeUp,
}

impl Key {
    /// Every key, once.
    ///
    /// Written out rather than derived, because there is no deriving it: the enum
    /// is `#[non_exhaustive]`, so nothing outside this crate can walk it, and the
    /// two things that need to — a trace's numbering
    /// ([`crate::trace`]) and a host's table walk — both need the whole set or they
    /// are checking a subset and saying nothing about the rest.
    ///
    /// The order is the wire order a trace records keys in, so **a key is added at
    /// the end**. Inserting one in the middle renumbers every key after it, and a
    /// trace read against the new numbering replays different keystrokes than were
    /// typed.
    pub const ALL: &'static [Key] = &[
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::N,
        Key::O,
        Key::P,
        Key::Q,
        Key::R,
        Key::S,
        Key::T,
        Key::U,
        Key::V,
        Key::W,
        Key::X,
        Key::Y,
        Key::Z,
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
        Key::Digit0,
        Key::Hyphen,
        Key::EqualSign,
        Key::OpenBracket,
        Key::CloseBracket,
        Key::Backslash,
        Key::NonUsPound,
        Key::Semicolon,
        Key::Quote,
        Key::Grave,
        Key::Comma,
        Key::Period,
        Key::Slash,
        Key::Tab,
        Key::CapsLock,
        Key::Spacebar,
        Key::Escape,
        Key::ReturnOrEnter,
        Key::DeleteOrBackspace,
        Key::DeleteForward,
        Key::LeftArrow,
        Key::RightArrow,
        Key::UpArrow,
        Key::DownArrow,
        Key::Home,
        Key::End,
        Key::LeftShift,
        Key::RightShift,
        Key::LeftControl,
        Key::RightControl,
        Key::LeftOption,
        Key::RightOption,
        Key::LeftCommand,
        Key::RightCommand,
        Key::Fn,
        Key::JapaneseEisuu,
        Key::JapaneseKana,
        Key::JapanesePcNfer,
        Key::JapanesePcXfer,
        Key::JapanesePcKatakana,
        Key::International1,
        Key::International3,
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::BrightnessDown,
        Key::BrightnessUp,
        Key::MissionControl,
        Key::Spotlight,
        Key::Dictation,
        Key::DoNotDisturb,
        Key::Rewind,
        Key::PlayPause,
        Key::FastForward,
        Key::Mute,
        Key::VolumeDown,
        Key::VolumeUp,
    ];

    /// This key's number on the wire, for the formats that carry keys as bytes.
    ///
    /// One numbering for all of them, rather than one each: a trace and a link
    /// that numbered keys separately would drift apart, and the symptom is a
    /// replay or a relay producing a *different* key rather than failing.
    ///
    /// Zero is reserved for "no key", so the numbering starts at one — a zeroed
    /// buffer decodes as absent rather than as `A`.
    pub fn code(self) -> u8 {
        Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .map(|index| index as u8 + 1)
            .unwrap_or(0)
    }

    pub fn from_code(code: u8) -> Option<Self> {
        if code == 0 {
            return None;
        }
        Self::ALL.get(code as usize - 1).copied()
    }

    /// The modifier this key contributes while it is held, if it is one.
    ///
    /// `Fn` is absent deliberately: macOS reports it, but it carries no
    /// modifier bit that a layout rule can match on, and on the built-in
    /// keyboard it is remapped to a command key before it could contribute one.
    pub const fn modifier(self) -> Option<crate::Modifiers> {
        use crate::Modifiers as M;
        match self {
            Key::LeftShift | Key::RightShift => Some(M::SHIFT),
            Key::LeftControl | Key::RightControl => Some(M::CONTROL),
            Key::LeftOption | Key::RightOption => Some(M::OPTION),
            Key::LeftCommand | Key::RightCommand => Some(M::COMMAND),
            Key::CapsLock => Some(M::CAPS_LOCK),
            _ => None,
        }
    }
}
