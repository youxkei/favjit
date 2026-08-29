//! The socket, the handshake and the files behind the source's link (ADR-0017).
//!
//! The initiating end of what `host-macos` responds to. `IK` is the pattern where
//! the initiator already knows the responder's static key and sends its own
//! inside the handshake, so this end has to have pinned the sink before it can
//! say anything at all — and a sink that has not pinned this machine refuses by
//! the handshake not completing (ADR-0004).
//!
//! **What the two machines agree on is not here.** The pattern, the frame, the
//! sealed record and the two handshake lengths are [`favjit_core::link`]'s, stated
//! once for both ends: a copy that disagrees is not an error but a read waiting for
//! bytes nobody will send. What is here is this end's own timings, which are
//! nobody else's business, and the calls.
//!
//! Not compiled as Windows-only, and deliberately: nothing in here touches the
//! platform, and the lengths are worth checking against a Noise implementation
//! wherever the suite runs.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use favjit_core::link::{
    exactly, Message, WrongLength, ANSWER, FRAME, HANDSHAKE, PAIRING, PATTERN, SEALED, SERVICE,
};
use favjit_core::pairing::{key_from_hex, Identity, IdentityStore, NoIdentity};
use favjit_core::source::Connected;
use log::{error, info, warn};

/// How long to wait for the sink to answer a handshake.
///
/// The machine is on the same desk, so anything longer than this is a machine
/// that is not going to answer — and while this is outstanding the keyboards are
/// the Windows machine's own, which is the state a person can work in.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(5);

/// How long to look for the sink over mDNS before saying it is not there.
const LOOK: Duration = Duration::from_secs(2);

/// How long to wait before looking again after a failed attempt.
///
/// Without it a run given a fixed address would retry as fast as the connection
/// is refused, which is a busy loop on a machine whose Mac is switched off.
const RETRY: Duration = Duration::from_secs(3);

/// How long a keystroke may sit in the socket before the link counts as gone.
///
/// A send that cannot complete has to fail rather than wait: the loop that is
/// calling it is the one holding the keyboards, and it gives them back as soon as
/// the send says the link has gone (ADR-0006, ADR-0008).
const SEND_WAIT: Duration = Duration::from_secs(2);

/// Where the two files live.
///
/// Under the person's own profile, because that is who runs this end: there is no
/// service to install here, so favjit on Windows lives in a login session. A
/// machine-wide directory would be readable by every user, and the identity is the
/// whole of what makes the Mac accept keystrokes from here — so keeping it out of
/// other users' reach is what the profile does for free, with no ACL to narrow
/// afterwards.
///
/// `USERPROFILE` behind `LOCALAPPDATA` because they are the same place spelled two
/// ways. With neither, what is left is a relative directory, and the write that
/// fails names it — which is how an environment with no profile at all becomes
/// visible rather than silently written to somewhere else.
pub fn favjit_directory() -> PathBuf {
    let local = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|_| {
            std::env::var("USERPROFILE").map(|home| PathBuf::from(home).join(r"AppData\Local"))
        })
        .unwrap_or_default();
    local.join("favjit")
}

pub fn identity_path() -> PathBuf {
    favjit_directory().join("identity")
}

/// Where the sink this machine will send input to is written down.
///
/// One key and not a list, which is the asymmetry ADR-0004 describes: a sink
/// decides which sources may type on it and can have several, and a source has
/// exactly one machine it is willing to hand its keyboard to.
pub fn sink_path() -> PathBuf {
    favjit_directory().join("sink")
}

/// Where a machine showing a pairing code is, as mDNS answers now.
///
/// A question of its own rather than the one a session asks: a converting sink offers
/// both at once and each is at a port of its own, so asking for the link would as
/// readily find the listener that wants a handshake (ADR-0017).
pub fn find_pairing() -> std::io::Result<Option<SocketAddr>> {
    crate::mdns::find(PAIRING, LOOK)
}

/// This machine's identity, read or made.
///
/// The sequence — read it, and only if there is nothing there make one and keep it,
/// and never overwrite what will not parse — is [`favjit_core::pairing::identity`]'s.
/// The three calls below are all this end contributes, which is what keeps the two
/// machines' answers to "what happens on first run" the same one.
pub fn identity() -> Result<Identity, NoIdentity> {
    favjit_core::pairing::identity(&mut IdentityFile(identity_path()))
}

/// The identity file, as the sequence in `core` reads and writes it.
struct IdentityFile(PathBuf);

impl IdentityStore for IdentityFile {
    fn read(&mut self) -> Option<Vec<u8>> {
        std::fs::read(&self.0).ok()
    }

    fn make(&mut self) -> Option<Identity> {
        let keypair = snow::Builder::new(pattern())
            .generate_keypair()
            .inspect_err(|error| warn!("cannot generate a keypair: {error}"))
            .ok()?;
        Identity::new(keypair.private, keypair.public)
    }

    fn keep(&mut self, bytes: &[u8]) -> bool {
        if let Some(parent) = self.0.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                warn!("cannot make {}: {error}", parent.display());
                return false;
            }
        }
        if let Err(error) = std::fs::write(&self.0, bytes) {
            warn!("cannot write {}: {error}", self.0.display());
            return false;
        }
        info!("made a new identity at {}", self.0.display());
        true
    }
}

/// The sink this machine has pinned, if it has pinned one.
pub fn read_sink(path: &Path) -> Option<Vec<u8>> {
    let text = std::fs::read_to_string(path).ok()?;
    // The same reader a person's own line goes through on the other machine, so
    // a key pasted between them is the same key on both.
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(key_from_hex)
}

/// Write down the sink this machine will send input to.
///
/// Replaced rather than appended to: there is one sink, and a file that
/// accumulated them would leave which one is in use depending on the order they
/// were written.
pub fn pin_sink(path: &Path, key: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{}\n", favjit_core::pairing::hex(key)))
}

/// The socket and the session, as the source's loop needs them.
pub struct Link {
    identity: Identity,
    sink: Vec<u8>,
    /// An address given on the command line, instead of looking for one.
    ///
    /// Here because mDNS is the one part of finding the sink that depends on the
    /// network behaving: a segment that drops multicast leaves the two machines
    /// unable to find each other with nothing wrong at either end, and typing an
    /// address is the way out of that.
    fixed: Option<SocketAddr>,
    open: Option<(TcpStream, snow::TransportState)>,
    /// Where the last look found the sink, for the session that opens next.
    ///
    /// Kept here rather than handed over and back, because an address is a thing to
    /// connect with and not anything the sequence above reads.
    found: Option<SocketAddr>,
}

impl Link {
    pub fn new(identity: Identity, sink: Vec<u8>, fixed: Option<SocketAddr>) -> Self {
        Self {
            identity,
            sink,
            fixed,
            open: None,
            found: None,
        }
    }

    /// Wait before looking again.
    pub fn pause(&self) {
        std::thread::sleep(RETRY);
    }

    /// Look for the sink, and remember where it is.
    pub fn find(&mut self) -> Connected {
        // Whatever was open is gone, or this would not have been called. Dropped
        // here rather than left until the next session opens, so a socket to a
        // machine that stopped answering is not still held while one is looked for.
        self.open = None;
        self.found = self.address();
        match self.found.is_some() {
            true => Connected::Ready,
            false => Connected::NotFound,
        }
    }

    /// Connect to what was found and shake hands with it.
    pub fn open(&mut self) -> bool {
        let Some(address) = self.found else {
            return false;
        };
        let mut stream = match TcpStream::connect_timeout(&address, HANDSHAKE_WAIT) {
            Ok(stream) => stream,
            Err(error) => {
                info!("cannot reach the sink at {address}: {error}");
                return false;
            }
        };
        // Every message on this link is one keystroke, so waiting to fill a
        // packet would be waiting for the next key to be pressed.
        let _ = stream.set_nodelay(true);
        if let Err(error) = stream
            .set_read_timeout(Some(HANDSHAKE_WAIT))
            .and_then(|()| stream.set_write_timeout(Some(SEND_WAIT)))
        {
            warn!("cannot put timeouts on the link to {address}: {error}");
            return false;
        }

        match self.shake_hands(&mut stream) {
            Ok(session) => {
                info!("the link to {address} is up");
                self.open = Some((stream, session));
                true
            }
            Err(error) => {
                // The refusal ADR-0004 asks for looks exactly like this: a sink
                // that has not pinned this machine ends the handshake rather than
                // answering a question about it.
                warn!(
                    "the sink at {address} did not complete the handshake ({error}); it may not \
                     have paired this machine — pair again, with favjit --pair on the Mac and \
                     favjit --pair <those digits> here"
                );
                false
            }
        }
    }

    /// Where to connect, either because it was given or because mDNS said so.
    fn address(&self) -> Option<SocketAddr> {
        if let Some(address) = self.fixed {
            return Some(address);
        }
        match crate::mdns::find(SERVICE, LOOK) {
            Ok(Some(address)) => Some(address),
            Ok(None) => {
                info!(
                    "nothing is advertising {} yet",
                    favjit_core::discovery::service(SERVICE)
                );
                None
            }
            Err(error) => {
                error!("cannot look for the sink: {error}");
                None
            }
        }
    }

    /// The two handshake messages, as `snow` and the socket exchange them.
    ///
    /// Each length is checked against what `core` states rather than trusted,
    /// because the far end reads exactly that many bytes: a message written at any
    /// other length is a read that waits for bytes nobody will send, which hangs
    /// rather than fails.
    fn shake_hands(&self, stream: &mut TcpStream) -> std::io::Result<snow::TransportState> {
        let mut session = snow::Builder::new(pattern())
            .local_private_key(self.identity.private())
            .and_then(|builder| builder.remote_public_key(&self.sink))
            .and_then(snow::Builder::build_initiator)
            .map_err(noise)?;

        let mut first = [0u8; HANDSHAKE];
        let wrote = session.write_message(&[], &mut first).map_err(noise)?;
        measured(exactly("the first message", wrote, HANDSHAKE))?;
        stream.write_all(&first)?;
        stream.flush()?;

        let mut answer = [0u8; ANSWER];
        stream.read_exact(&mut answer)?;
        let mut opened = [0u8; ANSWER];
        session.read_message(&answer, &mut opened).map_err(noise)?;

        session.into_transport_mode().map_err(noise)
    }

    /// Hand one message to the sink.
    ///
    /// `false` once the link has gone, which is what the loop above turns into
    /// giving the keyboards back.
    pub fn send(&mut self, message: Message) -> bool {
        let Some((stream, session)) = self.open.as_mut() else {
            return false;
        };
        let mut frame = [0u8; FRAME];
        message.encode(&mut frame);
        let mut sealed = [0u8; SEALED];
        let record = match session.write_message(&frame, &mut sealed) {
            Ok(record) => record,
            Err(error) => {
                // A cipher state that will not seal is one no later message can
                // be sealed with either, so the session is over.
                warn!("cannot seal a message for the sink: {error}");
                self.open = None;
                return false;
            }
        };
        if let Err(wrong) = exactly("a sealed record", record, SEALED) {
            // The sink reads exactly `SEALED` bytes, so anything else desyncs the
            // stream rather than failing at it.
            warn!("{wrong}");
            self.open = None;
            return false;
        }
        if let Err(error) = stream.write_all(&sealed) {
            info!("the link has gone: {error}");
            self.open = None;
            return false;
        }
        true
    }

    /// This machine's key, as a log line names it.
    pub fn fingerprint(&self) -> String {
        self.identity.fingerprint()
    }
}

fn pattern() -> snow::params::NoiseParams {
    PATTERN.parse().expect("a pattern this crate has")
}

fn noise(error: snow::Error) -> std::io::Error {
    std::io::Error::other(format!("{error}"))
}

fn measured(length: Result<(), WrongLength>) -> std::io::Result<()> {
    length.map_err(|wrong| std::io::Error::other(format!("{wrong}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use favjit_core::{DeviceId, Key};

    #[test]
    fn a_sink_pinned_in_a_file_is_read_back_as_the_key_that_was_written() {
        let path = std::env::temp_dir().join("favjit-test-sink");
        let key = vec![0x5au8; favjit_core::pairing::KEY];
        pin_sink(&path, &key).expect("the file is written");
        assert_eq!(read_sink(&path), Some(key));

        // Pinning again replaces it rather than adding to it: two keys in the
        // file would make which sink is in use depend on the order.
        let other = vec![0xa5u8; favjit_core::pairing::KEY];
        pin_sink(&path, &other).expect("the file is written");
        assert_eq!(read_sink(&path), Some(other));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_with_no_key_in_it_pins_nothing() {
        // What a half-finished setup looks like. Nothing is the answer that keeps
        // the source from connecting, which is the direction ADR-0004 wants it to
        // fail in.
        let path = std::env::temp_dir().join("favjit-test-sink-empty");
        std::fs::write(&path, "# the mac goes here\n\n").expect("the file is written");
        assert_eq!(read_sink(&path), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn this_end_and_a_noise_responder_agree_on_the_lengths_and_the_records() {
        // Against `snow` directly and over loopback, because what is being
        // checked is that the numbers **`core`** states are the ones a Noise
        // implementation actually writes: a length that is wrong is a read that
        // blocks for ever, and this is the one part of the link no simulated host
        // reaches. The sink's own half checks the same constants from its side.
        let sink = snow::Builder::new(pattern())
            .generate_keypair()
            .expect("a keypair");
        let source = snow::Builder::new(pattern())
            .generate_keypair()
            .expect("a keypair");

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a listener");
        let port = listener.local_addr().expect("an address").port();

        let sink_private = sink.private.clone();
        let source_public = source.public.clone();
        let responder = std::thread::spawn(move || {
            let mut session = snow::Builder::new(pattern())
                .local_private_key(&sink_private)
                .expect("the sink's key")
                .build_responder()
                .expect("a responder");
            let (mut stream, _) = listener.accept().expect("the source connects");

            let mut incoming = [0u8; 512];
            let mut buffer = [0u8; 512];
            let first = stream.read(&mut incoming).expect("the first message");
            session
                .read_message(&incoming[..first], &mut buffer)
                .expect("the first message opens");
            assert_eq!(
                session.get_remote_static().map(<[u8]>::to_vec),
                Some(source_public),
                "the key the sink reads is this end's own"
            );
            let reply = session.write_message(&[], &mut buffer).expect("the reply");
            stream.write_all(&buffer[..reply]).expect("write");
            let mut session = session.into_transport_mode().expect("a session");

            let mut sealed = [0u8; 512];
            let record = stream.read(&mut sealed).expect("a record");
            let mut frame = [0u8; FRAME];
            let opened = session
                .read_message(&sealed[..record], &mut frame)
                .expect("the record opens");

            (first, reply, record, opened, Message::decode(&frame))
        });

        let identity = Identity::new(source.private, source.public).expect("an identity");
        let mut link = Link::new(
            identity,
            sink.public.clone(),
            Some(SocketAddr::from(([127, 0, 0, 1], port))),
        );
        assert_eq!(link.find(), Connected::Ready);
        assert!(link.open(), "the session opens");
        assert!(link.send(Message::KeyDown {
            device: DeviceId(3),
            key: Key::K,
        }));

        let (first, reply, record, opened, message) = responder.join().expect("the sink thread");
        assert_eq!(first, HANDSHAKE, "the first handshake message's length");
        assert_eq!(reply, ANSWER, "the answer's length");
        assert_eq!(record, SEALED, "a sealed record's length");
        assert_eq!(opened, FRAME, "a record opens to exactly one frame");
        assert_eq!(
            message,
            Some(Message::KeyDown {
                device: DeviceId(3),
                key: Key::K
            })
        );
    }

    #[test]
    fn a_sink_that_is_not_listening_is_reported_rather_than_raised() {
        // The ordinary state of a machine that has not been switched on. The loop
        // above waits for it, and an error would make it stop instead.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a listener");
        let port = listener.local_addr().expect("an address").port();
        drop(listener);

        let identity = Identity::new(vec![1; 32], vec![2; 32]).expect("an identity");
        let mut link = Link::new(
            identity,
            vec![3; 32],
            Some(SocketAddr::from(([127, 0, 0, 1], port))),
        );
        // A fixed address is always "found"; being unreachable is the session
        // failing to open, which is the same round again to the sequence above.
        assert_eq!(link.find(), Connected::Ready);
        assert!(!link.open(), "nothing is listening there");
    }
}
