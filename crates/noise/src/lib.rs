//! The Noise session the link's records travel in, as `snow` performs it
//! (ADR-0012), and the identity it is keyed on.
//!
//! Here rather than in each host for the reason the pairing exchange is: what the
//! two machines have to agree on is the *construction* and not only the constants,
//! and one copy per platform fails as a record that will not open — which is
//! indistinguishable from the wrong code. `snow` is neither IO nor an OS call, so
//! nothing about it needs a machine to drive it and both halves are answerable in a
//! test (ADR-0006).
//!
//! Both halves are here, and each end uses the one it is: the sink responds and the
//! source initiates. What owns the socket in between is the host's, which is why
//! every step is a call that takes bytes and answers bytes rather than one that
//! reads and writes.
//!
//! `host-sim` depends on this crate directly, the same reason it depends on
//! `favjit-pairing-exchange`: standing in for the machine on the other end of a
//! handshake means running the construction for real (ADR-0005).

pub use favjit_host::Entropy;
use snow::resolvers::CryptoResolver;

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
    /// What it is for is telling one machine from another in a log line and in
    /// what `--identity` prints — nobody transcribes it, because the code carries
    /// the key across (`engine::pairing::pair`). The whole key rather than a
    /// digest for that reason: with no comparison to shorten, a digest is a step
    /// that buys nothing and a second thing a reader has to know how to compute.
    pub fn fingerprint(&self) -> String {
        hex(&self.public)
    }
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // The write cannot fail: the target is a string with the room already
        // reserved, so there is nothing for the result to report.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// One message, as bytes on the wire.
///
/// Every message is exactly this long, and the length is not carried. A reader
/// takes a fixed number of bytes and decodes them, so a stream cannot desync into
/// reading a payload as a length — which is the failure a length prefix invites
/// and the one that is hardest to see afterwards, since the bytes still decode
/// into *something*.
/// Wide enough to carry one of the forwarding machine's own trace records with a
/// kind byte in front of it, which is the longest thing a frame has to hold: the
/// records that machine makes reach the converting one over this link, so that
/// one recording holds both machines' (ADR-0009). Input is far shorter, and every
/// frame is this long anyway because the length is not carried.
///
/// Not the exact 33 that leaves: a message wants somewhere to grow to that does
/// not move every other number, and eight spare bytes cost one part in five of a
/// link that carries a keystroke every few milliseconds at most.
pub const FRAME: usize = 40;

/// The Noise handshake and cipher suite both ends name.
///
/// Written out in full rather than assembled from parts: the two machines have to
/// say the same string, and a mismatch is a handshake that fails with nothing to
/// point at.
pub const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// One frame on the wire, sealed.
///
/// The plaintext is a fixed [`FRAME`] and the cipher suite adds a 16-byte tag, so
/// every record is this long and none carries a length. Reading exactly this many
/// bytes is the whole of the framing.
pub const SEALED: usize = FRAME + 16;

/// The source's first handshake message, and the sink's answer to it.
///
/// Fixed for the same reason the records are: each is the same every time, so the
/// end reading one takes exactly this many bytes rather than whatever a single read
/// returned — which over a socket is not the message but a piece of it.
pub const HANDSHAKE: usize = 96;
pub const ANSWER: usize = 48;

/// A message whose length is not the one both ends read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrongLength {
    pub what: &'static str,
    pub wrote: usize,
    pub expected: usize,
}

impl core::fmt::Display for WrongLength {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} is {} bytes, not the {} both ends read",
            self.what, self.wrote, self.expected
        )
    }
}

/// Refuse a message that is not the length stated here.
///
/// Checked rather than trusted, because the reader takes the constant's worth of
/// bytes and nothing tells it otherwise: a message written at some other length is
/// a read that waits for bytes nobody will send, which is a link that hangs rather
/// than one that fails.
fn exactly(what: &'static str, wrote: usize, expected: usize) -> Result<(), WrongLength> {
    if wrote == expected {
        return Ok(());
    }
    Err(WrongLength {
        what,
        wrote,
        expected,
    })
}

/// A keypair from this machine's own entropy.
///
/// `Dh::set` and `Dh::pubkey` rather than `Builder::generate_keypair`, because
/// generating draws from whichever RNG the resolver resolves to — this machine's
/// own, reached through nothing an entropy source stands behind — and a keypair
/// this crate cannot attribute to the bytes it was handed is a keypair a scripted
/// test cannot reproduce (ADR-0006, ADR-0007).
///
/// `None` for either way it can fail, since neither leaves anything to present:
/// what to do about a machine that cannot make one is the sequence's.
pub fn keypair<E: Entropy + ?Sized>(entropy: &mut E) -> Option<Identity> {
    let mut private = [0u8; KEY];
    if !entropy.fill(&mut private) {
        return None;
    }
    let mut dh = snow::resolvers::DefaultResolver
        .resolve_dh(&snow::params::DHChoice::Curve25519)
        .expect("the curve this crate's pattern names");
    dh.set(&private);
    Identity::new(private.to_vec(), dh.pubkey().to_vec())
}

/// The sink's half of the handshake.
///
/// Two calls with the socket in between, rather than one that reads and writes:
/// which end owns the socket is the host's business, and this way the steps do not
/// care how it reads.
pub struct Responder(snow::HandshakeState);

impl Responder {
    /// `entropy` is this handshake's ephemeral key, and nothing past it: the
    /// static key is `identity`'s, so a call that produced anything else would be
    /// a second place deciding what a responder is (ADR-0006).
    pub fn new(identity: &Identity, entropy: &mut dyn Entropy) -> Result<Self, Failed> {
        let mut ephemeral = [0u8; KEY];
        if !entropy.fill(&mut ephemeral) {
            return Err(Failed::entropy());
        }
        snow::Builder::new(pattern())
            .local_private_key(identity.private())
            .map(|builder| builder.fixed_ephemeral_key_for_testing_only(&ephemeral))
            .and_then(snow::Builder::build_responder)
            .map(Self)
            .map_err(Failed::noise)
    }

    /// Take the source's first message, and give back what to send it.
    pub fn answer(&mut self, first: &[u8; HANDSHAKE]) -> Result<[u8; ANSWER], Failed> {
        let mut opened = [0u8; HANDSHAKE];
        self.0
            .read_message(first, &mut opened)
            .map_err(Failed::noise)?;
        let mut answer = [0u8; ANSWER];
        let wrote = self
            .0
            .write_message(&[], &mut answer)
            .map_err(Failed::noise)?;
        exactly("the answer", wrote, ANSWER).map_err(Failed::length)?;
        Ok(answer)
    }

    /// The session, and the key of whoever made it.
    ///
    /// The key comes from the handshake rather than from anything the peer says in
    /// the session: in this pattern it is proof of who is calling, and a key read
    /// any later would be a claim.
    pub fn done(self) -> Result<(Vec<u8>, Session), Failed> {
        let peer = self
            .0
            .get_remote_static()
            .ok_or_else(|| Failed("the handshake left no peer key".into()))?
            .to_vec();
        Ok((
            peer,
            Session(self.0.into_transport_mode().map_err(Failed::noise)?),
        ))
    }
}

/// The source's half.
pub struct Initiator(snow::HandshakeState);

impl Initiator {
    /// Pinned to the sink's key, which is what makes this pattern refuse a machine
    /// standing in for it (ADR-0004).
    pub fn new(
        identity: &Identity,
        sink: &[u8],
        entropy: &mut dyn Entropy,
    ) -> Result<Self, Failed> {
        let mut ephemeral = [0u8; KEY];
        if !entropy.fill(&mut ephemeral) {
            return Err(Failed::entropy());
        }
        snow::Builder::new(pattern())
            .local_private_key(identity.private())
            .and_then(|builder| builder.remote_public_key(sink))
            .map(|builder| builder.fixed_ephemeral_key_for_testing_only(&ephemeral))
            .and_then(snow::Builder::build_initiator)
            .map(Self)
            .map_err(Failed::noise)
    }

    pub fn first(&mut self) -> Result<[u8; HANDSHAKE], Failed> {
        let mut first = [0u8; HANDSHAKE];
        let wrote = self
            .0
            .write_message(&[], &mut first)
            .map_err(Failed::noise)?;
        exactly("the first message", wrote, HANDSHAKE).map_err(Failed::length)?;
        Ok(first)
    }

    pub fn take_answer(&mut self, answer: &[u8; ANSWER]) -> Result<(), Failed> {
        let mut opened = [0u8; ANSWER];
        self.0
            .read_message(answer, &mut opened)
            .map_err(Failed::noise)?;
        Ok(())
    }

    pub fn done(self) -> Result<Session, Failed> {
        Ok(Session(
            self.0.into_transport_mode().map_err(Failed::noise)?,
        ))
    }
}

/// An open session, and the only two things either end does with one.
pub struct Session(snow::TransportState);

impl Session {
    /// The frame inside a record, and the number the session had it at.
    ///
    /// Nothing rather than an error saying which way it failed: a record that will
    /// not open and one that opens to something other than a frame are the same
    /// answer to the end reading it, and what to do about it is the link's.
    ///
    /// The number comes back with the frame rather than from a call of its own, so
    /// a caller cannot read one record and the count of another: it is taken
    /// before the record is opened, since what the session counts afterwards is
    /// the next one.
    pub fn open(&mut self, sealed: &[u8; SEALED]) -> Option<(u64, [u8; FRAME])> {
        let at = self.0.receiving_nonce();
        let mut frame = [0u8; FRAME];
        match self.0.read_message(sealed, &mut frame) {
            Ok(FRAME) => Some((at, frame)),
            _ => None,
        }
    }

    /// The sealed record, and the number this session sent it under.
    pub fn seal(&mut self, frame: &[u8; FRAME]) -> Result<(u64, [u8; SEALED]), Failed> {
        let at = self.0.sending_nonce();
        let mut sealed = [0u8; SEALED];
        let wrote = self
            .0
            .write_message(frame, &mut sealed)
            .map_err(Failed::noise)?;
        exactly("a sealed record", wrote, SEALED).map_err(Failed::length)?;
        Ok((at, sealed))
    }
}

/// Why a handshake or a record did not come out.
///
/// One type with a sentence in it, rather than the cases apart: every one of them
/// ends the same way — this peer does not get in — and what a host does with it is
/// put it in a log line.
#[derive(Debug)]
pub struct Failed(String);

impl Failed {
    fn noise(error: snow::Error) -> Self {
        Self(format!("{error}"))
    }

    fn length(wrong: WrongLength) -> Self {
        Self(format!("{wrong}"))
    }

    fn entropy() -> Self {
        Self("no entropy for the handshake".into())
    }
}

impl core::fmt::Display for Failed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn pattern() -> snow::params::NoiseParams {
    PATTERN.parse().expect("a pattern this crate has")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A different byte each call, so two ephemeral keys in the same test are
    /// never the one the other side just drew.
    struct Scripted(u8);

    impl Entropy for Scripted {
        fn fill(&mut self, into: &mut [u8]) -> bool {
            into.fill(self.0);
            self.0 = self.0.wrapping_add(1);
            true
        }
    }

    #[test]
    fn the_two_ends_shake_hands_and_a_frame_survives_the_trip() {
        // The one place the lengths this crate states are checked against the
        // implementation that writes them, so that a length stated here is a
        // length that arrives. Each is asserted rather than left to `exactly`,
        // since a message read at the wrong length is a read that waits for
        // ever, not a test that fails.
        let sink = keypair(&mut Scripted(10)).expect("the sink's identity");
        let source = keypair(&mut Scripted(11)).expect("the source's identity");

        let mut initiator =
            Initiator::new(&source, sink.public(), &mut Scripted(1)).expect("an initiator");
        let mut responder = Responder::new(&sink, &mut Scripted(2)).expect("a responder");

        let first = initiator.first().expect("the first message");
        let answer = responder.answer(&first).expect("the answer");
        initiator.take_answer(&answer).expect("the answer opens");

        let (peer, mut sinks) = responder.done().expect("the sink's session");
        let mut sources = initiator.done().expect("the source's session");
        assert_eq!(peer, source.public(), "the key read is the source's own");

        let frame = [7u8; FRAME];
        // The first record of a session, so both ends have it at zero: what the
        // two machines' recordings are merged on is this number, and a sender and
        // a receiver that counted differently would line up nothing (ADR-0009).
        let (sent_at, sealed) = sources.seal(&frame).expect("a sealed record");
        assert_eq!(sent_at, 0);
        assert_eq!(sinks.open(&sealed), Some((0, frame)));

        // And it advances in step, so the second record is not the first's number.
        let (next_at, sealed) = sources.seal(&frame).expect("a second sealed record");
        assert_eq!(next_at, 1);
        assert_eq!(sinks.open(&sealed), Some((1, frame)));
    }

    #[test]
    fn a_source_the_sink_has_not_authorised_still_gets_a_session() {
        // Worth saying because it is what the sink's sequence rests on: the
        // handshake is where the key is learnt, not where it is judged, so the
        // refusal is a later step's alone.
        let sink = keypair(&mut Scripted(10)).expect("the sink's identity");
        let stranger = keypair(&mut Scripted(13)).expect("a stranger");

        let mut initiator =
            Initiator::new(&stranger, sink.public(), &mut Scripted(1)).expect("an initiator");
        let mut responder = Responder::new(&sink, &mut Scripted(2)).expect("a responder");
        let first = initiator.first().expect("the first message");
        let answer = responder.answer(&first).expect("the answer");
        initiator.take_answer(&answer).expect("the answer opens");

        let (peer, _) = responder.done().expect("a session");
        assert_eq!(peer, stranger.public());
    }

    #[test]
    fn a_source_pinned_to_another_key_does_not_get_in() {
        // The pinning ADR-0004 asks of the source, from the sink's end: a machine
        // standing in for the sink cannot open the first message, so there is
        // nothing for it to answer.
        let sink = keypair(&mut Scripted(10)).expect("the sink's identity");
        let someone_else = keypair(&mut Scripted(14)).expect("another machine");
        let source = keypair(&mut Scripted(15)).expect("the source's identity");

        let mut initiator =
            Initiator::new(&source, someone_else.public(), &mut Scripted(1)).expect("an initiator");
        let first = initiator.first().expect("the first message");
        assert!(Responder::new(&sink, &mut Scripted(2))
            .expect("a responder")
            .answer(&first)
            .is_err());
    }
}
