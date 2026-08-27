//! The socket and the advertisement behind the sink's link (ADR-0012).
//!
//! Calls and nothing else, into a listening socket, Bonjour and two files. Who
//! is allowed in, when the list is read, what order the handshake happens in,
//! what becomes of a record that will not open, and what the two machines
//! agree on down to the lengths are all `engine`'s, where the suite can drive
//! them (ADR-0006) — this file has none of it, and cannot reach it to borrow
//! it (ADR-0005).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use favjit_host::link::{Accepted, Incoming, LinkHost};
use favjit_host::{Entropy, EventKind, HostEvent, IdentityStore, Trouble};

use crate::capture::{Captured, Clock};
use crate::ffi::{
    DNSServiceRefDeallocate, DNSServiceRegister, DnsServiceRef, DNS_SERVICE_INTERFACE_ANY,
};
use crate::urandom;

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

/// Who this machine will take input from, as the file's own text.
///
/// The text and not the parsed list: what counts as a key in it is `engine`'s
/// question, and a check here would be a second answer to it.
pub fn authorized() -> String {
    whatever_it_holds(std::fs::read_to_string(authorized_path()))
}

/// The text a file holds, and none where there is no file to read.
///
/// A machine that has paired with nobody has no file, which is an empty list
/// rather than a failure: refusing everyone is what an empty list already means.
fn whatever_it_holds(answered: std::io::Result<String>) -> String {
    answered.unwrap_or_default()
}

/// The identity file, as `engine`'s sequence reads and writes it.
///
/// The path is its own rather than a parameter, so that the file a person reads a
/// key out of with `--identity` is the one the running converter presents.
pub struct IdentityFile {
    path: PathBuf,
}

impl Default for IdentityFile {
    fn default() -> Self {
        Self {
            path: identity_path(),
        }
    }
}

impl IdentityStore for IdentityFile {
    fn read(&mut self) -> Option<Vec<u8>> {
        std::fs::read(&self.path).ok()
    }

    /// The path is in what comes back, because a person told the file cannot
    /// be written has to be told which file: it is the machine's own answer
    /// about its own filesystem, not a judgement about what to do next
    /// (ADR-0006).
    fn make_directory(&mut self) -> Result<(), Trouble> {
        made(
            std::fs::create_dir_all(holding(&self.path)),
            holding(&self.path),
        )
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, Trouble> {
        use std::os::unix::fs::OpenOptionsExt;

        opened(
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&self.path),
            &self.path,
        )
    }
}

/// The directory a file sits in, and the file's own path where it names no
/// directory — which is a root, and a root is already there.
pub(crate) fn holding(file: &Path) -> &Path {
    file.parent().unwrap_or(file)
}

/// What the machine said about a directory it would not make, named by the
/// directory: a person told the identity cannot be kept has to be told which
/// place, and whether it is a permission or a full disk is the machine's own
/// answer rather than a judgement about what to do next (ADR-0006).
pub(crate) fn made(answered: std::io::Result<()>, at: &Path) -> Result<(), Trouble> {
    answered.map_err(|error| Trouble(format!("{}: {error}", at.display())))
}

/// The file an open answered with, or what the machine said about not opening
/// it.
fn opened(
    answered: std::io::Result<std::fs::File>,
    at: &Path,
) -> Result<Box<dyn favjit_host::Writing>, Trouble> {
    match answered {
        Ok(file) => Ok(Box::new(OpenFile {
            file,
            at: at.to_path_buf(),
        })),
        Err(error) => Err(Trouble(format!("{}: {error}", at.display()))),
    }
}

/// One open identity file.
///
/// The path travels with it, because the name is what a failure is reported
/// against and a file handle carries none.
struct OpenFile {
    file: std::fs::File,
    at: PathBuf,
}

impl favjit_host::Writing for OpenFile {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Trouble> {
        made(self.file.write_all(bytes), &self.at)
    }
}

/// The same source every other entropy user in this crate reads: a generator
/// seeded here would be one more thing to be wrong about, for a key this
/// file exists precisely to make once and keep.
impl Entropy for IdentityFile {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        crate::urandom(into)
    }
}

/// The socket and the advertisement, as the sequence in `engine` needs them.
pub struct Link {
    listener: TcpListener,
    authorized_at: PathBuf,
    clock: Clock,
    events: Sender<Captured>,
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
    /// Bind the listener.
    ///
    /// The machine picks the port rather than a person, because the source finds
    /// this machine by name over mDNS and the advertisement carries the number: one
    /// to choose would be one more thing for the two ends to agree on, one more
    /// thing already taken on the day it is, and no easier to find.
    pub fn bind(clock: Clock, events: Sender<Captured>) -> std::io::Result<Self> {
        listening(TcpListener::bind(("0.0.0.0", 0)), clock, events)
    }

    pub fn port(&self) -> std::io::Result<u16> {
        the_port(self.listener.local_addr())
    }
}

/// This end over the socket a bind opened, or what the machine said about not
/// opening one.
fn listening(
    answered: std::io::Result<TcpListener>,
    clock: Clock,
    events: Sender<Captured>,
) -> std::io::Result<Link> {
    answered.map(|listener| Link {
        listener,
        authorized_at: authorized_path(),
        clock,
        events,
        advertised: None,
    })
}

/// The port an address carries.
fn the_port(answered: std::io::Result<std::net::SocketAddr>) -> std::io::Result<u16> {
    answered.map(|at| std::net::SocketAddr::port(&at))
}

/// Say on the local network that this machine is offering `service` at this port.
///
/// The other end finds it by service rather than by address, so nothing has to be
/// configured on either machine and neither cares what the router handed out today.
/// The registration lasts as long as what comes back is held: dropping it withdraws
/// the advertisement, which is what keeps it from outliving the port it names.
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
        an_advertisement(registered(code), registration)
    }
}

/// The registration a call answered with, or what the machine said about it
/// answering with none.
fn an_advertisement(
    answered: std::io::Result<()>,
    registration: DnsServiceRef,
) -> std::io::Result<Advertisement> {
    answered.map(|()| Advertisement(registration))
}

/// Whether a whole record came off the socket.
///
/// Anything short of the whole of one is the connection going: a record is a
/// fixed width and a session cannot be carried on from the middle of one.
fn arrived(read: std::io::Result<()>) -> Incoming {
    match read.is_ok() {
        true => Incoming::Record,
        false => Incoming::Gone,
    }
}

/// What the socket said about a refused accept, and nothing about what it means.
///
/// The kind alone: which kinds are one more wait and which are the socket being
/// unusable is a table, and a table beside the socket is a decision no test can
/// drive (ADR-0006).
fn whether_one_may_still_come(error: &std::io::Error) -> Accepted {
    Accepted::No(error.kind())
}

/// Whether the registration went, as the machine's own code says.
fn registered(code: i32) -> std::io::Result<()> {
    match code == 0 {
        true => Ok(()),
        false => Err(std::io::Error::other(format!(
            "DNSServiceRegister returned {code}"
        ))),
    }
}

impl LinkHost for Link {
    fn listener_port(&mut self) -> Option<u16> {
        self.port().ok()
    }

    fn advertise(&mut self, service: &str, port: u16) -> bool {
        self.advertised = advertise(service, port).ok();
        self.advertised.is_some()
    }

    fn accept(&mut self) -> Accepted {
        connected(self.listener.accept())
    }

    fn authorized(&mut self) -> Option<String> {
        std::fs::read_to_string(&self.authorized_at).ok()
    }

    fn deliver(&mut self, event: EventKind) -> bool {
        self.events
            .send(Captured::Event(HostEvent {
                at: self.clock.now(),
                kind: event,
            }))
            .is_ok()
    }

    /// The line arrives composed and this is the call that writes it, which is
    /// the one thing this operation is (ADR-0006).
    fn warn(&mut self, message: core::fmt::Arguments) {
        log::warn!("{message}");
    }
}

/// What an accept came back with: the connection, one more wait, or the socket.
fn connected(answered: std::io::Result<(TcpStream, std::net::SocketAddr)>) -> Accepted {
    match answered {
        Ok((stream, _)) => Accepted::Yes(Box::new(Talk(stream))),
        Err(error) => whether_one_may_still_come(&error),
    }
}

/// The one connection this end serves, from the moment something connects to the
/// moment the run lets it go.
///
/// What is made of the bytes crossing it — the handshake, the session — is
/// `engine::link::serve`'s to hold, not this type's (ADR-0006).
struct Talk(TcpStream);

impl favjit_host::link::Talking for Talk {
    fn set_read_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.0.set_read_timeout(Some(timeout)))
    }

    fn set_nodelay(&mut self, enabled: bool) -> bool {
        went(self.0.set_nodelay(enabled))
    }

    fn take_handshake(&mut self, into: &mut [u8]) -> bool {
        went(self.0.read_exact(into))
    }

    fn send_answer(&mut self, answer: &[u8]) -> bool {
        went(self.0.write_all(answer))
    }

    fn flush_answer(&mut self) -> bool {
        went(self.0.flush())
    }

    fn take_record(&mut self, into: &mut [u8]) -> Incoming {
        arrived(self.0.read_exact(into))
    }

    /// Nothing on this machine: the socket closes when the run lets this go, and
    /// which of the two ways it ended is a fact for whoever reads the run's own
    /// stream rather than something the platform is told.
    fn sending_it_away(&mut self) {}
}

/// Whether a call did what it was asked, as its own answer says.
fn went(answered: std::io::Result<()>) -> bool {
    answered.is_ok()
}

impl Entropy for Link {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        urandom(into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes of no particular protocol, because the protocol is not what is
    /// under test: which bytes make a handshake and what they open into is
    /// `engine`'s and is driven there against the simulated link. What only a
    /// real socket can show is that three reads and a write, made through the
    /// operations above in the order a run makes them, carry the bytes that
    /// were written at the other end and nothing else.
    #[test]
    fn a_source_gets_in_over_a_socket_and_what_it_wrote_arrives() {
        let first = [1u8; 96];
        let answer = [2u8; 48];
        let record = [3u8; 48];

        let (events, from_the_link) = std::sync::mpsc::channel();
        let mut link = Link::bind(Clock::start(), events).expect("a listener");
        let port = link.port().expect("a port");

        let typing = std::thread::spawn(move || {
            let mut stream =
                TcpStream::connect(("127.0.0.1", port)).expect("the sink is listening");
            stream.write_all(&first).expect("write");
            let mut answered = [0u8; 48];
            stream.read_exact(&mut answered).expect("the answer");
            stream.write_all(&record).expect("write");
            answered
        });

        // This order is load-bearing, not incidental: take_handshake before
        // send_answer before take_record each return nothing (or a stale
        // value) if called out of turn, so reordering these would not fail
        // loudly — it would pass on data that was never really there.
        let Accepted::Yes(mut open) = link.accept() else {
            panic!("the connection is taken");
        };
        let mut arrived = [0u8; 96];
        assert!(
            open.take_handshake(&mut arrived),
            "the first message arrives"
        );
        assert_eq!(arrived, first);
        assert!(open.send_answer(&answer), "the answer is written");
        assert!(open.flush_answer(), "the answer goes back");
        let mut sealed = [0u8; 48];
        assert_eq!(open.take_record(&mut sealed), Incoming::Record);
        assert_eq!(sealed, record);

        assert_eq!(typing.join().expect("the source thread"), answer);
        drop(from_the_link);
    }
}
