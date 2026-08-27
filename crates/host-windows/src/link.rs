//! The socket and the files behind the source's link (ADR-0012).
//!
//! **What the two machines agree on is not here.** The pattern, the frame, the
//! sealed record and the two handshake lengths, and the handshake and the
//! sealing themselves, are `engine`'s (ADR-0006): a copy that disagrees is not
//! an error but a read waiting for bytes nobody will send, and a host that
//! drove the construction itself would be a decision no test can reach. What is
//! here is this end's own timings, the socket, and the files — one call and a
//! conversion, the same shape `host-macos`'s link answers the sink's half of
//! (ADR-0012).
//!
//! Not compiled as Windows-only, and deliberately: nothing in here touches the
//! platform.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};

use favjit_host::{Entropy, IdentityStore, Trouble};

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
    let local = local_app_data()
        .or_else(|| user_profile().map(|home| home.join(r"AppData\Local")))
        .unwrap_or_default();
    local.join("favjit")
}

/// `LOCALAPPDATA`, where this profile keeps what belongs to it.
fn local_app_data() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA").ok().map(PathBuf::from)
}

/// `USERPROFILE`, the same place spelled another way.
fn user_profile() -> Option<PathBuf> {
    std::env::var("USERPROFILE").ok().map(PathBuf::from)
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

/// The identity file, as `engine`'s sequence reads and writes it.
pub struct IdentityFile {
    pub path: PathBuf,
}

impl IdentityFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
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
        opened(
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
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

/// What the machine said about something it would not do to a path, named by
/// the path: a person told the identity cannot be kept has to be told which
/// file, and whether it is a permission or a full disk is the machine's own
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

/// The same source [`Link`]'s own ephemeral key comes from: a generator seeded
/// in this process would be one more thing to be wrong about, for a key this
/// file exists precisely to make once and keep.
impl Entropy for IdentityFile {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        getrandom::getrandom(into).is_ok()
    }
}

/// What is in the file naming the one machine this one relays to.
///
/// The text, not the key: which lines of it are keys, what a key looks like
/// written down, and which one of several counts are all questions about the
/// file's content, and the other machine reads its own list with the same
/// answers — so both are given once, where the suite drives them (ADR-0006).
pub fn read_sink(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The socket behind the source's link, as bytes alone past the address.
///
/// The handshake and the sealing are `engine`'s, driven a call at a time through
/// [`favjit_host::source::SourceHost`]'s raw methods: this holds only the
/// address, the stream once one is open, and the timings nobody else's business
/// depends on.
pub struct Link {
    /// An address given on the command line, instead of looking for one.
    ///
    /// Here because mDNS is the one part of finding the sink that depends on the
    /// network behaving: a segment that drops multicast leaves the two machines
    /// unable to find each other with nothing wrong at either end, and typing an
    /// address is the way out of that.
    fixed: Option<SocketAddr>,
    discovery: crate::mdns::Network,
}

impl Link {
    pub fn new(fixed: Option<SocketAddr>) -> Self {
        Self {
            fixed,
            discovery: crate::mdns::Network,
        }
    }

    /// Wait before looking again.
    pub fn pause(&self, how_long: core::time::Duration) {
        std::thread::sleep(how_long);
    }

    pub fn use_fixed(&mut self) -> Option<favjit_host::Sink> {
        self.fixed
    }

    /// Open the socket one search is made over.
    pub fn bind_discovery(&mut self) -> Option<Box<dyn favjit_host::Searching + '_>> {
        favjit_host::DiscoveryHost::bind_discovery(&mut self.discovery)
    }

    /// Connect to what was found.
    ///
    /// Bytes only past this: the handshake that follows is `engine`'s. What a
    /// refusal is worth saying about is the run's too — a machine that is not
    /// there is the ordinary state of one nobody has switched on (ADR-0006).
    pub fn connect(
        &mut self,
        sink: favjit_host::Sink,
        timeout: core::time::Duration,
    ) -> Option<Box<dyn favjit_host::source::Opening>> {
        connected(TcpStream::connect_timeout(&sink, timeout))
    }
}

/// The connection a connect opened, as the run holds it while a session is being
/// opened over it — and nothing where it opened none.
///
/// Handed over rather than kept here, so that a run looking again is one whose
/// last answer is not written to: a socket this side held on to would carry
/// records to the machine that answered last time, and one the run let go of is
/// closed by the letting go.
fn connected(
    answered: std::io::Result<TcpStream>,
) -> Option<Box<dyn favjit_host::source::Opening>> {
    answered
        .ok()
        .map(|socket| Box::new(Open(socket)) as Box<dyn favjit_host::source::Opening>)
}

/// One connection while a session is being opened over it.
struct Open(TcpStream);

impl favjit_host::source::Opening for Open {
    fn set_nodelay(&mut self, enabled: bool) -> bool {
        went(self.0.set_nodelay(enabled))
    }

    fn set_read_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.0.set_read_timeout(Some(timeout)))
    }

    fn set_write_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.0.set_write_timeout(Some(timeout)))
    }

    fn send_first_message(&mut self, first: &[u8]) -> bool {
        went(self.0.write_all(first))
    }

    fn flush_first_message(&mut self) -> bool {
        went(self.0.flush())
    }

    fn take_answer(&mut self, into: &mut [u8]) -> bool {
        went(self.0.read_exact(into))
    }

    fn keep(self: Box<Self>) -> Box<dyn favjit_host::source::Sending> {
        Box::new(Writing(self.0))
    }
}

/// One connection with a session going over it.
struct Writing(TcpStream);

impl favjit_host::source::Sending for Writing {
    /// Hand one sealed record to the sink, answering `0` for the write that went
    /// and the error's own number otherwise.
    ///
    /// The number is carried rather than read: which failure it was is the
    /// difference between the sink having gone and this write having run out of
    /// the time the socket was given, and those are the same "it failed" — so
    /// what the run is handed is the number, and what it says about it is its own
    /// (ADR-0006, ADR-0009).
    fn send(&mut self, sealed: &[u8]) -> i32 {
        written(self.0.write_all(sealed))
    }
}

/// Whether a call did what it was asked, as its own answer says.
fn went(answered: std::io::Result<()>) -> bool {
    answered.is_ok()
}

/// What the machine said about a write: `0` for the one that went, and its own
/// number otherwise.
///
/// `-1` where the failure carries no number of the platform's, which is the same
/// answer a call that could not be made at all gets: a write that ran out of time
/// and a connection the other end reset are different numbers rather than the
/// same failure, and the recording is where those numbers survive (ADR-0009).
fn written(write: std::io::Result<()>) -> i32 {
    match write {
        Ok(()) => 0,
        Err(error) => error.raw_os_error().unwrap_or(-1),
    }
}

impl Entropy for Link {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        // The same source the static keys come from. A generator seeded in this
        // process would be one more thing to be wrong about, and the code
        // protects the handshake only while it is unpredictable.
        getrandom::getrandom(into).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes of no particular protocol, because the protocol is not what is
    /// under test: which bytes make a handshake and what they open into is
    /// `engine`'s and is driven there against the simulated link. What only a
    /// real socket can show is that two writes and a read, made through the
    /// operations above in the order a run makes them, carry the bytes that
    /// were written and nothing else — and that the socket the handshake was
    /// made on is the one the record goes out on afterwards.
    #[test]
    fn this_end_and_a_listening_sink_agree_on_what_was_written() {
        let first = [1u8; 96];
        let answer = [2u8; 48];
        let record = [3u8; 48];

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a listener");
        let port = listener.local_addr().expect("an address").port();

        let sink = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the source connects");
            let mut arrived = [0u8; 96];
            stream.read_exact(&mut arrived).expect("the first message");
            stream.write_all(&answer).expect("write");
            let mut sealed = [0u8; 48];
            stream.read_exact(&mut sealed).expect("a record");
            (arrived, sealed)
        });

        let mut link = Link::new(Some(SocketAddr::from(([127, 0, 0, 1], port))));
        let reach = link.use_fixed().expect("the configured address");
        let mut open = link
            .connect(reach, core::time::Duration::from_secs(5))
            .expect("the socket connects");
        assert!(open.send_first_message(&first));
        assert!(open.flush_first_message());
        let mut answered = [0u8; 48];
        assert!(open.take_answer(&mut answered));
        assert_eq!(answered, answer);
        // Kept here for the reason a run keeps it: the handshake is over, and
        // what carries the record that follows is the same socket it was made
        // on.
        let mut writing = open.keep();
        assert_eq!(writing.send(&record), 0);

        assert_eq!(sink.join().expect("the sink thread"), (first, record));
    }

    #[test]
    fn a_sink_that_is_not_listening_is_reported_rather_than_raised() {
        // The ordinary state of a machine that has not been switched on. The loop
        // above waits for it, and an error would make it stop instead.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a listener");
        let port = listener.local_addr().expect("an address").port();
        drop(listener);

        let mut link = Link::new(Some(SocketAddr::from(([127, 0, 0, 1], port))));
        // A fixed address is always "found"; being unreachable is the socket
        // failing to open, which is the same round again to the sequence above.
        let sink = link.use_fixed().expect("the configured address");
        assert!(
            link.connect(sink, core::time::Duration::from_secs(5))
                .is_none(),
            "nothing is listening there"
        );
    }
}
