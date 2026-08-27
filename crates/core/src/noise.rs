//! The Noise session the link's records travel in, as `snow` performs it
//! (ADR-0017).
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

use crate::link::{exactly, ANSWER, FRAME, HANDSHAKE, PATTERN, SEALED};
use crate::pairing::Identity;

/// A keypair from this machine's own entropy.
///
/// `None` for either way it can fail, since neither leaves anything to present:
/// what to do about a machine that cannot make one is the sequence's.
pub fn keypair() -> Option<Identity> {
    let keypair = snow::Builder::new(pattern()).generate_keypair().ok()?;
    Identity::new(keypair.private, keypair.public)
}

/// The sink's half of the handshake.
///
/// Two calls with the socket in between, rather than one that reads and writes:
/// which end owns the socket is the host's business, and this way the steps do not
/// care how it reads.
pub struct Responder(snow::HandshakeState);

impl Responder {
    pub fn new(identity: &Identity) -> Result<Self, Failed> {
        snow::Builder::new(pattern())
            .local_private_key(identity.private())
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
    pub fn new(identity: &Identity, sink: &[u8]) -> Result<Self, Failed> {
        snow::Builder::new(pattern())
            .local_private_key(identity.private())
            .and_then(|builder| builder.remote_public_key(sink))
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
    /// The frame inside a record, or nothing.
    ///
    /// Nothing rather than an error saying which way it failed: a record that will
    /// not open and one that opens to something other than a frame are the same
    /// answer to the end reading it, and what to do about it is the link's.
    pub fn open(&mut self, sealed: &[u8; SEALED]) -> Option<[u8; FRAME]> {
        let mut frame = [0u8; FRAME];
        match self.0.read_message(sealed, &mut frame) {
            Ok(FRAME) => Some(frame),
            _ => None,
        }
    }

    pub fn seal(&mut self, frame: &[u8; FRAME]) -> Result<[u8; SEALED], Failed> {
        let mut sealed = [0u8; SEALED];
        let wrote = self
            .0
            .write_message(frame, &mut sealed)
            .map_err(Failed::noise)?;
        exactly("a sealed record", wrote, SEALED).map_err(Failed::length)?;
        Ok(sealed)
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

    fn length(wrong: crate::link::WrongLength) -> Self {
        Self(format!("{wrong}"))
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
    use crate::link::Message;
    use crate::{DeviceId, Key};

    #[test]
    fn the_two_ends_shake_hands_and_a_frame_survives_the_trip() {
        // The one place the lengths the link states are checked against the
        // implementation that writes them, so that a length stated there is a length
        // that arrives. Each is asserted rather than left to `exactly`, since a
        // message read at the wrong length is a read that waits for ever, not a test
        // that fails.
        let sink = keypair().expect("the sink's identity");
        let source = keypair().expect("the source's identity");

        let mut initiator = Initiator::new(&source, sink.public()).expect("an initiator");
        let mut responder = Responder::new(&sink).expect("a responder");

        let first = initiator.first().expect("the first message");
        let answer = responder.answer(&first).expect("the answer");
        initiator.take_answer(&answer).expect("the answer opens");

        let (peer, mut sinks) = responder.done().expect("the sink's session");
        let mut sources = initiator.done().expect("the source's session");
        assert_eq!(peer, source.public(), "the key read is the source's own");

        let mut frame = [0u8; FRAME];
        Message::KeyDown {
            device: DeviceId(3),
            key: Key::K,
        }
        .encode(&mut frame);
        let sealed = sources.seal(&frame).expect("a sealed record");
        assert_eq!(sinks.open(&sealed), Some(frame));
    }

    #[test]
    fn a_source_the_sink_has_not_authorised_still_gets_a_session() {
        // Worth saying because it is what the sink's sequence rests on: the
        // handshake is where the key is learnt, not where it is judged, so the
        // refusal in `link::serve` is the only refusal there is.
        let sink = keypair().expect("the sink's identity");
        let stranger = keypair().expect("a stranger");

        let mut initiator = Initiator::new(&stranger, sink.public()).expect("an initiator");
        let mut responder = Responder::new(&sink).expect("a responder");
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
        let sink = keypair().expect("the sink's identity");
        let someone_else = keypair().expect("another machine");
        let source = keypair().expect("the source's identity");

        let mut initiator = Initiator::new(&source, someone_else.public()).expect("an initiator");
        let first = initiator.first().expect("the first message");
        assert!(Responder::new(&sink)
            .expect("a responder")
            .answer(&first)
            .is_err());
    }
}
