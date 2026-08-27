//! The socket pairing runs over, on this end.
//!
//! Calls and nothing else: connect, one message out, one message in, bytes nothing
//! can predict, and a key written down. Which order they go in, what a message that
//! will not open means, and what is written down afterwards are `engine`'s, where
//! the suite drives them (ADR-0006). So is the exchange itself: an agreement
//! between two machines written once per platform fails as a key that will not
//! open (ADR-0012).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use favjit_host::pairing::SourcePairingHost;
use favjit_host::{Entropy, Trouble};

/// This end of pairing, for as long as one attempt takes.
///
/// The socket is not here: it belongs to [`Offer`], which only exists once one
/// is connected — so nothing asked of it has to ask whether there is one
/// (ADR-0006).
pub struct Pairing {
    /// Where the sink is, found before this was made.
    address: SocketAddr,
}

impl Pairing {
    pub fn new(address: SocketAddr) -> Self {
        Self { address }
    }
}

impl Entropy for Pairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        // The machine's own source, which is where the static keys come from too. A
        // generator seeded in this process would be one more thing to be wrong about,
        // and what the code protects it protects only while it is unpredictable.
        getrandom::getrandom(into).is_ok()
    }
}

impl SourcePairingHost for Pairing {
    fn connect(
        &mut self,
        timeout: core::time::Duration,
    ) -> Option<Box<dyn favjit_host::pairing::Offering>> {
        reached(TcpStream::connect_timeout(&self.address, timeout))
    }

    /// The path is in what comes back, because a person told the file cannot
    /// be written has to be told which file (ADR-0006).
    fn make_sink_directory(&mut self) -> Result<(), Trouble> {
        let path = crate::link::sink_path();
        crate::link::made(
            std::fs::create_dir_all(crate::link::holding(&path)),
            crate::link::holding(&path),
        )
    }

    fn pin_sink(&mut self, text: &str) -> Result<(), Trouble> {
        let path = crate::link::sink_path();
        crate::link::made(std::fs::write(&path, text), &path)
    }
}

/// The connection a connect opened, and nothing where it opened none.
fn reached(
    answered: std::io::Result<TcpStream>,
) -> Option<Box<dyn favjit_host::pairing::Offering>> {
    answered
        .ok()
        .map(|stream| Box::new(Offer(stream)) as Box<dyn favjit_host::pairing::Offering>)
}

/// The one connection this attempt runs over.
struct Offer(TcpStream);

impl Entropy for Offer {
    /// The machine's own source, which is where the static keys come from too. A
    /// generator seeded in this process would be one more thing to be wrong about,
    /// and what the code protects it protects only while it is unpredictable.
    fn fill(&mut self, into: &mut [u8]) -> bool {
        getrandom::getrandom(into).is_ok()
    }
}

impl favjit_host::pairing::Offering for Offer {
    fn set_read_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.0.set_read_timeout(Some(timeout)))
    }

    fn set_write_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.0.set_write_timeout(Some(timeout)))
    }

    fn send_offer(&mut self, offer: &[u8]) -> bool {
        went(self.0.write_all(offer))
    }

    fn flush_offer(&mut self) -> bool {
        went(self.0.flush())
    }

    fn take_answer(&mut self, into: &mut [u8]) -> bool {
        went(self.0.read_exact(into))
    }

    fn send_sealed_key(&mut self, sealed: &[u8]) -> bool {
        went(self.0.write_all(sealed))
    }

    fn flush_sealed_key(&mut self) -> bool {
        went(self.0.flush())
    }

    fn take_sealed_key(&mut self, into: &mut [u8]) -> bool {
        went(self.0.read_exact(into))
    }
}

/// Whether a call did what it was asked, as its own answer says.
fn went(answered: std::io::Result<()>) -> bool {
    answered.is_ok()
}
