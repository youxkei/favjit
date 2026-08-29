//! The socket pairing runs over, on this end.
//!
//! Calls and nothing else: connect, one message out, one message in, bytes nothing
//! can predict, and a key written down. Which order they go in, what a message that
//! will not open means, and what is written down afterwards are
//! [`favjit_core::pairing::pair_with`]'s, where the suite drives them (ADR-0006). So
//! is the exchange itself: an agreement between two machines written once per
//! platform fails as a key that will not open (ADR-0017).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use favjit_core::pairing::{Entropy, SourcePairingHost, OFFER, SEALED_KEY};
use log::{error, info};

/// How long each step of the exchange may take.
///
/// One person is at the desk reading a code off a screen, so the whole thing is
/// seconds. A step that cannot complete has to fail rather than wait: pairing is not
/// the run that holds keyboards, but a command that never returns is one nobody can
/// tell from a hung machine.
const STEP: Duration = Duration::from_secs(10);

/// This end of pairing, for as long as one attempt takes.
pub struct Pairing {
    /// Where the sink is, found before this was made.
    address: SocketAddr,
    stream: Option<TcpStream>,
}

impl Pairing {
    pub fn new(address: SocketAddr) -> Self {
        Self {
            address,
            stream: None,
        }
    }
}

impl Entropy for Pairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        // The machine's own source, which is where the static keys come from too. A
        // generator seeded in this process would be one more thing to be wrong about,
        // and what the code protects it protects only while it is unpredictable.
        getrandom::getrandom(into)
            .inspect_err(|error| error!("cannot read randomness: {error}"))
            .is_ok()
    }
}

impl SourcePairingHost for Pairing {
    fn connect(&mut self) -> bool {
        match TcpStream::connect_timeout(&self.address, STEP) {
            Ok(stream) => {
                let _ = stream.set_read_timeout(Some(STEP));
                let _ = stream.set_write_timeout(Some(STEP));
                info!("pairing with the sink at {}", self.address);
                self.stream = Some(stream);
                true
            }
            Err(error) => {
                error!("cannot reach the sink at {}: {error}", self.address);
                false
            }
        }
    }

    fn send_offer(&mut self, offer: &[u8; OFFER]) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        stream
            .write_all(offer)
            .and_then(|()| stream.flush())
            .is_ok()
    }

    fn take_answer(&mut self) -> Option<[u8; OFFER]> {
        let stream = self.stream.as_mut()?;
        let mut answer = [0u8; OFFER];
        stream.read_exact(&mut answer).ok()?;
        Some(answer)
    }

    fn send_sealed_key(&mut self, sealed: &[u8; SEALED_KEY]) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        stream
            .write_all(sealed)
            .and_then(|()| stream.flush())
            .is_ok()
    }

    fn take_sealed_key(&mut self) -> Option<[u8; SEALED_KEY]> {
        let stream = self.stream.as_mut()?;
        let mut sealed = [0u8; SEALED_KEY];
        stream.read_exact(&mut sealed).ok()?;
        Some(sealed)
    }

    fn pin_sink(&mut self, key: &[u8]) -> bool {
        let path = crate::link::sink_path();
        match crate::link::pin_sink(&path, key) {
            Ok(()) => true,
            Err(error) => {
                error!("cannot write {}: {error}", path.display());
                false
            }
        }
    }
}
