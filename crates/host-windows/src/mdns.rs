//! The socket the sink is looked for over.
//!
//! Calls and nothing else: a datagram out, and datagrams in until the time is up.
//! What the answers mean, which of them is the sink, and what a malformed one is
//! worth are [`favjit_core::discovery`]'s, where the suite drives them (ADR-0006).
//!
//! One-shot multicast queries are part of mDNS for exactly this case: a question
//! sent from a port other than 5353 is answered to that port, so an ordinary UDP
//! socket sees the whole answer with no group to join and nothing to unsubscribe
//! from.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use favjit_core::discovery::{look, Discovery, Found};

/// Where every mDNS question goes.
const GROUP: (Ipv4Addr, u16) = (Ipv4Addr::new(224, 0, 0, 251), 5353);

/// The largest answer worth reading.
///
/// An answer is meant to fit in one datagram, and one this size holds far more
/// records than a single service can produce.
const DATAGRAM: usize = 4096;

/// Look for whoever offers `service`, for up to `patience`, and say where to connect.
///
/// `Ok(None)` when nothing usable answered, which is the ordinary state of a
/// machine that has not been switched on yet rather than a failure.
pub fn find(service: &str, patience: Duration) -> io::Result<Option<SocketAddr>> {
    let mut network = Network::open(patience)?;
    let Some(found) = look(&mut network, service) else {
        return Ok(None);
    };
    if let Some(error) = network.failed {
        return Err(error);
    }
    Ok(resolve(&found))
}

/// The socket, for as long as one search takes.
struct Network {
    socket: UdpSocket,
    deadline: Instant,
    buffer: [u8; DATAGRAM],
    /// The first failure that was not the read timing out.
    ///
    /// Kept rather than raised where it happens, because the surface `core` drives
    /// answers with what came back and not with how: a socket that broke has no
    /// more answers, which is the same shape as a network that had none.
    failed: Option<io::Error>,
}

impl Network {
    fn open(patience: Duration) -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        // Link-local, which is as far as the other end of the desk is.
        socket.set_multicast_ttl_v4(1)?;
        socket.set_read_timeout(Some(patience))?;
        Ok(Self {
            socket,
            deadline: Instant::now() + patience,
            buffer: [0; DATAGRAM],
            failed: None,
        })
    }
}

impl Discovery for Network {
    fn ask(&mut self, question: &[u8]) -> bool {
        match self.socket.send_to(question, GROUP) {
            Ok(_) => true,
            Err(error) => {
                self.failed = Some(error);
                false
            }
        }
    }

    fn next_answer(&mut self) -> Option<Vec<u8>> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        if let Err(error) = self.socket.set_read_timeout(Some(left)) {
            self.failed = Some(error);
            return None;
        }
        match self.socket.recv_from(&mut self.buffer) {
            Ok((read, _)) => Some(self.buffer[..read].to_vec()),
            Err(error) if timed_out(&error) => None,
            Err(error) => {
                self.failed = Some(error);
                None
            }
        }
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

/// The address to connect to, once something has answered.
fn resolve(found: &Found) -> Option<SocketAddr> {
    if let Some(address) = found.address {
        return Some(SocketAddr::from((Ipv4Addr::from(address), found.port)));
    }
    // The trailing dot a name carries is not part of a host name anywhere else, and
    // a resolver handed one looks up a name with an empty last label.
    let host = found.host.trim_end_matches('.');
    (host, found.port).to_socket_addrs().ok()?.next()
}
