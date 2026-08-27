//! The socket pairing runs over, on this end.
//!
//! Calls and nothing else: show six digits, take one connection, one message in, one
//! out, bytes nothing can predict, and a line added to a file. The exchange itself —
//! which order they go in, that the code is spent whether the attempt worked or not,
//! what a message that will not open means — is [`favjit_core::pairing::pair`]'s,
//! where the suite drives it (ADR-0006). So is the arithmetic: what both machines
//! have to agree on cannot be written once per platform (ADR-0017).
//!
//! **The port is advertised while the code is up**, under a name of pairing's own:
//! the machine offering itself finds this one the way input finds it later, and a
//! converting machine has both up at once (ADR-0017). Withdrawn with the attempt, so
//! a name left standing cannot send the next source to a port nothing is on.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use favjit_core::link::PAIRING;
use favjit_core::pairing::{Code, Entropy, PairingHost, OFFER, SEALED_KEY};
use log::{error, info};

use crate::link::Advertisement;

/// How long each step of the exchange may take.
///
/// One person is at the desk reading a code off one screen and typing it at another,
/// so the whole thing is seconds. A step that cannot complete has to fail rather than
/// wait: a command that never returns is one nobody can tell from a hung machine.
const STEP: Duration = Duration::from_secs(60);

/// This end of pairing, for as long as one attempt takes.
pub struct Pairing {
    listener: TcpListener,
    advertised: Option<Advertisement>,
    /// The connection being served, once something has arrived.
    stream: Option<TcpStream>,
}

impl Pairing {
    /// Bind a port for one attempt, and say on the network that this machine is here.
    ///
    /// The port is the machine's to choose, as the link's is: the advertisement
    /// carries it, so a number to agree on would be one more thing already taken on
    /// the day it is.
    pub fn open() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", 0))?;
        let port = listener.local_addr()?.port();
        let advertised = match crate::link::advertise(PAIRING, port) {
            Ok(advertised) => Some(advertised),
            Err(error) => {
                // Worth saying rather than stopping: the other end can be given the
                // address by hand, and a code already on screen is worth offering.
                error!("cannot advertise while pairing ({error}); pass the address there instead");
                None
            }
        };
        info!("pairing is listening on port {port}");
        Ok(Self {
            listener,
            advertised,
            stream: None,
        })
    }
}

impl Entropy for Pairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        // The same place the static keys come from. A generator seeded in this
        // process would be one more thing to be wrong about, and the code protects
        // the exchange for exactly as long as it is unpredictable.
        let Ok(mut random) = std::fs::File::open("/dev/urandom") else {
            error!("cannot open /dev/urandom");
            return false;
        };
        random
            .read_exact(into)
            .inspect_err(|error| error!("cannot read randomness: {error}"))
            .is_ok()
    }
}

impl PairingHost for Pairing {
    fn show(&mut self, code: Code) {
        // On stdout, because the point of it is to be read off this screen and typed
        // at the other one — the same reason `--identity` prints rather than logs.
        println!(
            "pairing code: {}",
            core::str::from_utf8(&code).unwrap_or("??????")
        );
        println!("type it on the other machine within a minute: favjit --pair <those digits>");
    }

    fn wait_for_a_source(&mut self) -> bool {
        if let Err(error) = self.listener.set_nonblocking(false) {
            error!("cannot wait for a connection: {error}");
            return false;
        }
        match self.listener.accept() {
            Ok((stream, from)) => {
                let _ = stream.set_read_timeout(Some(STEP));
                let _ = stream.set_write_timeout(Some(STEP));
                info!("something connected from {from}");
                self.stream = Some(stream);
                true
            }
            Err(error) => {
                error!("nothing could be accepted: {error}");
                false
            }
        }
    }

    fn take_offer(&mut self) -> Option<[u8; OFFER]> {
        let stream = self.stream.as_mut()?;
        let mut offer = [0u8; OFFER];
        stream
            .read_exact(&mut offer)
            .inspect_err(|error| info!("nothing arrived from the connection: {error}"))
            .ok()?;
        Some(offer)
    }

    fn send_answer(&mut self, answer: &[u8; OFFER]) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        stream
            .write_all(answer)
            .and_then(|()| stream.flush())
            .is_ok()
    }

    fn take_sealed_key(&mut self) -> Option<[u8; SEALED_KEY]> {
        let stream = self.stream.as_mut()?;
        let mut sealed = [0u8; SEALED_KEY];
        stream.read_exact(&mut sealed).ok()?;
        Some(sealed)
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

    fn authorize(&mut self, key: &[u8]) -> bool {
        let path = crate::link::authorized_path();
        match crate::link::authorize(key) {
            Ok(()) => true,
            Err(error) => {
                error!("cannot write {}: {error}", path.display());
                false
            }
        }
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
