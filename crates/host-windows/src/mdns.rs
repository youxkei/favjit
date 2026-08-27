//! The socket the sink is looked for over.
//!
//! Calls and nothing else: a datagram out, and datagrams in until the time is up.
//! What the answers mean, which of them is the sink, and what a malformed one is
//! worth are [`favjit_discovery`]'s, where the suite drives them (ADR-0006).
//!
//! One-shot multicast queries are part of mDNS for exactly this case: a question
//! sent from a port other than 5353 is answered to that port, so an ordinary UDP
//! socket sees the whole answer with no group to join and nothing to unsubscribe
//! from.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};

use favjit_host::{Datagram, Instant};

/// Where every mDNS question goes.
const GROUP: (Ipv4Addr, u16) = (Ipv4Addr::new(224, 0, 0, 251), 5353);

/// This machine's local network, as the two things a lookup is made of.
///
/// Nothing is held: the socket belongs to [`Search`], which only exists once
/// one is open, and what a lookup resolved is the run's from the moment it is
/// answered — so nothing asked of either has to ask whether there is one
/// (ADR-0006).
pub struct Network;

/// One open socket, for as long as one search takes.
///
/// The clock's origin is taken when the socket is opened rather than left unset,
/// so that reading it is not a question about whether it was started: what
/// [`Searching::start_clock`] does is move the origin to now, which every
/// reading after it is relative to.
pub struct Search {
    socket: UdpSocket,
    origin: std::time::Instant,
}

impl favjit_host::Searching for Search {
    fn set_ttl(&mut self, ttl: u32) -> bool {
        went(self.socket.set_multicast_ttl_v4(ttl))
    }

    fn start_clock(&mut self) -> Instant {
        self.origin = std::time::Instant::now();
        Instant::default()
    }

    fn now(&mut self) -> Instant {
        since(self.origin.elapsed())
    }

    fn ask(&mut self, question: &[u8]) -> bool {
        sent(self.socket.send_to(question, GROUP))
    }

    fn set_timeout(&mut self, timeout: core::time::Duration) -> bool {
        went(self.socket.set_read_timeout(Some(timeout)))
    }

    fn receive(&mut self, into: &mut [u8]) -> Datagram {
        arrived(self.socket.recv_from(into))
    }
}

impl favjit_host::DiscoveryHost for Network {
    fn bind_discovery(&mut self) -> Option<Box<dyn favjit_host::Searching + '_>> {
        opened(UdpSocket::bind(("0.0.0.0", 0)))
    }

    fn resolve_discovered(
        &mut self,
        host: &str,
        port: u16,
        address: Option<[u8; 4]>,
    ) -> Option<favjit_host::Sink> {
        resolve(host, port, address)
    }
}

/// The search a bind opened, and nothing where it opened none.
fn opened(bound: io::Result<UdpSocket>) -> Option<Box<dyn favjit_host::Searching>> {
    bound.ok().map(|socket| {
        Box::new(Search {
            socket,
            origin: std::time::Instant::now(),
        }) as Box<dyn favjit_host::Searching>
    })
}

/// Whether a call did what it was asked, as its own answer says.
fn went(answered: io::Result<()>) -> bool {
    answered.is_ok()
}

/// Whether a question went, which is the same reading of a call that answers
/// with a count.
fn sent(answered: io::Result<usize>) -> bool {
    answered.is_ok()
}

/// How long since the clock's origin, in the run's own nanoseconds.
///
/// Saturating rather than wrapping: a run this far from its own origin has been
/// going for five hundred years, and a clock that wrapped would read as one that
/// had gone backwards.
fn since(elapsed: core::time::Duration) -> Instant {
    Instant {
        nanos: u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
    }
}

/// Whether an error is the read timeout coming due.
///
/// Two kinds, because the platforms do not agree on which one a socket timeout is,
/// and treating the wrong one as a failure would turn "nothing answered" into an
/// error the caller stops for.
fn timed_out(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// What one receive came back with.
///
/// A wait that came round with nothing is its own answer and not a failure: the
/// question goes out again, and a caller told it failed would stop looking for a
/// machine that is merely slow to answer.
fn arrived(received: io::Result<(usize, SocketAddr)>) -> Datagram {
    match received {
        Ok((read, _)) => Datagram::Received(read),
        Err(error) if timed_out(&error) => Datagram::TimedOut,
        Err(_) => Datagram::Failed,
    }
}

/// The address to connect to, once something has answered.
///
/// The literal one where the responder supplied it, which is the whole of this
/// where it did: a resolver reached for a name that came with its address would
/// be a call made for an answer already in hand.
pub fn resolve(host: &str, port: u16, address: Option<[u8; 4]>) -> Option<SocketAddr> {
    match address {
        Some(address) => Some(SocketAddr::from((Ipv4Addr::from(address), port))),
        None => looked_up(host, port),
    }
}

/// The address a resolver gives that name.
///
/// The trailing dot a name carries is not part of a host name anywhere else, and
/// a resolver handed one looks up a name with an empty last label.
fn looked_up(host: &str, port: u16) -> Option<SocketAddr> {
    the_first_of(look_up(host, port))
}

/// Ask the resolver.
fn look_up(host: &str, port: u16) -> io::Result<std::vec::IntoIter<SocketAddr>> {
    (host.trim_end_matches('.'), port).to_socket_addrs()
}

/// The first address a lookup answered with, and nothing where it answered with
/// none.
fn the_first_of(found: io::Result<std::vec::IntoIter<SocketAddr>>) -> Option<SocketAddr> {
    found.ok().and_then(|mut found| found.next())
}
