//! The code exchange itself, run by both ends for real and by the suite's
//! simulated peer (ADR-0004, ADR-0006).
//!
//! A crate of its own rather than a module of `engine`'s: `engine::pairing::pair`
//! and `pair_with` run this for real, and the end-to-end suite's simulated peer has
//! to run the same arithmetic to stand in for the machine that is missing from a
//! single-ended test — a hand-rolled fake would let the two ends' constructions
//! disagree without a test ever showing it. `engine` cannot depend on its own test
//! double, so what both of them call has to live somewhere neither is, which is
//! here. Everything else about pairing — the identity file, the order the exchange
//! runs in — stays `engine`'s: a real host reaches [`Code`], [`OFFER`] and
//! [`SEALED_KEY`] because it has to read and write fixed-length messages
//! (`engine::pairing` re-exports them at their old path), and [`Entropy`] because
//! it has to supply bytes, but never `offer`, `answer`, [`Started`] or [`Secret`] —
//! those run only inside `engine`'s own `pair` and `pair_with`, and inside the peer
//! the suite plays in their place.
//!
//! The one impure step is the bytes a scalar and a code are made of, which arrive
//! through [`Entropy`]: one call into the machine, and one the suite can answer
//! with bytes it chose, so what crosses in a test is what would cross for real.

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use rand_core::{CryptoRng, RngCore};
use spake2::{Ed25519Group, Identity as PakeIdentity, Password, Spake2};

/// How many digits a pairing code has (ADR-0004).
///
/// Six is enough because the code is single-use: `engine::pairing::pair` serves one
/// attempt and then the run is over, so the only way to test a guess is to make
/// another connection against a code that machine has newly shown.
pub const DIGITS: usize = 6;

/// The code one machine shows and the other is given.
///
/// Digits rather than a number, because what a person reads off a screen and types
/// is characters — and a leading zero is one of them, which a number would lose.
pub type Code = [u8; DIGITS];

/// How many bytes each end of the code exchange writes.
///
/// Stated here rather than in either host, because both ends read exactly this many:
/// a message written at any other length is a read waiting for bytes nobody will
/// send, which hangs rather than fails (`engine::link` states its own for the same
/// reason).
///
/// A group element and the byte in front of it that says which side sent it — not the
/// element alone, which is what the key length would suggest: an exchange whose two
/// halves were indistinguishable would agree with a machine reflecting its own
/// message back.
pub const OFFER: usize = 33;

/// What both ends bind the code exchange to.
///
/// A constant rather than something derived from either machine, since there is
/// nothing both ends know before they have agreed on anything — and two machines
/// deriving against different strings agree on nothing however right the code is.
const CONTEXT: &[u8] = b"favjit pairing v1";

/// The nonce each direction seals its static key under.
///
/// Fixed and different per direction, which holds because the key seals exactly one
/// message each way: what has to be avoided is one nonce twice under one key, and a
/// secret derived from one code and one connection is used once in each direction.
const FROM_THE_SOURCE: &[u8; 12] = b"source->sink";
const FROM_THE_SINK: &[u8; 12] = b"sink->source";

/// How many bytes a static key sealed under the shared secret takes.
///
/// The identity's public key, which is what gets sealed, is 32 bytes
/// (`engine::pairing::KEY`) plus the 16-byte tag the cipher adds to it. Stated rather
/// than measured off a sealed message, because both ends read exactly this many
/// before there is anything to measure.
pub const SEALED_KEY: usize = 32 + 16;

/// Bytes from the machine that nothing can predict.
///
/// [`favjit_host::Entropy`], and not a trait of this crate's own: a host crate
/// depends on `favjit-host` and not on this one, so a trait declared here would be
/// one this exchange could call for itself and a host could never implement.
pub use favjit_host::Entropy;

/// The machine's entropy, in the shape the exchange draws on it.
///
/// Nothing is generated here — every byte comes from [`Entropy::fill`]. A run that
/// could not be given bytes leaves `enough` false rather than raising, because the
/// interface this stands in for cannot fail, and going on with zeros would be a
/// scalar anyone can guess.
struct Supplied<'a> {
    entropy: &'a mut dyn Entropy,
    enough: bool,
}

impl<'a> Supplied<'a> {
    fn from(entropy: &'a mut dyn Entropy) -> Self {
        Self {
            entropy,
            enough: true,
        }
    }
}

impl RngCore for Supplied<'_> {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, into: &mut [u8]) {
        if !self.entropy.fill(into) {
            self.enough = false;
            into.fill(0);
        }
    }

    /// The failure is reported through `enough` rather than here.
    ///
    /// An error value at this surface would have to carry a code this crate has no
    /// use for, and the caller already has to look at `enough` before trusting what
    /// came out — one place to check beats two that can disagree.
    fn try_fill_bytes(&mut self, into: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(into);
        Ok(())
    }
}

// Nothing here weakens what the machine handed over, so what this stands in for is as
// good as the machine's own source.
impl CryptoRng for Supplied<'_> {}

/// The half of the exchange the machine offering itself sends.
///
/// Public because the suite drives the two ends against each other, and an end
/// standing in for its peer has to produce what the peer would.
pub fn offer(code: Code, entropy: &mut dyn Entropy) -> Option<(Started, [u8; OFFER])> {
    let mut supplied = Supplied::from(entropy);
    let (state, offer) = Spake2::<Ed25519Group>::start_a_with_rng(
        &Password::new(code),
        &PakeIdentity::new(CONTEXT),
        &PakeIdentity::new(CONTEXT),
        &mut supplied,
    );
    match supplied.enough {
        true => Some((Started(state), offer.try_into().ok()?)),
        false => None,
    }
}

/// The half the machine being paired to sends back, and the secret it agrees on.
///
/// One step rather than two, because this end has the other's message before it
/// starts: there is no state to hold between them.
pub fn answer(
    code: Code,
    offer: &[u8; OFFER],
    entropy: &mut dyn Entropy,
) -> Option<([u8; OFFER], Secret)> {
    let mut supplied = Supplied::from(entropy);
    // `start_b` against the other end's `start_a`: two machines on the same side of
    // one exchange agree on nothing.
    let (state, answer) = Spake2::<Ed25519Group>::start_b_with_rng(
        &Password::new(code),
        &PakeIdentity::new(CONTEXT),
        &PakeIdentity::new(CONTEXT),
        &mut supplied,
    );
    if !supplied.enough {
        return None;
    }
    // An error here is a message that is not a point on the group at all, which is
    // something else speaking rather than the wrong code: wrong digits produce a
    // well-formed message and a different secret, and show up where the key will not
    // open.
    let secret = state.finish(offer).ok()?;
    Some((answer.try_into().ok()?, Secret(secret)))
}

/// The exchange in flight at the end that started it.
pub struct Started(Spake2<Ed25519Group>);

impl Started {
    /// Take the other end's answer, and the secret it agrees on.
    pub fn finish(self, answer: &[u8; OFFER]) -> Option<Secret> {
        self.0.finish(answer).ok().map(Secret)
    }
}

/// What the code exchange agreed on, and what the static keys cross under.
///
/// Held rather than handed back as bytes, so that nothing outside can seal under it
/// with a nonce of its own choosing: the two that are used are the two that are
/// stated, and one nonce twice under one key is the mistake that has to be
/// impossible rather than avoided.
pub struct Secret(Vec<u8>);

impl Secret {
    fn cipher(&self) -> Option<ChaCha20Poly1305> {
        // The exchange over this group produces thirty-two bytes, which is the key
        // length. A secret of any other length is one this cannot key, and saying so
        // beats padding it into something that looks like a key.
        let key = Key::try_from(self.0.as_slice()).ok()?;
        Some(ChaCha20Poly1305::new(&key))
    }

    /// A static key, sealed for the other end to open.
    ///
    /// The key rather than the identity it is the public half of: what crosses is
    /// that public half, and an end standing in for its peer in the suite holds that
    /// and no private half to go with it.
    pub fn seal(&self, key: &[u8], from: Side) -> Option<[u8; SEALED_KEY]> {
        let cipher = self.cipher()?;
        let nonce = Nonce::try_from(from.nonce().as_slice()).ok()?;
        cipher.encrypt(&nonce, key).ok()?.try_into().ok()
    }

    /// The other end's static key, or nothing when it will not open.
    ///
    /// Nothing is what a wrong code looks like from either end: a secret derived from
    /// other digits opens nothing sealed under these.
    pub fn open(&self, sealed: &[u8; SEALED_KEY], from: Side) -> Option<Vec<u8>> {
        let cipher = self.cipher()?;
        let nonce = Nonce::try_from(from.nonce().as_slice()).ok()?;
        cipher.decrypt(&nonce, sealed.as_slice()).ok()
    }
}

/// Which machine a sealed key came from.
///
/// The nonce is per direction, which is what lets one secret seal one message each
/// way: naming the direction rather than passing the nonce keeps the two constants
/// somewhere they cannot be swapped by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Source,
    Sink,
}

impl Side {
    fn nonce(self) -> &'static [u8; 12] {
        match self {
            Self::Source => FROM_THE_SOURCE,
            Self::Sink => FROM_THE_SINK,
        }
    }
}
