//! The socket and the advertisement behind the sink's link (ADR-0017).
//!
//! Calls and nothing else, into a listening socket, Bonjour, two files and this
//! machine's cryptography. Who is allowed in, when the list is read, what order the
//! handshake happens in, what becomes of a record that will not open, and what the
//! two machines agree on down to the lengths are all [`favjit_core::link`]'s and
//! [`favjit_core::pairing`]'s, where the suite can drive them (ADR-0006).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use favjit_core::link::{
    Accepted, Incoming, LinkHost, ANSWER, FRAME, HANDSHAKE, IDLE, SEALED, SERVICE,
};
use favjit_core::pairing::{Authorized, Identity, IdentityStore};
use favjit_core::{EventKind, HostEvent};
use log::{error, info, warn};

use crate::capture::{Captured, Clock};
use crate::ffi::{
    DNSServiceRefDeallocate, DNSServiceRegister, DnsServiceRef, DNS_SERVICE_INTERFACE_ANY,
};
use favjit_core::noise::{self, Responder, Session};

/// Where the two files live.
///
/// Under `/Library` rather than a person's home: the converter reads them as root,
/// and a list a logged-in user could rewrite would be a way to authorise a source
/// without the explicit action ADR-0004 rests on.
pub fn identity_path() -> PathBuf {
    PathBuf::from("/Library/Application Support/favjit/identity")
}

pub fn authorized_path() -> PathBuf {
    PathBuf::from("/Library/Application Support/favjit/authorized")
}

/// Who this machine will take input from, as the list stands now.
///
/// A wrapper rather than a path handed out to be read: a caller that named the file
/// itself could name a different one from the running converter's, and the symptom
/// would be a machine that pairs and still refuses.
pub fn authorized() -> Authorized {
    Authorized::parse(&std::fs::read_to_string(authorized_path()).unwrap_or_default())
}

/// Add a key to the list.
///
/// The text goes through `core` and comes back, so what a person put in the file —
/// comments, an unfinished last line — is kept as it was. What counts as a key is
/// `core`'s too: a check here would be a second copy of that rule, and two copies
/// are two answers.
pub fn authorize(key: &[u8]) -> std::io::Result<()> {
    let path = authorized_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, Authorized::added(&before, key))
}

/// The identity file, as the sequence in `core` reads and writes it.
///
/// The path is its own rather than a parameter, so that the file a person reads a
/// key out of with `--identity` is the one the running converter presents.
pub struct IdentityFile(PathBuf);

impl Default for IdentityFile {
    fn default() -> Self {
        Self(identity_path())
    }
}

impl IdentityStore for IdentityFile {
    fn read(&mut self) -> Option<Vec<u8>> {
        std::fs::read(&self.0).ok()
    }

    fn make(&mut self) -> Option<Identity> {
        noise::keypair()
    }

    fn keep(&mut self, bytes: &[u8]) -> bool {
        if let Some(parent) = self.0.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                warn!("cannot make {}: {error}", parent.display());
                return false;
            }
        }
        match write_privately(&self.0, bytes) {
            Ok(()) => true,
            Err(error) => {
                warn!("cannot write {}: {error}", self.0.display());
                false
            }
        }
    }
}

/// Whether a connection this end could not take says anything about the next one.
///
/// Named kinds only, and everything else counted as the socket being unusable: a
/// kind this end cannot name might be one that never clears, and `core` bounds how
/// many in a row it will take before giving up ([`favjit_core::link::FAILURES`]) —
/// so the cost of being wrong here is a link that ends a little early, never one
/// that spins on a socket forever.
fn recoverable(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::{ConnectionAborted, ConnectionReset, Interrupted, WouldBlock};

    // A connection that went away between arriving and being taken, and a call cut
    // short by a signal: both are what a machine on the same desk being switched
    // off looks like, and what a port scan produces.
    matches!(
        error.kind(),
        ConnectionAborted | ConnectionReset | Interrupted | WouldBlock
    )
}

/// Write the file only its owner can read.
///
/// Narrowed as it is created rather than afterwards, because between a plain write
/// and a change of mode the private half of an identity is a file anyone can read.
fn write_privately(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

/// The socket, the session and the advertisement, as the sequence in `core` needs
/// them.
pub struct Link {
    listener: TcpListener,
    identity: Identity,
    authorized_at: PathBuf,
    clock: Clock,
    events: Sender<Captured>,
    /// What has connected but not yet shaken hands.
    connected: Option<TcpStream>,
    /// The handshake in progress. Held rather than passed back, because what it is
    /// made of is this platform's cryptography.
    shaking: Option<Responder>,
    /// The session being served. Held rather than passed back, because what it is
    /// made of — a socket and a cipher state — is exactly what stays on this side.
    open: Option<(TcpStream, Session)>,
    advertised: Option<Advertisement>,
}

/// The mDNS registration, for as long as favjit is listening.
///
/// A type of its own so that it is withdrawn by going out of scope: an
/// advertisement outliving the socket would send the source to a port nothing is
/// on, which looks like a machine that refuses input.
pub struct Advertisement(DnsServiceRef);

impl Drop for Advertisement {
    fn drop(&mut self) {
        unsafe { DNSServiceRefDeallocate(self.0) };
    }
}

// The handle is only ever used from the thread that made it and the one that drops
// it, never at the same time, which is what `Send` needs of it here.
unsafe impl Send for Advertisement {}

impl Link {
    /// Bind, and say which port it got.
    ///
    /// The machine picks the port rather than a person, because the source finds
    /// this machine by name over mDNS and the advertisement carries the number: one
    /// to choose would be one more thing for the two ends to agree on, one more
    /// thing already taken on the day it is, and no easier to find.
    pub fn bind(
        identity: Identity,
        clock: Clock,
        events: Sender<Captured>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", 0))?;
        Ok(Self {
            listener,
            identity,
            authorized_at: authorized_path(),
            clock,
            events,
            connected: None,
            shaking: None,
            open: None,
            advertised: None,
        })
    }

    pub fn port(&self) -> std::io::Result<u16> {
        Ok(self.listener.local_addr()?.port())
    }

    pub fn fingerprint(&self) -> String {
        self.identity.fingerprint()
    }

    fn register(&mut self, port: u16) -> std::io::Result<()> {
        self.advertised = Some(advertise(SERVICE, port)?);
        Ok(())
    }
}

/// Say on the local network that this machine is offering `service` at this port.
///
/// The other end finds it by service rather than by address, so nothing has to be
/// configured on either machine and neither cares what the router handed out today.
/// The registration lasts as long as what comes back is held: dropping it withdraws
/// the advertisement, which is what keeps it from outliving the port it names.
///
/// The name is given rather than assumed, because this machine offers two of them at
/// once and each is at a port of its own ([`favjit_core::link::PAIRING`]).
pub fn advertise(service: &str, port: u16) -> std::io::Result<Advertisement> {
    {
        let service_type = std::ffi::CString::new(service).expect("a name with no nul in it");
        let mut registration: DnsServiceRef = std::ptr::null_mut();
        // Network byte order, which the header asks for. The name and the host are
        // left to the system: it uses this machine's name, which is what a person
        // would look for.
        let code = unsafe {
            DNSServiceRegister(
                &mut registration,
                0,
                DNS_SERVICE_INTERFACE_ANY,
                std::ptr::null(),
                service_type.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                port.to_be(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if code != 0 {
            return Err(std::io::Error::other(format!(
                "DNSServiceRegister returned {code}"
            )));
        }
        info!("advertised {service} on port {port}");
        Ok(Advertisement(registration))
    }
}

impl LinkHost for Link {
    fn advertise(&mut self) -> bool {
        let port = match self.port() {
            Ok(port) => port,
            Err(error) => {
                error!("cannot tell which port to advertise: {error}");
                return false;
            }
        };
        match self.register(port) {
            Ok(()) => true,
            Err(error) => {
                error!("cannot advertise the link over mDNS: {error}");
                false
            }
        }
    }

    fn accept(&mut self) -> Accepted {
        match self.listener.accept() {
            Ok((stream, from)) => {
                // The timeout goes on here because it is a property of the socket:
                // a read with none is a thread parked forever on a machine that was
                // unplugged, holding the connection the next one needs.
                if let Err(error) = stream.set_read_timeout(Some(IDLE)) {
                    warn!("cannot put a timeout on the connection from {from}: {error}");
                    return Accepted::Failed;
                }
                let _ = stream.set_nodelay(true);
                info!("something connected from {from}");
                self.connected = Some(stream);
                Accepted::Connected
            }
            Err(error) => {
                error!("cannot accept on the link: {error}");
                if recoverable(&error) {
                    Accepted::Failed
                } else {
                    Accepted::Done
                }
            }
        }
    }

    fn take_handshake(&mut self) -> Option<[u8; HANDSHAKE]> {
        let mut first = [0u8; HANDSHAKE];
        self.connected
            .as_mut()?
            .read_exact(&mut first)
            .inspect_err(|error| info!("nothing arrived from a connection: {error}"))
            .ok()?;
        Some(first)
    }

    fn answer(&mut self, first: &[u8; HANDSHAKE]) -> Option<[u8; ANSWER]> {
        let mut responder = Responder::new(&self.identity)
            .inspect_err(|error| error!("cannot start a handshake: {error}"))
            .ok()?;
        let answer = responder
            .answer(first)
            .inspect_err(|error| info!("a connection did not get in: {error}"))
            .ok()?;
        self.shaking = Some(responder);
        Some(answer)
    }

    fn send_answer(&mut self, answer: &[u8; ANSWER]) -> bool {
        let Some(stream) = self.connected.as_mut() else {
            return false;
        };
        stream
            .write_all(answer)
            .and_then(|()| stream.flush())
            .inspect_err(|error| info!("cannot answer a connection: {error}"))
            .is_ok()
    }

    fn peer(&mut self) -> Option<Vec<u8>> {
        let (peer, session) = self
            .shaking
            .take()?
            .done()
            .inspect_err(|error| info!("the handshake left no session: {error}"))
            .ok()?;
        self.open = Some((self.connected.take()?, session));
        Some(peer)
    }

    fn authorized(&mut self) -> Authorized {
        Authorized::parse(&std::fs::read_to_string(&self.authorized_at).unwrap_or_default())
    }

    fn take_record(&mut self) -> Incoming {
        let Some((stream, _)) = self.open.as_mut() else {
            return Incoming::Ended;
        };
        let mut record = [0u8; SEALED];
        if let Err(error) = stream.read_exact(&mut record) {
            info!("the link closed: {error}");
            return Incoming::Ended;
        }
        Incoming::Record(record)
    }

    fn open(&mut self, record: &[u8; SEALED]) -> Option<[u8; FRAME]> {
        let (_, session) = self.open.as_mut()?;
        session.open(record)
    }

    fn deliver(&mut self, kind: EventKind) -> bool {
        self.events
            .send(Captured::Event(HostEvent::new(self.clock.now(), kind)))
            .is_ok()
    }

    fn close(&mut self, reason: &str) {
        info!("let the connection go: {reason}");
        self.connected = None;
        self.open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use favjit_core::link::Message;
    use favjit_core::noise::{keypair, Initiator};
    use favjit_core::{DeviceId, Key};

    #[test]
    fn a_connection_that_went_away_says_nothing_about_the_socket() {
        // The one that decides whether a port scan ends the link: each of these is a
        // connection that stopped existing, not a socket that stopped working.
        use std::io::{Error, ErrorKind};

        assert!(recoverable(&Error::from(ErrorKind::ConnectionAborted)));
        assert!(recoverable(&Error::from(ErrorKind::ConnectionReset)));
        assert!(recoverable(&Error::from(ErrorKind::Interrupted)));

        // And anything this end cannot name is counted as the socket, because a kind
        // that never clears would otherwise be a loop that never stops trying.
        assert!(!recoverable(&Error::from(ErrorKind::PermissionDenied)));
        assert!(!recoverable(&Error::other("something unnamed")));
    }

    #[test]
    fn a_source_gets_in_over_a_socket_and_its_keystroke_arrives() {
        // Over loopback, because what this file adds to the exchange is the socket:
        // that a read takes exactly the length the other end wrote is the part no
        // simulated host can be wrong about, and the part that hangs when it is.
        let sink = keypair().expect("the sink's identity");
        let source = keypair().expect("the source's identity");
        let (events, from_the_link) = std::sync::mpsc::channel();
        let mut link = Link::bind(sink.clone(), Clock::start(), events).expect("a listener");
        let port = link.port().expect("a port");

        let public = source.public().to_vec();
        let typing = std::thread::spawn(move || {
            let mut initiator = Initiator::new(&source, sink.public()).expect("an initiator");
            let mut stream =
                TcpStream::connect(("127.0.0.1", port)).expect("the sink is listening");
            stream
                .write_all(&initiator.first().expect("the first message"))
                .expect("write");
            let mut answer = [0u8; ANSWER];
            stream.read_exact(&mut answer).expect("the answer");
            initiator.take_answer(&answer).expect("the answer opens");
            let mut session = initiator.done().expect("a session");

            let mut frame = [0u8; FRAME];
            Message::KeyDown {
                device: DeviceId(3),
                key: Key::K,
            }
            .encode(&mut frame);
            stream
                .write_all(&session.seal(&frame).expect("a sealed record"))
                .expect("write");
        });

        // In the order `core::link::serve` calls them, since that is the only order
        // these four are usable in and this file is what they reach.
        assert_eq!(link.accept(), Accepted::Connected);
        let first = link.take_handshake().expect("the first message arrives");
        let answer = link.answer(&first).expect("it opens");
        assert!(link.send_answer(&answer), "the answer goes back");
        let peer = link.peer().expect("the handshake leaves a session");
        let incoming = link.take_record();
        typing.join().expect("the source thread");

        assert_eq!(peer, public, "the key read is the source's own");
        assert_eq!(
            match incoming {
                Incoming::Record(record) => link
                    .open(&record)
                    .and_then(|frame| { Message::decode(&frame) }),
                Incoming::Ended => None,
            },
            Some(Message::KeyDown {
                device: DeviceId(3),
                key: Key::K
            })
        );
        drop(from_the_link);
    }
}
