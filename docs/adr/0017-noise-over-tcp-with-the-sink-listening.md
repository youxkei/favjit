# ADR-0017: Carry input over a Noise session on TCP, with the sink listening

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

[ADR-0004](0004-peer-authentication.md) fixes what the link has to be: every
session mutually authenticated and encrypted, keyed on long-lived identities the
sink has pinned by an explicit action, and refusal by default. It states outright
that plain TCP with an application-level check is not enough, and leaves the
transport itself open.

What the link carries is small and constant: one event per keystroke, and pointer
reports at motion rates. Both matter for latency and neither for bandwidth. Nothing
in the design needs a stream in the other direction — input flows one way, from the
Windows source to the macOS sink ([ADR-0002](0002-input-topology.md)).

The two ends are not symmetric. The sink is the machine being controlled, it is the
one that authorises ([ADR-0004](0004-peer-authentication.md)), and it is the one
running as a supervised daemon that is always up ([ADR-0012](0012-macos-install-as-a-daemon-and-turn-off-with-a-file.md)).

## Decision

A Noise session in the `IK` pattern over a TCP connection, with the **sink
listening** and the source connecting.

`IK` is the pattern where the initiator already knows the responder's static public
key and sends its own inside the handshake. That is the shape of the two decisions
ADR-0004 makes: the source has pinned the sink it is willing to send input to, and
the sink learns which source is calling before the session exists and refuses one it
has not authorised. Refusal is a handshake that does not complete, not a check
layered over a session that does.

**What both machines agree on is stated once, in `core::link`**, beside the frame it
belongs with: the pattern string, the three lengths, the service name the source
looks for, the idle limit, and the rule that a message written at any other length is
refused. Neither host restates any of it ([ADR-0006](0006-host-boundary.md)).

**Each step of the handshake is its own host operation, and the order over them is
`core::link::serve`'s** — take the source's first message, open it and produce the
answer, send the answer, take the session and the key it proves — with taking a sealed
record separate from opening it. The platform's cryptography stays on its side of
that: what crosses is bytes. Then the list is read, and only a peer that is in it has
a frame read from it.

**Bringing the link up is the run's, not the host's.** `core::sink::run` establishes
the identity this machine presents over the three file operations of
`core::pairing::IdentityStore`, binds the socket with it, and hands the machine the
loop that serves it to turn alongside the converter's own ([ADR-0006](0006-host-boundary.md)):
the link waits on connections and the converter's loop must not. Saying on the network
that this machine is here is one of the link's own operations, done as its loop starts.
A machine that cannot turn the loop drops the link it was handed.

**A connection that could not be taken is not the socket failing, and `core` is what
counts them.** The host reports which of the two happened — this connection, or the
socket — and everything it cannot rule out recovering from is the first. `serve`
gives up after `FAILURES` of those in a row; a connection that becomes a session sets
the count back to zero. A count rather than a delay, because `core` has no clock
([ADR-0010](0010-clock-on-the-event-with-a-timer.md)).

**A link that stops being served ends the run**, whichever way it stopped. The
machine reports the loop it was handed coming back, after everything that loop put
into the stream, and `core` decides what that means: the run ends, having converted
what already arrived.

**Pairing is advertised under a name of its own**, `core::link::PAIRING`, at a port of
its own. The two things a sink offers are up at the same time on a converting machine
and neither is at a port anybody configured, so one name for both would leave a source
taking whichever answer arrived first: an offer spoken at a listener waiting for a
handshake, which fails as a pairing attempt that says nothing about why. With a name
each, a source asks for the thing it is doing, and pairing needs nothing switched off.
The key it writes down is in force for the next session, because the list is read at
every handshake rather than held from startup.

## Consequences

Authentication and refusal are the handshake's, not favjit's. There is no code path
in which a session is established and then rejected, so there is no code path in
which a mistake in that check lets input through.

TCP means head-of-line blocking: a lost packet delays the keystrokes behind it. That
is the right trade for a keyboard, where a keystroke arriving late is recoverable and
a keystroke arriving out of order is not.

The sink accepts connections, so the source needs no reachable address and no
inbound firewall rule; the machine that has to be reachable is the one that is always
running the daemon. A sink that is off is a source that cannot connect, which is the
failure ADR-0004 asks for.

One connection at a time, since there is one source. A second connection is refused
rather than queued: two sources feeding one conversion pipeline would interleave
their key state, and the pipeline's held-key reasoning assumes one physical set of
hands.

The calls are a host concern on both sides — the socket, and the cryptography that
needs the machine's entropy, live in `host-windows` and `host-macos`
([ADR-0006](0006-host-boundary.md)). Everything above is pure and lives in `core`, so
the end-to-end suite can drive a source through a simulated link into a sink without a
network.

The suite drives the link through a run of the converter, since that is what brings
it up: the simulated machine turns the loop it is handed, and what the link delivered
goes into the same stream the converter reads. Every way the handshake can stop early
is a case — a first message that never arrives, one that will not open, an answer that
cannot be sent — and so is a socket that cannot be bound and a loop that cannot be
turned. A run whose link never comes up still converts the keyboards in front of the
person.

A bound socket with nothing serving it would take the connection and never answer,
which from the other machine is worse than a machine that is not there — so the socket
goes with the loop. The advertisement goes with it too, since one that outlived the
socket would send the source to a port nothing is on.

Something scanning the port does not stop the link, and a socket that has stopped
answering does not spin: both are cases, and so is the count going back to zero on a
connection that worked. A run whose link ends exits with the code that says the
keyboards were given back, so the supervisor starts a run that binds the socket again
— about a second, and the local keyboards work throughout it except for that gap
([ADR-0012](0012-macos-install-as-a-daemon-and-turn-off-with-a-file.md) accepts the
same trade for the on/off switch).

The alternative — carrying on with the local keyboards and no link — is what the
suite would have to pin instead, and it is the worse behaviour: it is silent on both
machines, and the Windows keyboard stays dead until somebody notices and restarts
favjit by hand.

Which order the two loops on the sink interleave in is not explored, and cannot be
([ADR-0007](0007-deterministic-e2e.md)). What a case asks is what each loop did; the
timestamps on their events are what order them once one stream carries both.

`host-macos`'s link is calls only, and its own test drives them in the order `serve`
does, over loopback: that a read takes exactly the length the other end wrote is what
no simulated host can be wrong about, and what hangs when it is wrong. The Windows
host implements the same operations against the same constants, so the two ends cannot
disagree about them.

It adds a cryptography dependency. `snow` implements Noise in Rust with no async
runtime, which keeps it inside the shape ADR-0006 asks for: no concurrency in `core`,
and hosts that a single loop drives. `core` holds no cryptography and no entropy, so a
replayed trace still produces what the machine it came from produced
([ADR-0009](0009-trace-and-replay.md)).

## Alternatives considered

### One service name, with pairing requiring the converter to be switched off

The smaller change: pairing advertises what the link advertises, and refuses to run
while a converter is up so that only one thing answers for favjit. Not taken. It puts
a step on the person that has nothing to do with pairing — off, pair, on — at the one
moment they are working from the machine whose keyboard is about to be given back, and
it is a step whose reason nothing on screen explains. The refusal also cannot be
softened into a warning, because the failure it prevents is silent from the sink's end.
A second name costs one constant and removes the whole sequence.

### The cryptography in `core`, with the host supplying entropy

`snow` allows it: a custom `CryptoResolver` can provide the RNG. Not taken, and not
because of what `core` may depend on — the bar there is a crate widely enough used to
be worth trusting and free of a runtime of its own, which `snow` meets. It is that the
handshake is already drivable a step at a time through one-call operations, so moving
it would buy the suite nothing and cost every case in it a set of random bytes to
invent. Where that trade comes out the other way — an exchange the suite cannot drive
at all without the arithmetic — the arithmetic belongs in `core`.

### Noise's `KK` pattern

Both static keys known before the handshake, which hides the initiator's identity
from an observer as well as the responder's. **Not taken**: the responder has to
know *which* peer is connecting before the handshake starts, so a sink holding more
than one authorised key would have to attempt the handshake against each — and a
failed Noise handshake has consumed the connection, so each attempt needs the source
to connect again. ADR-0004's list makes the responder the one that decides, which is
`IK`.

### TLS with pinned self-signed certificates

The same guarantees, through a more familiar stack. **Not taken**: it reaches those
guarantees by pinning a certificate, which is a container for a key plus a name, a
validity period and a chain — none of which two machines at one desk have any use
for. Every one of those is another thing that can expire or mismatch, and the
identity ADR-0004 pins is the key itself.

### QUIC

Independent streams, so a lost packet holds up only its own. **Not taken**: it buys
head-of-line avoidance for a link that carries one ordered stream of key events, and
costs an async runtime in the host — where ADR-0006 wants a single loop.

### The source listening, the sink connecting

The sink knows where the source is and reconnects to it. **Not taken**: the sink is
the daemon that is always up, and it would be the one having to discover an address.
It also puts the machine that authorises in the position of asking to be given input,
which inverts ADR-0004's direction.

### UDP with a sequence number

Lower latency under loss, and input is a stream of small messages. **Not taken**:
key-down and key-up are not independent — a dropped release leaves a key held, which
is the failure the whole design is built to avoid. Recovering that means
retransmission and ordering, which is what TCP already is.
