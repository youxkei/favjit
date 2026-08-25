# ADR-0004: Authenticate the peer with pinned per-machine keypairs, paired once on the sink

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

The channel from the Windows source to the macOS sink ([ADR-0002](0002-input-topology.md)) carries keyboard and mouse events that the sink injects as ordinary input. Anything that can speak the protocol to the sink can type on the Mac and click on its behalf.

The threat model is not a hostile internet — the two machines sit on the same desk. But "same local network" is not the same as "trusted": office and shared Wi-Fi carry other people's machines, guest networks exist, and other devices on the same segment are not vouched for by anything. A local network is exactly the position an attacker would want for this.

What makes this different from a typical local-network service is the failure shape. A compromised file-sync channel leaks or corrupts data, which is bad but bounded and eventually visible. A compromised input channel is interactive control of a logged-in desktop session, and it looks like normal operation from the outside.

Constraints working in our favour:

- There are exactly two machines, both long-lived, both physically at hand. One-time setup is acceptable.
- No accounts, no server, no third-party service is wanted for a tool that fixes a keyboard layout.
- Sessions are long — one per working day, not one per event.

One property of short secrets decides how the keys get across, and it is worth stating because the two cases look alike and are not: a short **static** secret is weak, because anyone who records one exchange can try every value offline at their leisure. A short **single-use** secret, in a protocol that leaks nothing to an offline search, is not: the only way to test a guess is to make another attempt, and there is one guess per secret.

## Decision

Each machine generates a long-lived keypair on first run. The sink holds a list of authorized source public keys, and a key is added to it **only by an explicit action on the sink** — the machine being controlled decides who may control it. Every session is a mutually authenticated, encrypted channel keyed on those identities; a peer presenting an unpinned key is refused.

**The keys reach each other over a six-digit code the sink displays**, which the person enters on the source. Both ends run a password-authenticated key exchange over that code and, over the shared secret it produces, exchange their long-lived public keys and pin them. The code is single-use and spent on the first attempt whether it succeeded or failed; another attempt needs a code the sink has newly displayed. Nothing about a code survives pairing: every session after it is keyed on the pinned identities alone.

Refusal is the default. There is no mode in which the sink accepts input from an unknown peer, and no prompt on the source side that could authorize itself.

## Consequences

- **Fail closed.** An unconfigured or half-configured sink injects nothing. The failure mode of a mistake is "my keyboard doesn't work", which is loud, rather than "someone else's keyboard also works", which is silent.
- **This constrains the transport decision.** Whatever transport is chosen must carry mutual authentication and encryption keyed on these identities. Plain TCP or bare UDP with an application-level check is not sufficient — authentication that isn't bound to a session key leaves the stream open to injection and replay. In practice this narrows the transport choice to something that layers a real cryptographic session over it.
- Authorization is per-machine identity, not per-network-location, so it survives DHCP changes and doesn't grant anything to whoever happens to hold an address.
- Reinstalling either machine means re-pairing. With two machines that is a minute of work, and it is the correct behavior — a reinstalled machine is a new machine.
- Private key material now exists on both machines and has to be protected at rest.
- **An attacker positioned between the machines at pairing time is defeated rather than pinned in.** This is the residual risk of pinning on first contact, and the code is what removes it: it is not carried over the channel, so a machine in the middle has nothing to answer with. What is left is one guess per code, at one in a million.
- **Spending the code on a failed attempt is what holds that number**, so it is not an optimisation to skip. A code that survives failure is a code an attacker can walk through at whatever rate the sink accepts connections, and six digits stops being enough.
- **The long key is never something a person handles.** Sixty-four hex characters read aloud or retyped is where a mistake becomes likely and where the step gets skipped, and a pairing step people avoid protects nothing. Six digits on one screen, typed on the other, is a task that gets done.
- **A password-authenticated key exchange is a dependency neither machine would otherwise need**, and it is not substitutable by feeding the code into whatever pre-shared-key mode the session protocol offers: those keys are specified as high-entropy, and a handshake keyed on one is not password-hardened. Using the code that way would recreate the weak static secret this decision exists to avoid.
- **Pairing becomes a mode on each binary** — one that displays a code on the sink and one that takes it on the source — rather than a flag carrying a key.

## Alternatives considered

### A shared passphrase in a config file on both machines

The obvious minimum, and not taken. It has to be carried by hand into two places, which pushes it toward being short and reused. Worse, it identifies nothing: a leaked config file grants permanent typing rights, and the sink cannot tell one peer from another or revoke just one. On its own it also does nothing about replay, since knowledge of a static secret is not bound to a session.

### No authentication; trust the local network

Rejected. This is the remote-control vulnerability stated plainly. The desk being trusted does not make the network segment trusted, and the whole point of the threat model above is that this failure is invisible while it is happening.

### An IP address allowlist

Rejected as *the* mechanism. Addresses are trivially spoofable on a local segment and change under DHCP, so it would both fail to exclude an attacker and fail to admit the legitimate peer. It is acceptable only as an extra layer on top of the decision above, never instead of it.

### A pairing code entered each session

Rejected. The setup is two fixed machines used every day; per-session interaction is friction that buys nothing over pinning the identity once. It would also make the tool useless immediately after boot, which is when it is most wanted. A code entered *once* is a different proposition, and is the decision above.

### Carrying the public key by hand

Each machine prints its public key, and the person types it into the other. The simplest thing that satisfies the pinning requirement, and rejected: a key is thirty-two bytes, which is sixty-four characters however it is written down, and that is not a thing anybody does at a desk twice. The step that is tedious is the step that gets skipped or done wrong, and this one has no way to tell a typo from an attacker — both come out as a peer that will not connect.

### Using the code directly as the session's pre-shared key

The shortest change, and wrong. Pre-shared keys in session protocols are specified as high-entropy secrets, and a handshake keyed on one gives an eavesdropper a transcript to search offline. Six digits fall to that in moments. The single-use property is what makes a short code safe, and it only holds if the code is consumed by a protocol that yields nothing without an online attempt — which is what a password-authenticated key exchange is for and what a pre-shared key is not.

### Exchanging the keys over the channel and comparing a short string on both screens

The other well-known shape: the handshake carries both keys, each end derives a short string from it, and the person checks that the two screens agree. Rejected because comparing is a worse task than entering. It needs both screens read at once and can be passed by inattention, and that failure is silent — where a mistyped code fails by itself, loudly. Entering also puts the decision on the machine being controlled, which is where this ADR puts it.

### A code the source generates, entered on the sink

Symmetrical, and rejected for the reason the decision above gives: the machine being controlled decides who may control it. A code the source produced would make the source the party that initiates trust.

### A longer code — a passphrase, or words from a list

Would allow the code to be static rather than single-use, at the cost of the thing being fixed. Six digits is enough *because* it is single-use; length buys entropy that the one-guess rule already provides, and pays for it with the friction this decision removes.
