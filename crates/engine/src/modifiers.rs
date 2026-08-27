//! Modifier flags, and the modifier keys a set of them is delivered as.

/// A set of modifiers in effect for one key event.
///
/// Flags are side-insensitive — there is no separate bit for left and right —
/// because no layout rule distinguishes the two. Where a side matters it is
/// carried by [`crate::Key`] instead, which is why the left/right command swap
/// on the built-in keyboard is expressible while `COMMAND` alone is enough to
/// match against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1 << 0);
    pub const CONTROL: Self = Self(1 << 1);
    pub const OPTION: Self = Self(1 << 2);
    pub const COMMAND: Self = Self(1 << 3);
    pub const CAPS_LOCK: Self = Self(1 << 4);

    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub(crate) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }
}

/// The modifier keys in the order this set numbers them.
///
/// A numbering of its own, and not any platform's: what a host writes is its own
/// report's business, and one that reused these bit positions would be agreeing
/// with `engine` by coincidence. Caps Lock is here because a rule can be matched
/// against it while the key is held; a host with nowhere to put it says so.
const ORDER: [crate::Key; 9] = [
    crate::Key::LeftControl,
    crate::Key::LeftShift,
    crate::Key::LeftOption,
    crate::Key::LeftCommand,
    crate::Key::RightControl,
    crate::Key::RightShift,
    crate::Key::RightOption,
    crate::Key::RightCommand,
    crate::Key::CapsLock,
];

/// The modifier keys that must be down at the OS for one injected event.
///
/// Keys, where a rule matches on [`Modifiers`], because the two are different
/// questions: a rule asks *which modifier* — shift, whichever key produced it —
/// and the OS is told *which keys*. Answering the second here rather than leaving
/// a host to pick a side is what keeps it drivable by the suite (ADR-0006), and it
/// is what lets a release say the right command is still down instead of trading it
/// for the left one.
///
/// **A key absent from the set must not reach the OS.** That is what a rule's
/// mandatory modifier being consumed means: the shift that selected `'` out of a
/// layer is gone from the set the event carries, and anything that put it back
/// would deliver `"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct ModifierKeys(u16);

impl ModifierKeys {
    pub const NONE: Self = Self(0);

    /// The keys named, ignoring anything that is not a modifier key.
    ///
    /// `cfg(test)`: a run builds a set one key at a time, as each event arrives
    /// and while it is still held — naming a whole set at once is stating an
    /// expectation about one, which is a test's to do.
    #[cfg(test)]
    pub(crate) const fn of(keys: &[crate::Key]) -> Self {
        let mut out = Self::NONE;
        let mut at = 0;
        while at < keys.len() {
            out = out.with(keys[at]);
            at += 1;
        }
        out
    }

    pub(crate) const fn with(self, key: crate::Key) -> Self {
        match bit_of(key) {
            Some(bit) => Self(self.0 | bit),
            None => self,
        }
    }

    pub(crate) const fn contains(self, key: crate::Key) -> bool {
        match bit_of(key) {
            Some(bit) => self.0 & bit == bit,
            None => false,
        }
    }

    /// Every key in the set, in this type's own order.
    pub(crate) fn keys(self) -> impl Iterator<Item = crate::Key> {
        ORDER.into_iter().filter(move |key| self.contains(*key))
    }

    /// The modifiers these keys stand for, which is what rules are matched
    /// against.
    pub(crate) fn kinds(self) -> Modifiers {
        self.keys()
            .fold(Modifiers::NONE, |out, key| match modifier_of(key) {
                Some(modifier) => out.union(modifier),
                None => out,
            })
    }

    /// Drop every key standing for one of these modifiers.
    ///
    /// Both sides go, and deliberately: a rule that consumed shift consumed the
    /// modifier, so a shift still held on the other hand would select the shifted
    /// character just as well.
    pub(crate) fn without_kinds(self, modifiers: Modifiers) -> Self {
        self.keys()
            .filter(|key| match modifier_of(*key) {
                Some(modifier) => !modifiers.contains(modifier),
                None => true,
            })
            .fold(Self::NONE, Self::with)
    }

    /// Add a key for every one of these modifiers the set has none for.
    ///
    /// The left-hand key, there being no side in a modifier to read. A rule that
    /// wants a particular one emits that key instead, which is how the built-in
    /// keyboard's command swap is written.
    ///
    /// Caps Lock is not among them: it is a lock and not a modifier a keystroke can
    /// borrow, so a rule asking for it as an added modifier is asking for something
    /// pressing the key would not give it either.
    pub(crate) fn with_kinds(self, modifiers: Modifiers) -> Self {
        const LEFT: [(Modifiers, crate::Key); 4] = [
            (Modifiers::SHIFT, crate::Key::LeftShift),
            (Modifiers::CONTROL, crate::Key::LeftControl),
            (Modifiers::OPTION, crate::Key::LeftOption),
            (Modifiers::COMMAND, crate::Key::LeftCommand),
        ];
        let missing = modifiers.without(self.kinds());
        LEFT.iter().fold(self, |out, &(modifier, key)| {
            match missing.contains(modifier) {
                true => out.with(key),
                false => out,
            }
        })
    }

    /// The set as it is stored, for a trace to write and read back.
    pub(crate) const fn bits(self) -> u16 {
        self.0
    }

    pub(crate) const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }
}

/// The modifier this key contributes while it is held, if it is one.
///
/// A free function rather than a method on [`crate::Key`]: that type is
/// `favjit-hid`'s, shared with `host-sim` as the vocabulary both machines'
/// `engine`s agree on, and which keys act as which modifier is this layout's own
/// decision over it, not part of that vocabulary (ADR-0005).
///
/// `Fn` is absent deliberately: macOS reports it, but it carries no modifier bit
/// that a layout rule can match on, and on the built-in keyboard it is remapped to
/// a command key before it could contribute one.
pub(crate) const fn modifier_of(key: crate::Key) -> Option<Modifiers> {
    match key {
        crate::Key::LeftShift | crate::Key::RightShift => Some(Modifiers::SHIFT),
        crate::Key::LeftControl | crate::Key::RightControl => Some(Modifiers::CONTROL),
        crate::Key::LeftOption | crate::Key::RightOption => Some(Modifiers::OPTION),
        crate::Key::LeftCommand | crate::Key::RightCommand => Some(Modifiers::COMMAND),
        crate::Key::CapsLock => Some(Modifiers::CAPS_LOCK),
        _ => None,
    }
}

/// Which bit stands for a key, for a key this set can hold at all.
const fn bit_of(key: crate::Key) -> Option<u16> {
    let mut at = 0;
    while at < ORDER.len() {
        // Compared by discriminant rather than matched, because `Key` is
        // `#[non_exhaustive]` and a match here would have to name every other key
        // to say nothing about it.
        if ORDER[at] as u8 == key as u8 {
            return Some(1 << at);
        }
        at += 1;
    }
    None
}
