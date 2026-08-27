//! The Dudrack layout: JIS-Dvorak with a Henkan layer.
//!
//! Ported from the Karabiner-Elements configuration this layout was developed in,
//! entry for entry, rather than reinvented: the layout is the one already in the
//! user's fingers, so a rule that reads oddly here is a rule that reads oddly there.
//!
//! What the table covers: modifier remaps including space-as-shift, the Henkan
//! and Neutral layers on the Dudrack keyboards, JIS labels and PC-JIS IME keys
//! on every other external keyboard, the Cmd+H guards, and Home/End.

use core::time::Duration;

use super::{Action, FromMods, Layer, Layout, Rule};
use crate::{DeviceMatch, Key, Modifiers as M, Scope};

/// Lenovo TrackPoint Keyboard II: a JIS QWERTY keyboard typed in Dudrack, so it
/// rides the Dudrack layers rather than the raw-JIS remaps.
const TRACKPOINT_KEYBOARD_II: DeviceMatch = DeviceMatch::new(6127, 24801);

/// How long the space bar may be held and still count as a tap.
///
/// Generous on purpose, and the direction matters: erring long occasionally
/// types a space the user had stopped wanting, while erring short swallows one
/// they did type. A lost character is the worse of the two.
///
/// This is favjit's number. The Karabiner configuration this layout came from
/// sets no tap-window parameter at all, so it runs on a default whose value is
/// recorded nowhere here.
const TAP_WINDOW: Duration = Duration::from_millis(1000);

/// Key positions whose output is named under a US/ANSI interpretation.
///
/// The layout's job ends at "which key position, with which modifiers"; what
/// character that produces is settled by the OS input source, not here. These
/// names say which character each pair is *meant* to produce under a US layout,
/// so the tables below can be read as the symbols the user types.
///
/// Naming them under US rather than JIS is not a way of reaching JIS behaviour
/// indirectly — it is the direction the whole layout runs in. JIS labels are
/// reproduced by sending US positions, which is why `RAW_JIS` below exists at
/// all. The Karabiner configuration this was ported from is pinned to an ANSI
/// virtual keyboard for a concrete reason: on JIS it aliases the US `backslash`
/// position onto JIS `]`, which puts `\` and therefore ctrl+`\` out of reach.
/// favjit's injection path carries no such aliasing — the position types `\` and
/// `|` where it should (`docs/platform/macos/event-injection.md`) — so the names
/// below can be trusted to mean what they say.
mod ansi {
    use crate::{Key, Modifiers as M};

    pub const DOUBLE_QUOTE: (Key, M) = (Key::Quote, M::SHIFT);
    pub const SINGLE_QUOTE: (Key, M) = (Key::Quote, M::NONE);
    pub const AMPERSAND: (Key, M) = (Key::Digit7, M::SHIFT);
    pub const LEFT_PAREN: (Key, M) = (Key::Digit9, M::SHIFT);
    pub const RIGHT_PAREN: (Key, M) = (Key::Digit0, M::SHIFT);
    pub const EQUAL: (Key, M) = (Key::EqualSign, M::NONE);
    pub const TILDE: (Key, M) = (Key::Grave, M::SHIFT);
    pub const CARET: (Key, M) = (Key::Digit6, M::SHIFT);
    pub const BACKTICK: (Key, M) = (Key::Grave, M::NONE);
    pub const AT: (Key, M) = (Key::Digit2, M::SHIFT);
    pub const LEFT_BRACE: (Key, M) = (Key::OpenBracket, M::SHIFT);
    pub const LEFT_BRACKET: (Key, M) = (Key::OpenBracket, M::NONE);
    pub const RIGHT_BRACE: (Key, M) = (Key::CloseBracket, M::SHIFT);
    pub const RIGHT_BRACKET: (Key, M) = (Key::CloseBracket, M::NONE);
    pub const PLUS: (Key, M) = (Key::EqualSign, M::SHIFT);
    pub const ASTERISK: (Key, M) = (Key::Digit8, M::SHIFT);
    pub const COLON: (Key, M) = (Key::Semicolon, M::SHIFT);
    pub const UNDERSCORE: (Key, M) = (Key::Hyphen, M::SHIFT);
    pub const BACKSLASH: (Key, M) = (Key::Backslash, M::NONE);
    pub const PIPE: (Key, M) = (Key::Backslash, M::SHIFT);
}

/// One layer entry: the physical key, whether shift is part of it, and what it
/// produces.
type Entry = (Key, bool, (Key, M));

/// A layer entry whose output carries no modifier of its own.
const fn plain(key: Key) -> (Key, M) {
    (key, M::NONE)
}

const fn emit(scope: Scope, layer: Layer, from: Key, mods: FromMods, to: (Key, M)) -> Rule {
    Rule {
        scope,
        layer,
        from,
        mods,
        action: Action::Emit {
            key: to.0,
            modifiers: to.1,
        },
    }
}

const fn swallow(scope: Scope, layer: Layer, from: Key, mods: FromMods) -> Rule {
    Rule {
        scope,
        layer,
        from,
        mods,
        action: Action::Swallow,
    }
}

const fn hold_henkan(scope: Scope, from: Key) -> Rule {
    Rule {
        scope,
        layer: Layer::Any,
        from,
        mods: FromMods::ANY,
        action: Action::HoldHenkan,
    }
}

/// Expand a layer table.
///
/// Entry order is preserved because the engine takes the first match: a shifted
/// entry has to precede the unshifted entry for the same key wherever the
/// unshifted one tolerates shift.
fn expand(
    scope: Scope,
    layer: Layer,
    shifted: FromMods,
    unshifted: FromMods,
    entries: &[Entry],
) -> impl Iterator<Item = Rule> + '_ {
    entries.iter().map(move |&(from, needs_shift, to)| {
        emit(
            scope,
            layer,
            from,
            if needs_shift { shifted } else { unshifted },
            to,
        )
    })
}

pub(super) fn layout() -> Layout {
    let mut rules: Vec<Rule> = Vec::new();

    // ---- Modifier remaps -------------------------------------------------
    //
    // Caps Lock and Tab are shared by every Dudrack keyboard; the rest are
    // MacBook-physical and stay built-in only.
    rules.extend([
        emit(
            Scope::Dudrack,
            Layer::Any,
            Key::CapsLock,
            FromMods::ANY,
            plain(Key::LeftControl),
        ),
        emit(
            Scope::Dudrack,
            Layer::Any,
            Key::Tab,
            FromMods::ANY,
            plain(Key::LeftCommand),
        ),
        emit(
            Scope::BuiltIn,
            Layer::Any,
            Key::LeftCommand,
            FromMods::ANY,
            plain(Key::LeftOption),
        ),
        emit(
            Scope::BuiltIn,
            Layer::Any,
            Key::LeftOption,
            FromMods::ANY,
            plain(Key::LeftCommand),
        ),
        // Fn produces a plain right command, not the Henkan layer: the Henkan
        // rule below matches the physical right command key, and a rule's
        // output is never fed back through the table.
        emit(
            Scope::BuiltIn,
            Layer::Any,
            Key::Fn,
            FromMods::ANY,
            plain(Key::RightCommand),
        ),
        // Space and Shift. Built-in only — every other keyboard keeps its space
        // bar, since this exists to spare the MacBook's little fingers.
        //
        // Shift here rather than on a command key: holding both command keys
        // ghosts on MacBook keyboards, so a chord that needed left command,
        // right command and one more key never saw the third press reported.
        Rule {
            scope: Scope::BuiltIn,
            layer: Layer::Any,
            from: Key::Spacebar,
            mods: FromMods::ANY,
            action: Action::TapHold {
                hold: Key::LeftShift,
                tap: Key::Spacebar,
                tap_window: TAP_WINDOW,
            },
        },
        hold_henkan(Scope::BuiltIn, Key::RightCommand),
        // The Dudrack-typed external keyboard has real JIS thumb keys, so the
        // layer key is 変換 there rather than right command.
        hold_henkan(Scope::DudrackExternal, Key::JapanesePcXfer),
        emit(
            Scope::DudrackExternal,
            Layer::Any,
            Key::JapanesePcNfer,
            FromMods::ANY,
            plain(Key::LeftShift),
        ),
        emit(
            Scope::DudrackExternal,
            Layer::Any,
            Key::JapanesePcKatakana,
            FromMods::ANY,
            plain(Key::LeftCommand),
        ),
        // 右Alt sits where a thumb lands; nothing is bound to it on purpose.
        swallow(
            Scope::DudrackExternal,
            Layer::Any,
            Key::RightOption,
            FromMods::ANY,
        ),
    ]);

    // ---- The MacBook's function row --------------------------------------
    //
    // The row sends F1 to F12 and nothing else, and what the icons printed on it
    // mean is the OS's doing for its own keyboard — a key taken from that keyboard
    // and handed back as F1 through a virtual device is an F1
    // (`docs/platform/macos/hid-input-callbacks.md`). So the icons are this
    // layout's to reproduce, and the pairing is Karabiner-Elements' own default
    // for the same row, read off its `make_default_fn_function_keys_json`.
    //
    // Built-in only. An external keyboard's top row is printed `F1`, and a person
    // pressing it there means the function key.
    rules.extend(
        [
            (Key::F1, Key::BrightnessDown),
            (Key::F2, Key::BrightnessUp),
            (Key::F3, Key::MissionControl),
            (Key::F4, Key::Spotlight),
            (Key::F5, Key::Dictation),
            (Key::F6, Key::DoNotDisturb),
            (Key::F7, Key::Rewind),
            (Key::F8, Key::PlayPause),
            (Key::F9, Key::FastForward),
            (Key::F10, Key::Mute),
            (Key::F11, Key::VolumeDown),
            (Key::F12, Key::VolumeUp),
        ]
        .map(|(from, to)| emit(Scope::BuiltIn, Layer::Any, from, FromMods::ANY, plain(to))),
    );

    // ---- Cmd+H guards ----------------------------------------------------
    //
    // Cmd+H (Hide) and Cmd+Opt+H (Hide Others) get triggered by accident.
    // These match the *physical* key that yields `h` after conversion, which is
    // why they have to precede the layers: on Dudrack that key is `j`, on a
    // raw-JIS keyboard it is `h`. Option is tolerated rather than required so
    // that plain Cmd+H matches too.
    const HIDE_GUARD: FromMods = FromMods::mandatory(M::COMMAND, M::CAPS_LOCK.union(M::OPTION));
    rules.extend([
        // Neutral only: under Henkan `j` is `\`, which must pass through.
        swallow(Scope::Dudrack, Layer::Neutral, Key::J, HIDE_GUARD),
        swallow(Scope::RawJis, Layer::Any, Key::H, HIDE_GUARD),
        // Holding both physical command keys while reaching for `@` ghosts on
        // MacBook keyboards: left command leaks `option` through its remap and
        // the Henkan `@` is shift+2, so the chord surfaces as option+shift+2 —
        // a window-manager binding that flings the focused window away. Swallow
        // the contaminated press; `@` typed without option is untouched.
        swallow(
            Scope::BuiltIn,
            Layer::Henkan,
            Key::H,
            FromMods::mandatory(M::OPTION, M::CAPS_LOCK.union(M::SHIFT)),
        ),
    ]);

    // ---- Henkan layer ----------------------------------------------------
    #[rustfmt::skip]
    const HENKAN: &[Entry] = &[
        (Key::Q,         false, plain(Key::Digit1)),
        (Key::W,         true,  ansi::DOUBLE_QUOTE),
        (Key::W,         false, plain(Key::Digit2)),
        (Key::E,         false, plain(Key::Digit3)),
        (Key::R,         false, plain(Key::Digit4)),
        (Key::T,         false, plain(Key::Digit5)),
        (Key::Y,         true,  ansi::AMPERSAND),
        (Key::Y,         false, plain(Key::Digit6)),
        (Key::U,         true,  ansi::SINGLE_QUOTE),
        (Key::U,         false, plain(Key::Digit7)),
        (Key::I,         true,  ansi::LEFT_PAREN),
        (Key::I,         false, plain(Key::Digit8)),
        (Key::O,         true,  ansi::RIGHT_PAREN),
        (Key::O,         false, plain(Key::Digit9)),
        (Key::P,         true,  plain(Key::Digit0)),
        (Key::P,         false, plain(Key::Digit0)),
        (Key::A,         false, plain(Key::Tab)),
        (Key::S,         false, plain(Key::Escape)),
        (Key::D,         false, plain(Key::ReturnOrEnter)),
        (Key::F,         false, plain(Key::DeleteOrBackspace)),
        (Key::G,         false, plain(Key::DeleteForward)),
        (Key::H,         true,  ansi::BACKTICK),
        (Key::H,         false, ansi::AT),
        (Key::J,         true,  ansi::UNDERSCORE),
        (Key::J,         false, ansi::BACKSLASH),
        (Key::K,         false, ansi::LEFT_BRACKET),
        (Key::L,         false, ansi::RIGHT_BRACKET),
        (Key::Semicolon, true,  ansi::PIPE),
        (Key::Semicolon, false, ansi::BACKSLASH),
        (Key::Z,         false, plain(Key::LeftArrow)),
        (Key::X,         false, plain(Key::DownArrow)),
        (Key::C,         false, plain(Key::UpArrow)),
        (Key::V,         false, plain(Key::RightArrow)),
        (Key::B,         false, plain(Key::X)),
        (Key::N,         false, plain(Key::JapaneseKana)),
        (Key::M,         false, plain(Key::JapaneseEisuu)),
        (Key::Comma,     false, (Key::LeftArrow,  M::COMMAND)),
        (Key::Period,    false, (Key::RightArrow, M::COMMAND)),
        (Key::Slash,     true,  ansi::TILDE),
        (Key::Slash,     false, ansi::CARET),
    ];
    rules.extend(expand(
        Scope::Dudrack,
        Layer::Henkan,
        FromMods::mandatory(
            M::SHIFT,
            M::CAPS_LOCK
                .union(M::COMMAND)
                .union(M::CONTROL)
                .union(M::OPTION),
        ),
        FromMods::ANY,
        HENKAN,
    ));

    // ---- Neutral layer ---------------------------------------------------
    //
    // Layer::Any, placed after the Henkan table: a key the Henkan layer does
    // not define falls through to its Neutral meaning.
    #[rustfmt::skip]
    const NEUTRAL: &[Entry] = &[
        (Key::Q,           true,  ansi::ASTERISK),
        (Key::Q,           false, ansi::COLON),
        (Key::W,           false, plain(Key::Comma)),
        (Key::E,           false, plain(Key::Period)),
        (Key::R,           false, plain(Key::P)),
        (Key::T,           false, plain(Key::Y)),
        (Key::Y,           false, plain(Key::F)),
        (Key::U,           false, plain(Key::G)),
        (Key::I,           false, plain(Key::C)),
        (Key::O,           false, plain(Key::R)),
        (Key::P,           false, plain(Key::L)),
        (Key::OpenBracket, false, plain(Key::Slash)),
        (Key::A,           false, plain(Key::A)),
        (Key::S,           false, plain(Key::O)),
        (Key::D,           false, plain(Key::E)),
        (Key::F,           false, plain(Key::U)),
        (Key::G,           false, plain(Key::I)),
        (Key::H,           false, plain(Key::D)),
        (Key::J,           false, plain(Key::H)),
        (Key::K,           false, plain(Key::T)),
        (Key::L,           false, plain(Key::N)),
        (Key::Semicolon,   false, plain(Key::S)),
        (Key::Quote,       true,  ansi::EQUAL),
        (Key::Quote,       false, plain(Key::Hyphen)),
        (Key::Z,           true,  ansi::PLUS),
        (Key::Z,           false, plain(Key::Semicolon)),
        (Key::X,           false, plain(Key::Q)),
        (Key::C,           false, plain(Key::J)),
        (Key::V,           false, plain(Key::K)),
        (Key::B,           false, plain(Key::X)),
        (Key::N,           false, plain(Key::B)),
        (Key::M,           false, plain(Key::M)),
        (Key::Comma,       false, plain(Key::W)),
        (Key::Period,      false, plain(Key::V)),
        (Key::Slash,       false, plain(Key::Z)),
    ];
    rules.extend(expand(
        Scope::Dudrack,
        Layer::Any,
        FromMods {
            mandatory: M::SHIFT,
            optional: super::Optional::Any,
        },
        FromMods::ANY,
        NEUTRAL,
    ));

    // ---- Raw-JIS external keyboards --------------------------------------
    //
    // PC-JIS IME keys onto their Apple-JIS equivalents. Some Windows JIS
    // keyboards deliver 変換 as the grave position, so that is caught too.
    rules.extend([
        emit(
            Scope::RawJis,
            Layer::Any,
            Key::JapanesePcNfer,
            FromMods::ANY,
            plain(Key::JapaneseEisuu),
        ),
        emit(
            Scope::RawJis,
            Layer::Any,
            Key::JapanesePcXfer,
            FromMods::ANY,
            plain(Key::JapaneseKana),
        ),
        emit(
            Scope::RawJis,
            Layer::Any,
            Key::Grave,
            FromMods::ANY,
            plain(Key::JapaneseKana),
        ),
    ]);

    // ---- The keyboard at the other machine -------------------------------
    //
    // A PC keyboard has right control where a Mac has nothing much, and command is
    // the modifier a person reaches for most on the machine they are typing *into*.
    // The Dudrack keyboards have their own answer to that in caps lock and tab.
    //
    // Scoped to what arrives over the link rather than to external keyboards in
    // general: a keyboard at the other machine and one under the person's other hand
    // are different keyboards, however alike they are printed.
    rules.push(emit(
        Scope::Forwarded,
        Layer::Any,
        Key::RightControl,
        FromMods::ANY,
        plain(Key::LeftCommand),
    ));

    // Every key whose JIS label differs from the US label at the same
    // position, so the keyboard behaves as it is printed. Order within the
    // table does not matter: the unshifted entries do not tolerate shift, so
    // the two halves cannot overlap. Option is tolerated, which keeps the JIS
    // reading intact under option chords.
    #[rustfmt::skip]
    const RAW_JIS: &[Entry] = &[
        // Number row: the shifted symbols differ.
        (Key::Digit2,        true,  ansi::DOUBLE_QUOTE),
        (Key::Digit6,        true,  ansi::AMPERSAND),
        (Key::Digit7,        true,  ansi::SINGLE_QUOTE),
        (Key::Digit8,        true,  ansi::LEFT_PAREN),
        (Key::Digit9,        true,  ansi::RIGHT_PAREN),
        // JIS: -/= and ^/~.
        (Key::Hyphen,        true,  ansi::EQUAL),
        (Key::EqualSign,     true,  ansi::TILDE),
        (Key::EqualSign,     false, ansi::CARET),
        // JIS: @/`, [/{, ]/}.
        (Key::OpenBracket,   true,  ansi::BACKTICK),
        (Key::OpenBracket,   false, ansi::AT),
        (Key::CloseBracket,  true,  ansi::LEFT_BRACE),
        (Key::CloseBracket,  false, ansi::LEFT_BRACKET),
        // JIS `]`/`}` is HID 0x32 on most keyboards and HID 0x31 on some, so
        // both wirings are covered.
        (Key::NonUsPound,    true,  ansi::RIGHT_BRACE),
        (Key::NonUsPound,    false, ansi::RIGHT_BRACKET),
        (Key::Backslash,     true,  ansi::RIGHT_BRACE),
        (Key::Backslash,     false, ansi::RIGHT_BRACKET),
        // JIS: ;/+ and :/*.
        (Key::Semicolon,     true,  ansi::PLUS),
        (Key::Quote,         true,  ansi::ASTERISK),
        (Key::Quote,         false, ansi::COLON),
        // JIS-only positions.
        (Key::International1, true,  ansi::UNDERSCORE),
        (Key::International1, false, ansi::BACKSLASH),
        (Key::International3, true,  ansi::PIPE),
        (Key::International3, false, ansi::BACKSLASH),
    ];
    const RAW_JIS_TOLERATED: M = M::CAPS_LOCK
        .union(M::CONTROL)
        .union(M::COMMAND)
        .union(M::OPTION);
    rules.extend(expand(
        Scope::RawJis,
        Layer::Any,
        FromMods::mandatory(M::SHIFT, RAW_JIS_TOLERATED),
        FromMods::tolerating(RAW_JIS_TOLERATED),
        RAW_JIS,
    ));

    // ---- Home / End ------------------------------------------------------
    //
    // Bare Home/End are app-dependent on macOS; Cmd+Left/Right is the
    // line-navigation pair every app agrees on. Extra modifiers pass through,
    // so shift still selects.
    rules.extend([
        emit(
            Scope::Any,
            Layer::Any,
            Key::Home,
            FromMods::ANY,
            (Key::LeftArrow, M::COMMAND),
        ),
        emit(
            Scope::Any,
            Layer::Any,
            Key::End,
            FromMods::ANY,
            (Key::RightArrow, M::COMMAND),
        ),
    ]);

    Layout::new(rules, vec![TRACKPOINT_KEYBOARD_II])
}
