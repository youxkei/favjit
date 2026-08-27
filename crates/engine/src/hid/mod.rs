//! HID as far as favjit speaks it: what a page and a usage mean, and the reports a
//! device is told with.
//!
//! The vocabulary — [`page`], [`usage`], the bytes a report is made of — is
//! `favjit-hid`'s (ADR-0005), so that both machines' `engine`s and `host-sim`
//! agree on it without either depending on the other. Re-exported here only for
//! this module's own use, not for a caller outside it: a caller reading a page
//! or a usage number has no reason to reach it through `engine` rather than
//! `favjit-hid` directly.
//!
//! What stays here is the decision no table can make on its own: which usages
//! ride together on one report, and when a report is rebuilt from scratch rather
//! than carried forward ([`report::Keyboard`], [`report::render`]).

pub mod report;
mod tables;

pub use favjit_hid::{page, usage};
