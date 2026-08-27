//! Who this machine will take input from (ADR-0004).
//!
//! Here rather than in a host because none of it touches the machine: a key is
//! bytes, a list of them is text, and whether one is in the list is a question
//! with an answer. What the hosts do with the answer — reading the file, opening a
//! socket — is the part that cannot be tested away from the platform, and keeping
//! this out of there is what keeps that part small (ADR-0006).
//!
//! The code exchange is here for the same reason, arithmetic and all. Which side
//! starts as which, what the exchange binds to, which direction each nonce seals and
//! that what is sealed is the static key are as much an agreement between the two
//! machines as the lengths are — and an agreement written once per platform fails as
//! a key that will not open, which is what a wrong code looks like too. The one
//! impure step is the bytes a scalar and a code are made of, which arrive through
//! [`Entropy`]: one call into the machine, and one the suite can answer with bytes it
//! chose, so what crosses in a test is what would cross for real.

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use rand_core::{CryptoRng, RngCore};
use spake2::{Ed25519Group, Identity as PakeIdentity, Password, Spake2};

/// How long an identity is.
///
/// One length rather than "whatever was in the file": a shorter string that
/// happens to be valid hex would be pinned as an identity nothing can present,
/// which looks paired and refuses everything.
pub const KEY: usize = 32;

/// A machine's long-lived identity, as it sits in a file.
///
/// Both halves together. Deriving the public key from the private one would mean
/// this crate naming the curve the keys are on, which is the platform's choice and
/// not this one's — and two files could disagree about one identity, where one file
/// cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    private: Vec<u8>,
    public: Vec<u8>,
}

impl Identity {
    /// Take the two halves as they were generated.
    pub fn new(private: Vec<u8>, public: Vec<u8>) -> Option<Self> {
        (private.len() == KEY && public.len() == KEY).then_some(Self { private, public })
    }

    /// Read what was stored, or nothing if it is not an identity.
    ///
    /// `None` rather than a partial read: a file of the wrong length is either
    /// somebody else's or a truncated write, and treating either as an identity
    /// would mean presenting a key no peer has pinned while looking configured.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        (bytes.len() == KEY * 2).then(|| Self {
            private: bytes[..KEY].to_vec(),
            public: bytes[KEY..].to_vec(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = self.private.clone();
        bytes.extend_from_slice(&self.public);
        bytes
    }

    pub fn private(&self) -> &[u8] {
        &self.private
    }

    pub fn public(&self) -> &[u8] {
        &self.public
    }

    /// The identity as a person sees it named.
    ///
    /// What it is for is telling one machine from another in a log line and in what
    /// `--identity` prints — nobody transcribes it, because the code carries the key
    /// across ([`pair`]). The whole key rather than a digest for that reason: with
    /// no comparison to shorten, a digest is a step that buys nothing and a second
    /// thing a reader has to know how to compute.
    pub fn fingerprint(&self) -> String {
        hex(&self.public)
    }
}

/// The keys this machine accepts input from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authorized {
    keys: Vec<Vec<u8>>,
}

impl Authorized {
    /// Read the list out of the file's text.
    ///
    /// A line that is not a key is skipped rather than failing the whole list:
    /// this is a file a person edits, and one stray character should leave the
    /// list short — which refuses a peer — rather than empty, which refuses
    /// everyone, or aborted, which leaves the converter with no list at all.
    pub fn parse(text: &str) -> Self {
        let keys = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(key_from_hex)
            .collect();
        Self { keys }
    }

    /// The text to store for this list with one more key in it.
    ///
    /// Text in, text out: what the file looked like before is kept as it was,
    /// comments and all, because a person put them there.
    pub fn added(text: &str, key: &[u8]) -> String {
        let mut out = String::from(text);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&hex(key));
        out.push('\n');
        out
    }

    pub fn holds(&self, key: &[u8]) -> bool {
        self.keys.iter().any(|pinned| pinned == key)
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// The machine's side of keeping an identity: the file, and its entropy.
///
/// Three operations that only report what happened, so the sequence over them is
/// checkable away from a disk (ADR-0006). None of them decides anything: whether a
/// file that is not an identity may be replaced, and whether a fresh one is worth
/// keeping, are [`identity`]'s.
pub trait IdentityStore {
    /// What is in the file, or nothing when there is no file to read.
    fn read(&mut self) -> Option<Vec<u8>>;

    /// A keypair from the machine.
    fn make(&mut self) -> Option<Identity>;

    /// Keep these bytes, saying whether they were kept.
    fn keep(&mut self, bytes: &[u8]) -> bool;
}

/// Why this machine has no identity to present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoIdentity {
    /// There is a file, and it is not an identity.
    Foreign,
    CannotMake,
    CannotKeep,
}

impl core::fmt::Display for NoIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Foreign => "the identity file is not a keypair",
            Self::CannotMake => "cannot make a keypair",
            Self::CannotKeep => "cannot keep the keypair that was made",
        })
    }
}

/// The identity this machine presents, made on first use.
///
/// Made here rather than by an installer: a machine nobody has paired has nothing
/// to protect, and one that loses the file gets a new identity — which is the right
/// outcome, since a peer that pinned the old one should refuse the new.
///
/// A file that is not an identity is left as it is and refused. Replacing it would
/// throw away an identity a peer may have pinned, and what is actually in it is
/// either somebody else's or a write that did not finish.
///
/// A keypair that cannot be kept is refused rather than used, because the next run
/// would present a different one: a peer that pinned this one would then refuse a
/// machine that looks paired.
pub fn identity(store: &mut dyn IdentityStore) -> Result<Identity, NoIdentity> {
    if let Some(bytes) = store.read() {
        return Identity::from_bytes(&bytes).ok_or(NoIdentity::Foreign);
    }
    let identity = store.make().ok_or(NoIdentity::CannotMake)?;
    if !store.keep(&identity.to_bytes()) {
        return Err(NoIdentity::CannotKeep);
    }
    Ok(identity)
}

/// How many digits a pairing code has (ADR-0004).
///
/// Six is enough because the code is single-use: [`pair`] serves one attempt and
/// then the run is over, so the only way to test a guess is to make another
/// connection against a code this machine has newly shown.
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
/// send, which hangs rather than fails ([`crate::link`] states its own for the same
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
/// The key plus the tag the cipher adds to it. Stated rather than measured off a
/// sealed message, because both ends read exactly this many before there is anything
/// to measure.
pub const SEALED_KEY: usize = KEY + 16;

/// Bytes from the machine that nothing can predict.
///
/// The one thing the exchange needs that is not arithmetic. A trait of its own rather
/// than a method on each end's host, because both ends need exactly this and a second
/// declaration is a second thing to keep in step.
pub trait Entropy {
    /// Fill this, saying whether it could.
    ///
    /// `false` rather than fewer bytes: a scalar drawn from part of a buffer and a
    /// code made from what was available are both something weaker than they look,
    /// and neither is worth carrying on with.
    fn fill(&mut self, into: &mut [u8]) -> bool;
}

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

/// Six digits, from bytes the machine supplied.
///
/// A byte over 249 is thrown away rather than taken modulo ten, because the remainder
/// would make the first six digits likelier than the last four — and six digits have
/// little enough to spend without giving any of it away.
fn code_from(entropy: &mut dyn Entropy) -> Option<Code> {
    let mut code = [0u8; DIGITS];
    let mut filled = 0;
    while filled < DIGITS {
        let mut byte = [0u8; 1];
        if !entropy.fill(&mut byte) {
            return None;
        }
        if byte[0] < 250 {
            code[filled] = b'0' + byte[0] % 10;
            filled += 1;
        }
    }
    Some(code)
}

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
    /// The key rather than the [`Identity`] it is half of: what crosses is the public
    /// half, and an end standing in for its peer in the suite holds that and no
    /// private half to go with it.
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

/// The machine's side of pairing.
///
/// Every operation is one call into the platform and the answer it gave; the order is
/// [`pair`]'s (ADR-0006). What is left here is the machine and nothing else — a
/// screen, a socket, a file — with [`Entropy`] for the bytes, since the exchange
/// itself is arithmetic and lives above this line.
pub trait PairingHost: Entropy {
    /// Put the code in front of the person.
    fn show(&mut self, code: Code);

    /// Wait for a source to connect, and say whether one did.
    ///
    /// The wait ADR-0006 allows a host to hold, for the same reason the event
    /// stream's is: bounding it needs a clock, and `core` has none.
    fn wait_for_a_source(&mut self) -> bool;

    /// Take the source's half of the code exchange.
    fn take_offer(&mut self) -> Option<[u8; OFFER]>;

    fn send_answer(&mut self, answer: &[u8; OFFER]) -> bool;

    /// Take the source's static key, sealed under the shared secret.
    fn take_sealed_key(&mut self) -> Option<[u8; SEALED_KEY]>;

    fn send_sealed_key(&mut self, sealed: &[u8; SEALED_KEY]) -> bool;

    /// Add this key to the list the converting run accepts input from.
    fn authorize(&mut self, key: &[u8]) -> bool;
}

/// How pairing ended.
///
/// One enum for both ends, because what a person is told is the same either way:
/// which machine could not be reached is the only thing that differs, and a name
/// per end says it without a second set of the four they share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paired {
    /// The peer's key is pinned, as this fingerprint.
    Pinned(String),
    /// What arrived would not open under the code, which is what a wrong code looks
    /// like from either end.
    WrongCode,
    /// Nobody answered the code this machine showed.
    NoSource,
    /// The machine showing the code could not be reached.
    NoSink,
    /// The machine could not produce a code.
    NoCode,
    /// The exchange stopped part way — the connection went, or the machine could
    /// not do its half.
    Interrupted,
    /// The key opened and could not be written down.
    CannotKeep,
}

/// One pairing: show a code, serve one attempt, pin what it proves.
///
/// **One attempt, and then the run is over** — which is what makes six digits enough
/// (ADR-0004). Serving a second attempt against a shown code would let an attacker
/// try again at whatever rate this machine accepts connections, and no length of code
/// survives that.
///
/// The code is shown before anything is waited for, because a code produced after a
/// source has connected is a code nobody could have entered.
///
/// Both keys cross. This machine pins the source's so its converting run will accept
/// input, and the source is given this machine's because it is the end that opens the
/// session and cannot address a machine whose key it does not hold (ADR-0017).
pub fn pair(mine: &Identity, host: &mut dyn PairingHost) -> Paired {
    let Some(code) = code_from(host) else {
        return Paired::NoCode;
    };
    host.show(code);
    if !host.wait_for_a_source() {
        return Paired::NoSource;
    }

    let Some(offer) = host.take_offer() else {
        return Paired::Interrupted;
    };
    let Some((answer, secret)) = answer(code, &offer, host) else {
        return Paired::Interrupted;
    };
    if !host.send_answer(&answer) {
        return Paired::Interrupted;
    }

    let Some(sealed) = host.take_sealed_key() else {
        return Paired::Interrupted;
    };
    // Before anything of this machine's is sent back: a source that had the code
    // wrong learns nothing from a pairing attempt it could not complete.
    let Some(peer) = secret.open(&sealed, Side::Source) else {
        return Paired::WrongCode;
    };
    let Some(sealed_mine) = secret.seal(mine.public(), Side::Sink) else {
        return Paired::Interrupted;
    };
    if !host.send_sealed_key(&sealed_mine) {
        return Paired::Interrupted;
    }

    // Last, because pinning is what the exchange was for: a key written down before
    // it was opened under the code would be a key the code did not vouch for.
    match host.authorize(&peer) {
        true => Paired::Pinned(hex(&peer)),
        false => Paired::CannotKeep,
    }
}

/// The forwarding machine's side of pairing.
///
/// The mirror of [`PairingHost`], and separate rather than one trait with both ends
/// in it: this end is given the code rather than making one, connects rather than
/// waits, and speaks first. A trait covering both would give each end operations it
/// must never call.
///
/// The two in the middle — sending this machine's sealed key, taking the peer's —
/// are the same operations the other end has. They are stated twice because the
/// traits are separate; if a third role ever needs them they belong in a trait of
/// their own.
pub trait SourcePairingHost: Entropy {
    /// Reach the machine showing the code, and say whether it was reached.
    ///
    /// Finding it is this side's too: where the sink is has nothing to do with the
    /// exchange, and an address is a thing to connect with rather than anything the
    /// code reads.
    fn connect(&mut self) -> bool;

    fn send_offer(&mut self, offer: &[u8; OFFER]) -> bool;

    fn take_answer(&mut self) -> Option<[u8; OFFER]>;

    fn send_sealed_key(&mut self, sealed: &[u8; SEALED_KEY]) -> bool;

    fn take_sealed_key(&mut self) -> Option<[u8; SEALED_KEY]>;

    /// Write this key down as the one machine this end will send input to.
    ///
    /// One key and not a list, which is the asymmetry ADR-0004 describes: a sink
    /// decides which sources may type on it and can have several, and a source has
    /// exactly one machine it is willing to hand its keyboard to.
    fn pin_sink(&mut self, key: &[u8]) -> bool;
}

/// One pairing from this end: send the code's half, pin what comes back.
///
/// This end speaks first, because it is the end that connects — so the offer goes
/// out before an answer is waited for, and this machine's key goes out before the
/// sink's arrives. That ordering is the other end's read in reverse ([`pair`]).
///
/// Sending this machine's key before the sink's has opened is safe in a way the
/// other direction is not: the sink has shown the code to whoever is at the desk,
/// and a machine that answered without knowing it learns only a public key it could
/// have asked for over the session anyway. The sink withholds its own until it has
/// opened this one, which is the half that has something to withhold.
pub fn pair_with(code: Code, mine: &Identity, host: &mut dyn SourcePairingHost) -> Paired {
    if !host.connect() {
        return Paired::NoSink;
    }
    let Some((started, offer)) = offer(code, &mut *host) else {
        return Paired::Interrupted;
    };
    if !host.send_offer(&offer) {
        return Paired::Interrupted;
    }
    let Some(answer) = host.take_answer() else {
        return Paired::Interrupted;
    };
    let Some(secret) = started.finish(&answer) else {
        return Paired::Interrupted;
    };

    let Some(sealed_mine) = secret.seal(mine.public(), Side::Source) else {
        return Paired::Interrupted;
    };
    if !host.send_sealed_key(&sealed_mine) {
        return Paired::Interrupted;
    }
    let Some(sealed) = host.take_sealed_key() else {
        return Paired::Interrupted;
    };
    let Some(sink) = secret.open(&sealed, Side::Sink) else {
        return Paired::WrongCode;
    };

    // Last, for the reason it is last at the other end: a key written down before it
    // was opened under the code is a key the code did not vouch for.
    match host.pin_sink(&sink) {
        true => Paired::Pinned(hex(&sink)),
        false => Paired::CannotKeep,
    }
}

/// A key as a person pastes it, or nothing if it is not one.
pub fn key_from_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len() / 2)
        .map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect();
    bytes.filter(|bytes| bytes.len() == KEY)
}

pub fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // The write cannot fail: the target is a string with the room already
        // reserved, so there is nothing for the result to report.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(fill: u8) -> Vec<u8> {
        vec![fill; KEY]
    }

    #[test]
    fn a_key_survives_being_read_out_and_typed_back_in() {
        let key = key(0xab);
        assert_eq!(key_from_hex(&hex(&key)), Some(key));
    }

    #[test]
    fn only_something_the_right_length_is_a_key() {
        // A short string of valid hex would be pinned as an identity nothing can
        // present: the list would look configured and refuse every source.
        assert_eq!(key_from_hex("00ff"), None);
        assert_eq!(key_from_hex("zz"), None);
        assert_eq!(key_from_hex("abc"), None);
    }

    #[test]
    fn an_empty_list_authorises_nobody() {
        let authorized = Authorized::parse("");
        assert!(authorized.is_empty());
        assert!(!authorized.holds(&key(1)));
    }

    #[test]
    fn comments_and_blank_lines_are_not_keys() {
        let text = format!("# a note\n\n{}\n", hex(&key(7)));
        let authorized = Authorized::parse(&text);

        assert_eq!(authorized.len(), 1);
        assert!(authorized.holds(&key(7)));
        assert!(!authorized.holds(&key(8)));
    }

    #[test]
    fn a_line_that_is_not_a_key_leaves_the_list_short_rather_than_empty() {
        let text = format!("not a key\n{}\n", hex(&key(3)));
        let authorized = Authorized::parse(&text);

        assert_eq!(authorized.len(), 1);
        assert!(authorized.holds(&key(3)));
    }

    #[test]
    fn adding_a_key_keeps_what_was_there() {
        let before = "# the Windows machine\n";
        let after = Authorized::added(before, &key(2));

        assert!(after.starts_with(before));
        assert_eq!(Authorized::parse(&after).len(), 1);
        assert!(Authorized::parse(&after).holds(&key(2)));
    }

    #[test]
    fn adding_to_a_file_with_no_final_newline_does_not_join_two_keys() {
        // A file a person edited by hand may end mid-line, and a key glued to the
        // end of another is neither of them.
        let before = hex(&key(4));
        let after = Authorized::added(&before, &key(5));
        let authorized = Authorized::parse(&after);

        assert_eq!(authorized.len(), 2);
        assert!(authorized.holds(&key(4)));
        assert!(authorized.holds(&key(5)));
    }

    #[test]
    fn an_identity_is_both_halves_or_nothing() {
        let identity = Identity::new(key(1), key(2)).expect("two keys of the right length");
        assert_eq!(Identity::from_bytes(&identity.to_bytes()), Some(identity));

        assert_eq!(Identity::from_bytes(&key(1)), None);
        assert_eq!(Identity::from_bytes(&[]), None);
        assert!(Identity::new(key(1), vec![2, 2]).is_none());
    }

    #[test]
    fn a_fingerprint_is_the_public_half() {
        let identity = Identity::new(key(1), key(0xfe)).expect("two keys");
        assert_eq!(identity.fingerprint(), hex(&key(0xfe)));
        assert_ne!(identity.fingerprint(), hex(&key(1)));
    }
}
