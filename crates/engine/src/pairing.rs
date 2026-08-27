//! Who this machine will take input from (ADR-0004).
//!
//! Here rather than in a host because none of it touches the machine: a key is
//! bytes, a list of them is text, and whether one is in the list is a question
//! with an answer. What the hosts do with the answer — reading the file, opening a
//! socket — is the part that cannot be tested away from the platform, and keeping
//! this out of there is what keeps that part small (ADR-0006).
//!
//! The code exchange's own arithmetic — which side starts as which, what the
//! exchange binds to, which direction each nonce seals, that what is sealed is the
//! static key — is `favjit_pairing_exchange`'s rather than this module's, because the
//! end-to-end suite's simulated peer has to run the same functions this module calls
//! and this crate cannot depend on its own test double.

use favjit_pairing_exchange::Side;
pub use favjit_pairing_exchange::{Code, DIGITS, OFFER, SEALED_KEY};

pub use favjit_host::pairing::{PairingHost, SourcePairingHost};
pub use favjit_host::{Entropy, IdentityStore};
pub use favjit_noise::{Identity, KEY};

/// How long each sink-side step of the exchange may take.
const SINK_STEP: core::time::Duration = core::time::Duration::from_secs(60);

/// How long each source-side step of the exchange may take.
const SOURCE_STEP: core::time::Duration = core::time::Duration::from_secs(10);

/// The keys this machine accepts input from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authorized {
    keys: Vec<Vec<u8>>,
}

impl Authorized {
    /// Read the list out of the file's text.
    ///
    /// A line that is not a key is skipped rather than failing the whole list:
    /// this is a file a person edits, and one stray character should leave the
    /// list short — which refuses a peer — rather than empty, which refuses
    /// everyone, or aborted, which leaves the converter with no list at all.
    pub(crate) fn parse(text: &str) -> Self {
        let keys = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(key_from_hex)
            .collect();
        Self { keys }
    }

    /// The text to store for this list with one more key in it.
    ///
    /// Text in, text out: what the file looked like before is kept as it was,
    /// comments and all, because a person put them there.
    pub(crate) fn added(text: &str, key: &[u8]) -> String {
        let mut out = String::from(text);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&hex(key));
        out.push('\n');
        out
    }

    pub(crate) fn holds(&self, key: &[u8]) -> bool {
        self.keys.iter().any(|pinned| pinned == key)
    }
}

/// Why this machine has no identity to present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoIdentity {
    /// There is a file, and it is not an identity.
    Foreign,
    CannotMake,
    /// It was made and could not be written down, with what the machine said
    /// about that.
    ///
    /// Carried rather than described where it happened: a permission and a
    /// full disk need different things done about them, and which of them it
    /// was is the machine's account of itself rather than a host's judgement
    /// (ADR-0006).
    CannotKeep(favjit_host::Trouble),
}

impl core::fmt::Display for NoIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Foreign => f.write_str("the identity file is not a keypair"),
            Self::CannotMake => f.write_str("cannot make a keypair"),
            Self::CannotKeep(trouble) => {
                write!(f, "cannot keep the keypair that was made: {}", trouble.0)
            }
        }
    }
}

/// The identity this machine presents, made on first use.
///
/// Made here rather than by an installer: a machine nobody has paired has nothing
/// to protect, and one that loses the file gets a new identity — which is the right
/// outcome, since a peer that pinned the old one should refuse the new.
///
/// A file that is not an identity is left as it is and refused. Replacing it would
/// throw away an identity a peer may have pinned, and what is actually in it is
/// either somebody else's or a write that did not finish.
///
/// A keypair that cannot be kept is refused rather than used, because the next run
/// would present a different one: a peer that pinned this one would then refuse a
/// machine that looks paired.
pub fn identity<H: IdentityStore + Entropy + ?Sized>(host: &mut H) -> Result<Identity, NoIdentity> {
    if let Some(bytes) = host.read() {
        return Identity::from_bytes(&bytes).ok_or(NoIdentity::Foreign);
    }
    let identity = crate::noise::keypair(host).ok_or(NoIdentity::CannotMake)?;
    // Stopped at the first of the three that would not, and carrying what the
    // machine said about it: the next step of a write that has already failed
    // is a call against a file that is not there.
    host.make_directory().map_err(NoIdentity::CannotKeep)?;
    let mut open = host.open().map_err(NoIdentity::CannotKeep)?;
    open.write(&identity.to_bytes())
        .map_err(NoIdentity::CannotKeep)?;
    Ok(identity)
}

/// Six digits, from bytes the machine supplied.
///
/// A byte over 249 is thrown away rather than taken modulo ten, because the remainder
/// would make the first six digits likelier than the last four — and six digits have
/// little enough to spend without giving any of it away.
fn code_from(entropy: &mut dyn Entropy) -> Option<Code> {
    let mut code = [0u8; DIGITS];
    let mut filled = 0;
    while filled < DIGITS {
        let mut byte = [0u8; 1];
        if !entropy.fill(&mut byte) {
            return None;
        }
        if byte[0] < 250 {
            code[filled] = b'0' + byte[0] % 10;
            filled += 1;
        }
    }
    Some(code)
}

/// How pairing ended.
///
/// One enum for both ends, because what a person is told is the same either way:
/// which machine could not be reached is the only thing that differs, and a name
/// per end says it without a second set of the four they share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paired {
    /// The peer's key is pinned, as this fingerprint.
    Pinned(String),
    /// What arrived would not open under the code, which is what a wrong code looks
    /// like from either end.
    WrongCode,
    /// Nobody answered the code this machine showed.
    NoSource,
    /// The machine showing the code could not be reached.
    NoSink,
    /// This machine could not open its pairing listener.
    CannotListen,
    /// The machine could not produce a code.
    NoCode,
    /// The exchange stopped part way — the connection went, or the machine could
    /// not do its half.
    Interrupted,
    /// The key opened and could not be written down, with what the machine
    /// said about that.
    ///
    /// Carried for the reason [`NoIdentity::CannotKeep`] carries one: a
    /// pairing a person has to redo is one they have to fix something for
    /// first, and which thing is the machine's own account of itself
    /// (ADR-0006).
    CannotKeep(favjit_host::Trouble),
}

/// One pairing: show a code, serve one attempt, pin what it proves.
///
/// **One attempt, and then the run is over** — which is what makes six digits enough
/// (ADR-0004). Serving a second attempt against a shown code would let an attacker
/// try again at whatever rate this machine accepts connections, and no length of code
/// survives that.
///
/// The code is shown before anything is waited for, because a code produced after a
/// source has connected is a code nobody could have entered.
///
/// Both keys cross. This machine pins the source's so its converting run will accept
/// input, and the source is given this machine's because it is the end that opens the
/// session and cannot address a machine whose key it does not hold (ADR-0012).
pub fn pair(mine: &Identity, host: &mut dyn PairingHost) -> Paired {
    let Some(mut listening) = host.bind_listener() else {
        return Paired::CannotListen;
    };
    let Some(port) = listening.port() else {
        return Paired::CannotListen;
    };
    // The listener remains useful when Bonjour does not: the source can be handed
    // its address explicitly, which is why registration failure does not spend the
    // code or stop the attempt.
    host.advertise(crate::link::PAIRING, port);

    let Some(code) = code_from(host) else {
        return Paired::NoCode;
    };
    let digits = core::str::from_utf8(&code).unwrap_or("??????");
    host.show(&format!("pairing code: {digits}"));
    host.show("type it on the other machine within a minute: favjit --pair <those digits>");
    if !listening.set_blocking() {
        return Paired::NoSource;
    }
    let Some(mut open) = listening.accept() else {
        return Paired::NoSource;
    };
    if !open.set_read_timeout(SINK_STEP) || !open.set_write_timeout(SINK_STEP) {
        return Paired::Interrupted;
    }

    let mut offer = [0u8; OFFER];
    if !open.take_offer(&mut offer) {
        return Paired::Interrupted;
    }
    // Answered against the connection's own entropy, which is this machine's:
    // what the exchange needs is bytes nothing can predict, and the connection is
    // what the run holds while it is running one (ADR-0012).
    let Some((answer, secret)) = favjit_pairing_exchange::answer(code, &offer, open.as_mut())
    else {
        return Paired::Interrupted;
    };
    if !open.send_answer(&answer) || !open.flush_answer() {
        return Paired::Interrupted;
    }

    let mut sealed = [0u8; SEALED_KEY];
    if !open.take_sealed_key(&mut sealed) {
        return Paired::Interrupted;
    }
    // Before anything of this machine's is sent back: a source that had the code
    // wrong learns nothing from a pairing attempt it could not complete.
    let Some(peer) = secret.open(&sealed, Side::Source) else {
        return Paired::WrongCode;
    };
    let Some(sealed_mine) = secret.seal(mine.public(), Side::Sink) else {
        return Paired::Interrupted;
    };
    if !open.send_sealed_key(&sealed_mine) || !open.flush_sealed_key() {
        return Paired::Interrupted;
    }

    // Last, because pinning is what the exchange was for: a key written down before
    // it was opened under the code would be a key the code did not vouch for.
    let current = host.authorized().unwrap_or_default();
    if let Err(trouble) = host.make_authorized_directory() {
        return Paired::CannotKeep(trouble);
    }
    match host.authorize(&Authorized::added(&current, &peer)) {
        Ok(()) => Paired::Pinned(hex(&peer)),
        Err(trouble) => Paired::CannotKeep(trouble),
    }
}

/// One pairing from this end: send the code's half, pin what comes back.
///
/// This end speaks first, because it is the end that connects — so the offer goes
/// out before an answer is waited for, and this machine's key goes out before the
/// sink's arrives. That ordering is the other end's read in reverse ([`pair`]).
///
/// Sending this machine's key before the sink's has opened is safe in a way the
/// other direction is not: the sink has shown the code to whoever is at the desk,
/// and a machine that answered without knowing it learns only a public key it could
/// have asked for over the session anyway. The sink withholds its own until it has
/// opened this one, which is the half that has something to withhold.
pub fn pair_with(code: Code, mine: &Identity, host: &mut dyn SourcePairingHost) -> Paired {
    let Some(mut open) = host.connect(SOURCE_STEP) else {
        return Paired::NoSink;
    };
    if !open.set_read_timeout(SOURCE_STEP) || !open.set_write_timeout(SOURCE_STEP) {
        return Paired::Interrupted;
    }
    // Offered against the connection's own entropy, for the reason the other end
    // answers against its (ADR-0012).
    let Some((started, offer)) = favjit_pairing_exchange::offer(code, open.as_mut()) else {
        return Paired::Interrupted;
    };
    if !open.send_offer(&offer) || !open.flush_offer() {
        return Paired::Interrupted;
    }
    let mut answer = [0u8; OFFER];
    if !open.take_answer(&mut answer) {
        return Paired::Interrupted;
    }
    let Some(secret) = started.finish(&answer) else {
        return Paired::Interrupted;
    };

    let Some(sealed_mine) = secret.seal(mine.public(), Side::Source) else {
        return Paired::Interrupted;
    };
    if !open.send_sealed_key(&sealed_mine) || !open.flush_sealed_key() {
        return Paired::Interrupted;
    }
    let mut sealed = [0u8; SEALED_KEY];
    if !open.take_sealed_key(&mut sealed) {
        return Paired::Interrupted;
    }
    let Some(sink) = secret.open(&sealed, Side::Sink) else {
        return Paired::WrongCode;
    };

    // Last, for the reason it is last at the other end: a key written down before it
    // was opened under the code is a key the code did not vouch for.
    if let Err(trouble) = host.make_sink_directory() {
        return Paired::CannotKeep(trouble);
    }
    match host.pin_sink(&sink_text(&sink)) {
        Ok(()) => Paired::Pinned(hex(&sink)),
        Err(trouble) => Paired::CannotKeep(trouble),
    }
}

/// The whole text of the file naming the one machine a relaying run sends input
/// to.
///
/// Replaced rather than added to, unlike the sink's own list: a source has
/// exactly one machine it will hand its keyboard to (ADR-0004), and a second
/// key in the file would make which one that is depend on the order they were
/// written in.
fn sink_text(key: &[u8]) -> String {
    format!("{}\n", hex(key))
}

/// The one machine a relaying run sends input to, out of the text of the file
/// naming it.
///
/// The first key the file holds, read exactly the way [`Authorized::parse`]
/// reads the sink's own list: the two files are written by the two ends of the
/// same pairing, so a key pasted between them has to be the same key on both,
/// and one reader is what makes that so rather than two that agree today.
///
/// `None` where the text names none — a file a person has emptied, or one with
/// nothing in it but comments — which is a machine with nowhere to send input
/// rather than an error to report.
pub(crate) fn pinned_sink(text: &str) -> Option<Vec<u8>> {
    Authorized::parse(text).keys.into_iter().next()
}

/// The digits a key is written down as, for saying which machine is which.
///
/// The same digits the person reads off the other machine's screen, so what
/// [`crate::source::identify`] prints and what a pairing wrote are the one
/// function rather than two beside each other.
pub(crate) fn written_as(key: &[u8]) -> String {
    hex(key)
}

/// A key as a person pastes it, or nothing if it is not one.
pub(crate) fn key_from_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len() / 2)
        .map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect();
    bytes.filter(|bytes| bytes.len() == KEY)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // The write cannot fail: the target is a string with the room already
        // reserved, so there is nothing for the result to report.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(fill: u8) -> Vec<u8> {
        vec![fill; KEY]
    }

    #[test]
    fn a_key_survives_being_read_out_and_typed_back_in() {
        let key = key(0xab);
        assert_eq!(key_from_hex(&hex(&key)), Some(key));
    }

    #[test]
    fn the_file_a_pairing_wrote_names_the_sink_it_paired_with() {
        // The two halves of the same file, against each other: what a source
        // writes down is what a later run of it reads, and a format written
        // twice is a machine that pairs and then cannot find its sink.
        let sink = key(0x5a);
        assert_eq!(pinned_sink(&sink_text(&sink)), Some(sink));
    }

    #[test]
    fn a_file_with_no_key_in_it_pins_nothing() {
        // What a half-finished setup looks like: the file exists because a
        // person made it, and nothing in it is a key. Nothing is the answer that
        // keeps the source from connecting, which is the direction ADR-0004
        // wants this to fail in.
        assert_eq!(pinned_sink(""), None);
        assert_eq!(pinned_sink("# the mac goes here\n\n"), None);
        // Valid hex of the wrong length is the same answer: a sink nothing can
        // present is one no handshake would ever open.
        assert_eq!(pinned_sink("00ff\n"), None);
    }

    #[test]
    fn the_first_key_in_the_file_is_the_sink() {
        // A source relays to one machine (ADR-0004), so a second key is not a
        // second sink to choose between — reading them as a list would make
        // which one is in use depend on the order somebody wrote them in.
        let first = key(0x11);
        let text = format!("{}\n{}\n", hex(&first), hex(&key(0x22)));
        assert_eq!(pinned_sink(&text), Some(first));
    }

    #[test]
    fn only_something_the_right_length_is_a_key() {
        // A short string of valid hex would be pinned as an identity nothing can
        // present: the list would look configured and refuse every source.
        assert_eq!(key_from_hex("00ff"), None);
        assert_eq!(key_from_hex("zz"), None);
        assert_eq!(key_from_hex("abc"), None);
    }

    #[test]
    fn an_empty_list_authorises_nobody() {
        let authorized = Authorized::parse("");
        assert!(!authorized.holds(&key(1)));
    }

    #[test]
    fn comments_and_blank_lines_are_not_keys() {
        let text = format!("# a note\n\n{}\n", hex(&key(7)));
        let authorized = Authorized::parse(&text);

        assert!(authorized.holds(&key(7)));
        assert!(!authorized.holds(&key(8)));
    }

    #[test]
    fn a_line_that_is_not_a_key_leaves_the_list_short_rather_than_empty() {
        let text = format!("not a key\n{}\n", hex(&key(3)));
        let authorized = Authorized::parse(&text);

        assert!(authorized.holds(&key(3)));
    }

    #[test]
    fn adding_a_key_keeps_what_was_there() {
        let before = "# the Windows machine\n";
        let after = Authorized::added(before, &key(2));

        assert!(after.starts_with(before));
        assert!(Authorized::parse(&after).holds(&key(2)));
    }

    #[test]
    fn adding_to_a_file_with_no_final_newline_does_not_join_two_keys() {
        // A file a person edited by hand may end mid-line, and a key glued to the
        // end of another is neither of them.
        let before = hex(&key(4));
        let after = Authorized::added(&before, &key(5));
        let authorized = Authorized::parse(&after);

        assert!(authorized.holds(&key(4)));
        assert!(authorized.holds(&key(5)));
    }

    #[test]
    fn an_identity_is_both_halves_or_nothing() {
        let identity = Identity::new(key(1), key(2)).expect("two keys of the right length");
        assert_eq!(Identity::from_bytes(&identity.to_bytes()), Some(identity));

        assert_eq!(Identity::from_bytes(&key(1)), None);
        assert_eq!(Identity::from_bytes(&[]), None);
        assert!(Identity::new(key(1), vec![2, 2]).is_none());
    }

    #[test]
    fn a_fingerprint_is_the_public_half() {
        let identity = Identity::new(key(1), key(0xfe)).expect("two keys");
        assert_eq!(identity.fingerprint(), hex(&key(0xfe)));
        assert_ne!(identity.fingerprint(), hex(&key(1)));
    }
}
