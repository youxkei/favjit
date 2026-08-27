//! The Noise session the link's records travel in.
//!
//! The construction itself is `favjit-noise`'s (ADR-0005): `host-sim` has to run
//! the same construction to stand in for the machine on the other end of it,
//! which is why it lives in a crate neither `engine` nor `host-sim` depends on
//! the other to reach. Re-exported here only for [`crate::pairing`] and
//! [`crate::link`]'s own use, not for a caller outside them.

pub(crate) use favjit_noise::{keypair, Responder, Session};
