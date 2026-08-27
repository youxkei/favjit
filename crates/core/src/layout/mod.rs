//! The conversion pipeline: one ordered rule table, first match wins.
//!
//! ADR-0003 puts every source behind one pipeline, and calls the rules
//! themselves the valuable part of the project. So the engine here knows
//! nothing about any particular layout — the layout is the table in
//! [`dudrack`], and swapping it is a data change.

mod dudrack;

use core::time::Duration;

use crate::{DeviceInfo, DeviceMatch, Instant, Key, Modifiers, Scope};

/// Which modifiers a rule requires, and which it tolerates.
///
/// The distinction decides what reaches the OS: a mandatory modifier is
/// consumed by the rule, an optional one is passed through onto the injected
/// event. `shift` on a symbol rule is mandatory because the shift *is* the
/// symbol; `command` is optional because the user still means command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FromMods {
    pub mandatory: Modifiers,
    pub optional: Optional,
}

/// The modifiers a rule tolerates on top of its mandatory ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optional {
    /// Anything at all.
    Any,
    /// Exactly these, and nothing else.
    Only(Modifiers),
}

impl FromMods {
    /// Matches whatever is held.
    pub const ANY: Self = Self {
        mandatory: Modifiers::NONE,
        optional: Optional::Any,
    };

    pub const fn mandatory(mandatory: Modifiers, optional: Modifiers) -> Self {
        Self {
            mandatory,
            optional: Optional::Only(optional),
        }
    }

    pub const fn tolerating(optional: Modifiers) -> Self {
        Self {
            mandatory: Modifiers::NONE,
            optional: Optional::Only(optional),
        }
    }

    fn accepts(&self, held: Modifiers) -> bool {
        if !held.contains(self.mandatory) {
            return false;
        }
        match self.optional {
            Optional::Any => true,
            Optional::Only(tolerated) => held.without(self.mandatory).is_subset_of(tolerated),
        }
    }
}

/// Whether a rule is confined to one layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Both layers.
    Any,
    /// Only while the Henkan key is held.
    Henkan,
    /// Only while it is not.
    Neutral,
}

/// What a rule does with the key that matched it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Send this key instead, with these modifiers added.
    Emit { key: Key, modifiers: Modifiers },
    /// Send nothing.
    Swallow,
    /// Hold the Henkan layer for as long as the physical key is held.
    HoldHenkan,
    /// A modifier while held, a key when tapped.
    ///
    /// `hold` is lazy: it reaches the OS only once a key that needs it arrives,
    /// so a tap cannot leave a stray modifier behind. `tap` is sent only if the
    /// key is released alone and within `tap_window`; held longer than that and
    /// then abandoned, the key produces nothing at all.
    TapHold {
        hold: Key,
        tap: Key,
        tap_window: Duration,
    },
}

/// One entry of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub scope: Scope,
    pub layer: Layer,
    pub from: Key,
    pub mods: FromMods,
    pub action: Action,
}

/// What the pipeline decided for one physical key press.
///
/// A press that matched no rule resolves to `Emit` of the key itself: the
/// pipeline converts what it has rules for and gets out of the way otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// `consumed` and `added` are kept apart rather than folded into one
    /// modifier set, because only the caller knows which modifiers are actually
    /// in effect at the OS: a lazy hold is matched against here, but must not be
    /// stamped onto an event before it has been sent.
    Emit {
        key: Key,
        consumed: Modifiers,
        added: Modifiers,
    },
    Swallow,
    HoldHenkan,
    TapHold {
        hold: Key,
        tap: Key,
        deadline: Instant,
    },
}

/// A rule table plus the device configuration it is read against.
#[derive(Debug, Clone)]
pub struct Layout {
    rules: Vec<Rule>,
    /// External keyboards typed in Dudrack rather than by their JIS labels.
    dudrack_externals: Vec<DeviceMatch>,
}

impl Layout {
    pub fn new(rules: Vec<Rule>, dudrack_externals: Vec<DeviceMatch>) -> Self {
        Self {
            rules,
            dudrack_externals,
        }
    }

    /// The layout this repository is built around: JIS-Dvorak with a Henkan
    /// layer on the Dudrack keyboards, JIS labels on every other external one.
    pub fn dudrack() -> Self {
        dudrack::layout()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    fn in_scope(&self, scope: Scope, info: &DeviceInfo) -> bool {
        let dudrack_external = || self.dudrack_externals.iter().any(|m| m.matches(info));
        match scope {
            Scope::Any => true,
            Scope::BuiltIn => info.is_built_in,
            Scope::Dudrack => info.is_built_in || dudrack_external(),
            Scope::DudrackExternal => !info.is_built_in && dudrack_external(),
            Scope::RawJis => !info.is_built_in && !dudrack_external(),
            Scope::Forwarded => crate::link::is_from_source(info.id),
        }
    }

    /// Resolve one physical key press.
    ///
    /// `held` is what rules are matched against: the modifier set after
    /// conversion, so a remapped Caps Lock arrives here as `CONTROL`. Rules
    /// therefore never see the pre-conversion modifier, which is what stops a
    /// remap from feeding itself — the built-in `Fn` → right command remap
    /// cannot trigger the Henkan layer that the physical right command key
    /// holds. A lazy hold is included even before it has been sent to the OS,
    /// because being matched against is the whole purpose of holding it.
    ///
    /// `at` dates the press, which is what a tap window is measured from.
    pub fn resolve(
        &self,
        info: &DeviceInfo,
        key: Key,
        at: Instant,
        held: Modifiers,
        henkan_held: bool,
    ) -> Outcome {
        for rule in &self.rules {
            if rule.from != key {
                continue;
            }
            let layer_ok = match rule.layer {
                Layer::Any => true,
                Layer::Henkan => henkan_held,
                Layer::Neutral => !henkan_held,
            };
            if !layer_ok || !self.in_scope(rule.scope, info) || !rule.mods.accepts(held) {
                continue;
            }
            return match rule.action {
                Action::Emit { key, modifiers } => Outcome::Emit {
                    key,
                    consumed: rule.mods.mandatory,
                    added: modifiers,
                },
                Action::Swallow => Outcome::Swallow,
                Action::HoldHenkan => Outcome::HoldHenkan,
                Action::TapHold {
                    hold,
                    tap,
                    tap_window,
                } => Outcome::TapHold {
                    hold,
                    tap,
                    deadline: at.saturating_add(tap_window),
                },
            };
        }
        Outcome::Emit {
            key,
            consumed: Modifiers::NONE,
            added: Modifiers::NONE,
        }
    }
}
