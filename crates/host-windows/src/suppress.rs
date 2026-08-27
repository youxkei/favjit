//! Taking this machine's input away from it, and giving it back.
//!
//! A low-level hook is the only Windows input hook that can refuse an event rather than
//! merely watch it: returning non-zero from one ends the event, and nothing downstream
//! sees it. **Raw input is downstream too**, which is the fact this module is shaped
//! around — a key the hook refuses reaches nothing at all, favjit included
//! ([docs/platform/windows/hooks-and-raw-input.md](../../../docs/platform/windows/hooks-and-raw-input.md)).
//!
//! So **the keyboard hook is where keys are read from**, and not only where they are
//! refused: it reports the key first and turns it down second, which is the only order
//! in which both can happen. Deskflow's Windows hook is the same shape, and its
//! `RegisterRawInputDevices` count is zero.
//!
//! What that costs is the device: `KBDLLHOOKSTRUCT` says which key and not which
//! keyboard, so every key this machine forwards arrives as one unnamed external
//! keyboard. ADR-0003 allows for it — "where that information is unavailable, the
//! single pipeline still stands but per-device rules are not expressible" — and
//! `--ansi` is already a property of the machine rather than of a keyboard.
//!
//! The pointer is the other way round: a hook that refuses a pointer event leaves raw
//! input arriving, so the mouse is captured there, where its movement is a movement
//! rather than a cursor position (ADR-0011).
//!
//! **What suppresses is one number**, and the procedures read it per event. An ordinary
//! exit gives the input back by dropping the host, and nothing else has to run for that.
//!
//! **A wedge is not covered.** ADR-0008 requires a watchdog wherever suppression is
//! held, and the one favjit has supervises through POSIX pipes, so nothing supervises
//! this side: a relaying run says so and that is the whole of it. Keeping each procedure
//! to a couple of atomic reads and one post makes a wedge *in them* unreachable, which
//! is not the same as covering a wedge in the loop they are refusing input for.

use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use favjit_host::source::{Answering, Answers, Arrived, Suppressing};

use crate::ffi::*;

/// What is being refused as things stand, as [`Suppressing`] numbered.
///
/// A static because a hook procedure is a bare function pointer with nowhere to hang a
/// context off: Windows calls it with the event and nothing else. A number rather than
/// the enum, because that is what an atomic holds.
static REFUSING: AtomicU8 = AtomicU8::new(NOTHING);

const NOTHING: u8 = 0;
const THE_SWITCH: u8 = 1;
const EVERYTHING: u8 = 2;

/// The positions the chord is made of, as a hook reports them: the make code in
/// the low half and the extended bit in the next.
///
/// Stored once, while the hooks are installed, from what [`Hooks::install`] is
/// handed — so the procedure compares two numbers rather than reading anything.
/// Whole positions and not make codes alone, because the prefix is part of which
/// key a number is: a comparison that ignored it would refuse the arrow-cluster
/// key that shares a number with one of the chord's.
static TO_THE_SINK: AtomicU32 = AtomicU32::new(NO_POSITION);
static BACK_HERE: AtomicU32 = AtomicU32::new(NO_POSITION);

/// What stands for a key this keyboard has no position for, which matches
/// nothing.
///
/// Outside the range a packed position can take, rather than a make code of
/// zero: zero packs to zero and so does an absent position, and an event with no
/// scan code would then be the chord.
const NO_POSITION: u32 = u32::MAX;

/// One position packed into the number an atomic holds.
fn packed(position: Option<(u16, bool)>) -> u32 {
    match position {
        Some((code, extended)) => u32::from(code) | (u32::from(extended) << 16),
        None => NO_POSITION,
    }
}

/// What the procedures refuse, as the number they read it out of.
///
/// `Suppressing` is `#[non_exhaustive]`: a kind this crate does not yet name
/// refuses nothing, the same as `Nothing`, until it is given a meaning of its
/// own.
fn refusing(what: Suppressing) -> u8 {
    match what {
        Suppressing::Nothing => NOTHING,
        Suppressing::TheSwitch => THE_SWITCH,
        Suppressing::Everything => EVERYTHING,
        _ => NOTHING,
    }
}

/// The bits the run keeps here for what this machine was let see go down and
/// not yet go up, as many as `HELD_HERE_BITS` names, in words.
///
/// Atomics and not a lock, because a procedure runs on the thread the event
/// arrived on and must return before the next one: a lock taken there would be
/// a wedge inside the one place ADR-0008 cannot have one. Which bit a key is,
/// and whether it has one, is the run's: what is stored here is the number it
/// chose, in the word and bit that number spells.
static HELD_HERE: [AtomicU64; favjit_host::source::HELD_HERE_BITS / 64] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// The word and bit one of the run's numbers spells.
fn held_word(bit: usize) -> (&'static AtomicU64, u64) {
    (&HELD_HERE[bit >> 6], 1 << (bit & 0x3F))
}

/// The window keys are handed to.
static KEYS_GO_TO: AtomicUsize = AtomicUsize::new(0);

/// How many pointer events have been refused.
///
/// Counted because a run that reports none of them while it was relaying is one whose
/// mouse reached neither machine, and the number is the only thing that says which. No
/// count for keys: what refuses them is what reports them, so "refused but not
/// captured" is not a state a key can be in.
static POINTERS: AtomicUsize = AtomicUsize::new(0);

/// Where keys are handed to, which is the thread that installed the hooks.
pub fn keys_go_to(window: Handle) {
    KEYS_GO_TO.store(window as usize, Ordering::SeqCst);
}

/// Refuse this machine's own input, or stop refusing it.
///
/// The number alone. What the tray item draws is [`draw`]'s, and setting the two
/// together is the run's: they are one state seen from two places
/// (`docs/platform/windows/tray-item-as-its-own-program.md`), and which of them
/// happens when is not a thing this file decides (ADR-0006).
pub fn take(what: Suppressing) {
    REFUSING.store(refusing(what), Ordering::SeqCst);
}

/// Whether there is a window for the tray item to read a state off yet.
///
/// There is not before the first event, which is the state a run comes up in.
pub fn there_is_a_window() -> bool {
    KEYS_GO_TO.load(Ordering::SeqCst) != 0
}

/// Put what is refused where the tray item reads it.
pub fn draw(what: Suppressing) {
    crate::tray::say(KEYS_GO_TO.load(Ordering::SeqCst) as Handle, what);
}

/// How many pointer events have been refused.
pub fn refused() -> usize {
    POINTERS.load(Ordering::SeqCst)
}

/// One hook, taken off again when this goes.
///
/// Its own type rather than a pair in one value, because taking one off is one
/// call and taking the other off is another: a `Drop` doing both would be a host
/// sequencing two calls into the machine (ADR-0006), and there is nothing to
/// decide about the order.
///
/// Dropping is not what ends suppression — [`take`]'s number is — because these
/// only go out of scope on the paths that were going to end anyway, and the paths
/// that matter are the ones where no code of ours runs at all. With the hooks gone
/// nothing reads that number either, so nothing here has to put it back.
pub struct Hooked(Handle);

impl Drop for Hooked {
    fn drop(&mut self) {
        unsafe { UnhookWindowsHookEx(self.0) };
    }
}

/// Where the chord's two keys are, for the run's answer to compare against.
///
/// `None` for a key this keyboard has no position for, which matches nothing — so
/// the chord does not move the keyboard, rather than a wrong position doing it.
/// Which keys they are, and finding and spelling their positions, is `engine`'s
/// and `favjit-hid`'s to have settled before this (ADR-0006).
pub fn the_chord_is_at(switch_to_the_sink: Option<(u16, bool)>, switch_back: Option<(u16, bool)>) {
    TO_THE_SINK.store(packed(switch_to_the_sink), Ordering::SeqCst);
    BACK_HERE.store(packed(switch_back), Ordering::SeqCst);
}

/// Put the keyboard's procedure on this thread.
///
/// The thread that installs a low-level hook is the thread its procedure is called
/// on, so this has to be the thread with the message loop — a hook installed on a
/// thread that never pumps messages is never called, and the symptom is input that
/// is neither captured nor suppressed with nothing logged anywhere.
pub fn hook_the_keyboard() -> Option<Hooked> {
    installed(unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, keyboard_hook, null_mut(), 0) })
}

/// Put the pointer's procedure on the same thread.
pub fn hook_the_pointer() -> Option<Hooked> {
    installed(unsafe { SetWindowsHookExW(WH_MOUSE_LL, mouse_hook, null_mut(), 0) })
}

/// The hook a call answered with, where it installed one.
fn installed(answered: Handle) -> Option<Hooked> {
    (!answered.is_null()).then_some(Hooked(answered))
}

/// Hand what arrived to the run, and answer the platform with what it says.
///
/// The whole of the answer is the run's: whether the key is handed over, whether
/// it is refused, and in which order, are decided above this boundary and reached
/// through a function pointer (ADR-0006). What is here is the event read as what
/// it is, and the calls the answer is made of.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: Wparam, lparam: Lparam) -> Lresult {
    answering()(
        a_key(code, lparam),
        &Called {
            code,
            wparam,
            lparam,
        },
    )
}

/// The same for a pointer event.
///
/// The pointer is not read from here: a hook that refuses one leaves raw input
/// arriving, and raw input is where a movement is still a movement.
unsafe extern "system" fn mouse_hook(code: i32, wparam: Wparam, lparam: Lparam) -> Lresult {
    answering()(
        a_pointer(code, lparam),
        &Called {
            code,
            wparam,
            lparam,
        },
    )
}

/// What the run answers a procedure with, as the pointer to it.
///
/// A static because a procedure the platform calls is a bare function pointer
/// with nowhere to hang a context off: Windows calls it with the event and
/// nothing else. Started as [`nobody_answers`] rather than as null, so that
/// reading it is not a question about whether a run has installed one — a null
/// there could not be called at all, and asking would be a turning beside the
/// call this procedure is (ADR-0006).
static THE_RUN: AtomicPtr<()> = AtomicPtr::new(nobody_answers as *mut ());

/// Let the event carry on, for a call arriving before a run installed an answer.
///
/// Nothing reads this in a run: the procedures are installed after the answer
/// is, and the platform calls a procedure only for a hook that exists.
fn nobody_answers(_arrived: Arrived, answers: &dyn Answers) -> isize {
    answers.let_it_carry_on()
}

/// What the run answers with.
fn answering() -> Answering {
    // Sound because [`answer_with`] is the only thing that stores here, and what
    // it stores is a function of this type.
    unsafe { core::mem::transmute::<*mut (), Answering>(THE_RUN.load(Ordering::SeqCst)) }
}

/// Answer the procedures with this from now on.
pub fn answer_with(answering: Answering) {
    THE_RUN.store(answering as *mut (), Ordering::SeqCst);
}

/// One call into a procedure, and the calls its answer can be made of.
///
/// The three arguments the platform called with are held for the length of the
/// call, because letting the event carry on is a call that takes them back and
/// they are the platform's own values (ADR-0006).
struct Called {
    code: i32,
    wparam: Wparam,
    lparam: Lparam,
}

impl Answers for Called {
    fn refusing(&self) -> Suppressing {
        what_is_refused(REFUSING.load(Ordering::SeqCst))
    }

    fn the_chord(&self) -> favjit_host::source::Chord {
        (
            unpacked(TO_THE_SINK.load(Ordering::SeqCst)),
            unpacked(BACK_HERE.load(Ordering::SeqCst)),
        )
    }

    fn keys_go_somewhere(&self) -> bool {
        KEYS_GO_TO.load(Ordering::SeqCst) != 0
    }

    fn hand_the_key_over(&self, packed: usize, flags: i64) -> bool {
        let window = KEYS_GO_TO.load(Ordering::SeqCst) as Handle;
        went(unsafe { PostMessageW(window, WM_KEY, packed, flags as Lparam) })
    }

    /// The high bit is what `GetAsyncKeyState` sets for a key that is down; the
    /// low one says it has been pressed since the last ask, which is a different
    /// question and not this one.
    fn the_modifier_is_down(&self) -> bool {
        unsafe { GetAsyncKeyState(VK_MENU) as u16 & 0x8000 != 0 }
    }

    fn held_here(&self, bit: usize) -> bool {
        let (word, mask) = held_word(bit);
        word.load(Ordering::SeqCst) & mask != 0
    }

    fn now_held_here(&self, bit: usize) {
        let (word, mask) = held_word(bit);
        word.fetch_or(mask, Ordering::SeqCst);
    }

    fn let_go_of_here(&self, bit: usize) {
        let (word, mask) = held_word(bit);
        word.fetch_and(!mask, Ordering::SeqCst);
    }

    fn one_pointer_refused(&self) {
        POINTERS.fetch_add(1, Ordering::SeqCst);
    }

    fn let_it_carry_on(&self) -> isize {
        unsafe { CallNextHookEx(null_mut(), self.code, self.wparam, self.lparam) }
    }

    /// Non-zero, which is what ends the event on this platform.
    fn end_it_here(&self) -> isize {
        1
    }
}

/// Whether a post went, as the platform's own answer says.
fn went(posted: i32) -> bool {
    posted != 0
}

/// What one number stands for, as the run published it.
fn what_is_refused(number: u8) -> Suppressing {
    match number {
        THE_SWITCH => Suppressing::TheSwitch,
        EVERYTHING => Suppressing::Everything,
        _ => Suppressing::Nothing,
    }
}

/// One position out of the number an atomic holds, and nothing for the value
/// that stands for a key this keyboard has no position for.
fn unpacked(number: u32) -> Option<(u16, bool)> {
    match number {
        NO_POSITION => None,
        packed => Some((packed as u16, packed & (1 << 16) != 0)),
    }
}

/// One key event read as what it is, and nothing of ours for a call that
/// carries none.
///
/// The make code and the vkey go over in one word and the flags in the other,
/// which is what the capture loop's message carries.
unsafe fn a_key(code: i32, lparam: Lparam) -> Arrived {
    if code != HC_ACTION {
        return Arrived::NothingOfOurs;
    }
    let event = &*(lparam as *const KeyboardHookEvent);
    if event.flags & LLKHF_INJECTED != 0 {
        return Arrived::NothingOfOurs;
    }
    Arrived::Key {
        at: (event.make_code as u16, event.flags & LLKHF_EXTENDED != 0),
        packed: (event.make_code as usize & 0xFFFF) | ((event.vkey as usize & 0xFFFF) << 16),
        flags: event.flags as i64,
    }
}

/// One pointer event read the same way.
///
/// The pointer is dereferenced only where the code says there is an event behind
/// it, so a call carrying none never reads it.
unsafe fn a_pointer(code: i32, lparam: Lparam) -> Arrived {
    if code != HC_ACTION {
        return Arrived::NothingOfOurs;
    }
    let event = &*(lparam as *const MouseHookEvent);
    match event.flags & LLMHF_INJECTED != 0 {
        true => Arrived::NothingOfOurs,
        false => Arrived::Pointer,
    }
}
