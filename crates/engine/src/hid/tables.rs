//! What the layout asks of `favjit-hid`'s tables, checked against what they can
//! express.
//!
//! A rule whose `from` key no usage names never fires; a rule whose output has no
//! usage fails at the moment the user presses it. Both are reported at runtime,
//! which only helps someone who is watching — so the layout is walked here and
//! every key it mentions is required to be expressible.
//!
//! Whether the table's two directions agree with *each other*, and which keys the
//! read direction names with no way to send them back out, are `favjit-hid`'s own
//! question to ask of itself, answered in that crate's tests; this is the layout's
//! question to ask of the table.

#![cfg(test)]

use super::usage;
use crate::{Action, Key, Layout};

/// Every page a keyboard's keys or controls are read from.
///
/// The pointer's own usages are on the generic desktop page too, and they are not
/// keys — but [`usage::named`] does not answer for them, so walking the page reads
/// only the one control that lives there.
const PAGES: [u32; 5] = [0x07, 0x00FF, 0x0C, 0xFF01, 0x01];

/// Every key the tables can read, in usage order.
fn readable() -> Vec<Key> {
    PAGES
        .iter()
        .flat_map(|&page| (0..=0xFFFFu32).filter_map(move |usage| usage::named(page, usage)))
        .collect()
}

#[test]
fn every_key_a_rule_matches_on_can_arrive() {
    let readable = readable();
    let missing: Vec<Key> = Layout::dudrack()
        .rules()
        .iter()
        .map(|rule| rule.from)
        .filter(|key| !readable.contains(key))
        .collect();

    assert!(
        missing.is_empty(),
        "no usage names these keys, so their rules can never fire: {missing:?}"
    );
}

#[test]
fn every_key_a_rule_emits_can_be_sent() {
    let mut missing = Vec::new();
    for rule in Layout::dudrack().rules() {
        // Swallow and HoldHenkan send nothing, so they ask nothing of the output
        // table; the tap and hold of a TapHold are both sent, so both count.
        let emitted: &[Key] = match &rule.action {
            Action::Emit { key, .. } => &[*key],
            Action::TapHold { hold, tap, .. } => &[*hold, *tap],
            Action::Swallow | Action::HoldHenkan => &[],
        };
        for key in emitted {
            if usage::of(*key).is_none() {
                missing.push(*key);
            }
        }
    }

    assert!(
        missing.is_empty(),
        "no usage for these, so a rule that emits one fails when pressed: {missing:?}"
    );
}
