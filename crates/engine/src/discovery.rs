//! The sequence that carries one mDNS lookup across the host boundary.
//!
//! The host owns the UDP socket, monotonic clock and resolver calls. This module
//! owns their order, the deadline, the wire format and which answer completes the
//! lookup (ADR-0006).

use core::time::Duration;

use favjit_host::{Datagram, DiscoveryHost, Instant, Sink};

/// The largest mDNS answer worth reading.
///
/// One answer is one datagram, and this holds far more records than one service can
/// produce while bounding what the network can make the run allocate.
const DATAGRAM_BYTES: usize = 4096;

/// The multicast hop limit: the peer is the other end of the desk, not a service to
/// route beyond the local network.
const MULTICAST_TTL: u32 = 1;

fn deadline(start: Instant, patience: Duration) -> Instant {
    Instant {
        nanos: start
            .nanos
            .saturating_add(u64::try_from(patience.as_nanos()).unwrap_or(u64::MAX)),
    }
}

fn remaining(deadline: Instant, now: Instant) -> Duration {
    Duration::from_nanos(deadline.nanos.saturating_sub(now.nanos))
}

/// Look for one service until `patience` runs out.
///
/// `None` includes both ordinary silence and a failed host operation. The caller
/// decides whether either one is worth another attempt.
pub fn find(service: &str, patience: Duration, host: &mut dyn DiscoveryHost) -> Option<Sink> {
    // The search is given up before the address it produced is resolved, because
    // it is borrowed from the machine for as long as it is open: resolving reaches
    // a resolver and not that socket, so it is the machine's again by then.
    let found = searched(service, patience, host)?;
    host.resolve_discovered(&found.host, found.port, found.address)
}

/// One lookup over an open socket, until something answers or the time is up.
fn searched(
    service: &str,
    patience: Duration,
    host: &mut dyn DiscoveryHost,
) -> Option<favjit_discovery::Found> {
    let mut searching = host.bind_discovery()?;
    if !searching.set_ttl(MULTICAST_TTL) {
        return None;
    }

    let deadline = deadline(searching.start_clock(), patience);
    let question = favjit_discovery::question(service);
    if !searching.ask(&question) {
        return None;
    }

    let mut answer = [0u8; DATAGRAM_BYTES];
    loop {
        let left = remaining(deadline, searching.now());
        if left.is_zero() || !searching.set_timeout(left) {
            return None;
        }
        let read = match searching.receive(&mut answer) {
            Datagram::Received(read) if read <= answer.len() => read,
            Datagram::TimedOut | Datagram::Failed => return None,
            Datagram::Received(_) => return None,
            _ => return None,
        };
        if let Some(found) = favjit_discovery::answer(&answer[..read], service) {
            return Some(found);
        }
    }
}
