//! The socket pairing runs over, on this end.
//!
//! Calls and nothing else: show six digits, take one connection, one message in, one
//! out, bytes nothing can predict, and a line added to a file. The exchange itself —
//! which order they go in, that the code is spent whether the attempt worked or not,
//! what a message that will not open means — is [`favjit_engine::pairing::pair`]'s,
//! where the suite drives it (ADR-0006). So is the arithmetic: what both machines
//! have to agree on cannot be written once per platform (ADR-0012).
//!
//! **The port is advertised while the code is up**, under a name of pairing's own:
//! the machine offering itself finds this one the way input finds it later, and a
//! converting machine has both up at once (ADR-0012). Withdrawn with the attempt, so
//! a name left standing cannot send the next source to a port nothing is on.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

use favjit_host::pairing::PairingHost;
use favjit_host::{Entropy, Trouble};

use crate::link::Advertisement;

/// This end of pairing, for as long as one attempt takes.
///
/// Neither socket is here: the listener belongs to [`Listener`] and the
/// connection to [`Exchange`], each of which only exists once its own is open —
/// so nothing asked of a socket has to ask whether there is one (ADR-0006).
#[derive(Default)]
pub struct Pairing {
    advertised: Option<Advertisement>,
}

impl Pairing {
    /// One pairing attempt, before its listener has been opened.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Entropy for Pairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        crate::urandom(into)
    }
}

impl PairingHost for Pairing {
    fn bind_listener(&mut self) -> Option<Box<dyn favjit_host::pairing::Listening>> {
        bound(TcpListener::bind(("0.0.0.0", 0)))
    }

    fn advertise(&mut self, service: &str, port: u16) -> bool {
        self.advertised = crate::link::advertise(service, port).ok();
        self.advertised.is_some()
    }

    fn show(&mut self, line: &str) {
        // On stdout, because the point of it is to be read off this screen and typed
        // at the other one — the same reason `--identity` prints rather than logs.
        println!("{line}");
    }

    fn authorized(&mut self) -> Option<String> {
        std::fs::read_to_string(crate::link::authorized_path()).ok()
    }

    /// The path is in what comes back, because a person told the list cannot
    /// be written has to be told which file (ADR-0006).
    fn make_authorized_directory(&mut self) -> Result<(), Trouble> {
        let path = crate::link::authorized_path();
        crate::link::made(
            std::fs::create_dir_all(crate::link::holding(&path)),
            crate::link::holding(&path),
        )
    }

    fn authorize(&mut self, text: &str) -> Result<(), Trouble> {
        let path = crate::link::authorized_path();
        crate::link::made(std::fs::write(&path, text), &path)
    }
}

impl Drop for Pairing {
    /// Withdraw the advertisement with the attempt.
    ///
    /// One left standing would send the next source to a port nothing is on, which
    /// from that end looks like a machine refusing to pair.
    fn drop(&mut self) {
        self.advertised.take();
    }
}

/// The socket a source is taken on.
struct Listener(TcpListener);

/// The listener a bind opened, and nothing where it opened none.
fn bound(
    answered: std::io::Result<TcpListener>,
) -> Option<Box<dyn favjit_host::pairing::Listening>> {
    answered
        .ok()
        .map(|listener| Box::new(Listener(listener)) as Box<dyn favjit_host::pairing::Listening>)
}

impl favjit_host::pairing::Listening for Listener {
    fn port(&mut self) -> Option<u16> {
        the_port(self.0.local_addr())
    }

    fn set_blocking(&mut self) -> bool {
        went(self.0.set_nonblocking(false))
    }

    fn accept(&mut self) -> Option<Box<dyn favjit_host::pairing::Exchanging>> {
        arrived(self.0.accept())
    }
}

/// The port an address carries, and nothing where the machine named no address.
fn the_port(answered: std::io::Result<SocketAddr>) -> Option<u16> {
    // Spelled with the type it belongs to, because a bare `.port()` beside
    // [`Listening::port`] reads as that one — and that one reaches the machine.
    answered.ok().map(|address| SocketAddr::port(&address))
}

/// Whether a call did what it was asked, as its own answer says.
fn went(answered: std::io::Result<()>) -> bool {
    answered.is_ok()
}

/// The connection something arrived on, and nothing where nothing did.
///
/// The address it came from is not carried: which machine may pair is settled by
/// the code, and an address would be a second answer to a question the code has
/// already answered (ADR-0012).
fn arrived(
    answered: std::io::Result<(TcpStream, SocketAddr)>,
) -> Option<Box<dyn favjit_host::pairing::Exchanging>> {
    answered
        .ok()
        .map(|(stream, _)| Box::new(Exchange(stream)) as Box<dyn favjit_host::pairing::Exchanging>)
}

/// The one connection this attempt runs over.
struct Exchange(TcpStream);

/// The same source every other entropy user in this crate reads, for the reason
/// the identity file's is (ADR-0012).
impl Entropy for Exchange {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        crate::urandom(into)
    }
}

impl favjit_host::pairing::Exchanging for Exchange {
    fn set_read_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.0.set_read_timeout(Some(timeout)))
    }

    fn set_write_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.0.set_write_timeout(Some(timeout)))
    }

    fn take_offer(&mut self, into: &mut [u8]) -> bool {
        went(self.0.read_exact(into))
    }

    fn send_answer(&mut self, answer: &[u8]) -> bool {
        went(self.0.write_all(answer))
    }

    fn flush_answer(&mut self) -> bool {
        went(self.0.flush())
    }

    fn take_sealed_key(&mut self, into: &mut [u8]) -> bool {
        went(self.0.read_exact(into))
    }

    fn send_sealed_key(&mut self, sealed: &[u8]) -> bool {
        went(self.0.write_all(sealed))
    }

    fn flush_sealed_key(&mut self) -> bool {
        went(self.0.flush())
    }
}
