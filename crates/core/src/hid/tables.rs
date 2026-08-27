//! What the layout asks of these tables, checked against what they can express.
//!
//! Two directions of one table sit between the layout and a HID device — usage to
//! key on the way in and key to usage on the way out ([`super::usage`]) — and a gap
//! in either is silent in a way nothing else is. A rule whose `from` key no usage
//! names never fires; a rule whose output has no usage fails at the moment the user
//! presses it. Both are reported at runtime, which only helps someone who is
//! watching.
//!
//! So the layout is walked here and every key it mentions is required to be
//! expressible. In the same crate as the layout, which is the point: the table and
//! the rules that lean on it can be pressed against each other without a machine
//! anywhere near them.

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

/// Keys that arrive and cannot be sent.
///
/// The list is named rather than the case simply tolerated, because it is the
/// hardest kind of gap to notice: a key pressed on a keyboard whose rules do not
/// mention it is emitted as itself, so one that cannot be sent is a key that
/// vanishes. A key joining this list by accident fails here instead of going
/// quiet.
///
/// `Fn` is here because it is read from Apple's top-case page and sending it
/// would need that page's own report, which the layout never asks for: the rule
/// that mentions `Fn` converts it to a command key, and it is scoped to the
/// built-in keyboard, which is the only keyboard that reports it at all.
const DROPPED: &[Key] = &[Key::Fn];

#[test]
fn only_the_named_keys_are_dropped_when_passing_through() {
    // A press no rule matches is emitted as itself, so the output direction has to
    // cover the input direction and not just the rules.
    let mut unsendable: Vec<Key> = readable()
        .into_iter()
        .filter(|key| usage::of(*key).is_none())
        .collect();
    let mut named = DROPPED.to_vec();
    unsendable.sort_unstable_by_key(|k| format!("{k:?}"));
    named.sort_unstable_by_key(|k| format!("{k:?}"));

    assert_eq!(
        unsendable, named,
        "the set of keys that arrive but cannot be sent has changed"
    );
}

#[test]
fn the_two_directions_agree_wherever_both_have_an_answer() {
    // Written out separately, so nothing but this stops them drifting apart. A
    // usage that read as one key and wrote as another would convert correctly and
    // send the wrong keystroke.
    let mut disagreed = Vec::new();
    for page in PAGES {
        for usage_number in 0..=0xFFFFu32 {
            let Some(key) = usage::named(page, usage_number) else {
                continue;
            };
            // A key that cannot be sent at all is [`DROPPED`]'s to account for; what
            // this is about is the pair that can both be read and written.
            if DROPPED.contains(&key) {
                continue;
            }
            match usage::of(key) {
                Some((written_page, written))
                    if (written_page, u32::from(written)) == (page, usage_number) => {}
                other => disagreed.push((page, usage_number, key, other)),
            }
        }
    }

    assert!(
        disagreed.is_empty(),
        "the read and written usage differ for these: {disagreed:?}"
    );
}

#[test]
fn the_unmapped_list_says_what_is_actually_unmapped() {
    let readable = readable();
    let wrongly_listed: Vec<Key> = usage::UNMAPPED
        .iter()
        .copied()
        .filter(|key| readable.contains(key))
        .collect();

    assert!(
        wrongly_listed.is_empty(),
        "listed as unmapped but a usage names them: {wrongly_listed:?}"
    );
}

#[test]
fn a_pointer_element_is_watched_and_a_neighbour_on_its_page_is_not() {
    // The generic desktop page carries the system power and sleep controls beside
    // X and Y, and admitting the page would relay those.
    assert!(usage::pointer(0x01, usage::POINTER_X));
    assert!(usage::pointer(0x01, usage::POINTER_Y));
    assert!(usage::pointer(0x01, usage::POINTER_WHEEL));
    assert!(usage::pointer(0x09, 1));
    assert!(!usage::pointer(0x01, 0x81), "system power down");
    assert!(!usage::pointer(0x01, 0x06), "the keyboard collection");
}
