//! `favjit-tray` — the item that says where the keyboard is, and moves it.
//!
//! Two things the chord cannot do. It says whether favjit is running at all and whether
//! it has found the Mac, which until now was a question only the log answered; and it
//! moves the keyboard with the pointer, for whoever would rather click than remember two
//! chords.
//!
//! **It is not an escape from a favjit that is refusing wrongly**, which is what the
//! Mac's menu bar item is (`docs/platform/macos/menu-bar-item-as-its-own-agent.md`). The difference is that this machine's favjit
//! refuses the pointer as well as the keys, so while the keyboard is the Mac's there is
//! nothing here to click. What covers that state is the watchdog (ADR-0008), and this
//! item is honest about which of the two is doing the covering: the state it draws while
//! the pointer is frozen is the reason the pointer is frozen.
//!
//! It holds no state of its own. What it draws is what the converter published on its
//! window, read through the same `host-windows` code the converter writes it with, so the
//! icon and the refusing cannot disagree (`docs/platform/windows/tray-item-as-its-own-program.md`).

#![cfg(windows)]
// No console, ever. A tray item started at logon by a console program is a black window
// sitting on the desktop for the whole session, and there is nothing for a console to
// carry here: what this program has to say, it says by being an icon. The log below is
// for the case where it cannot be one.
#![windows_subsystem = "windows"]

use std::fs::OpenOptions;
use std::time::Duration;

use favjit_host::source::{Driving, Suppressing};
use favjit_host_windows::{link, tray};
use log::{error, info};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// How often the item looks at what favjit is refusing.
///
/// The state changes from the keyboard — a chord — as well as from here, and it changes
/// on its own when the link comes up or goes away. Polling is what keeps the icon about
/// the machine rather than about the last click made here.
const POLL: Duration = Duration::from_millis(500);

/// Where the keyboard is, as this item draws it.
///
/// Four and not three: a favjit that is not running is a state a person needs to be able
/// to see, and it is the one the other three cannot be distinguished from without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    /// Nothing is forwarding.
    NotRunning,
    /// This machine's, and there is no link — the Mac is off, or not on this network.
    NoLink,
    /// This machine's, with a link to send it over.
    Here,
    /// The Mac's. Nothing typed or pointed here reaches this machine.
    TheMacs,
}

impl Where {
    fn read() -> Self {
        // Two questions, because they have two different answers: whether anything is
        // forwarding at all, and what it is refusing if so.
        match tray::running().and_then(tray::refusing) {
            None => Self::NotRunning,
            Some(Suppressing::Nothing) => Self::NoLink,
            Some(Suppressing::TheSwitch) => Self::Here,
            Some(Suppressing::Everything) => Self::TheMacs,
            // `Suppressing` is `#[non_exhaustive]`: a state this program does not yet
            // draw is drawn as the keyboard being this machine's, which is the reading
            // that sends nobody to the wrong machine.
            Some(_) => Self::Here,
        }
    }

    /// The line that says what is going on.
    fn label(self) -> &'static str {
        match self {
            Self::NotRunning => "favjit is not running",
            Self::NoLink => "favjit: no link to the Mac",
            Self::Here => "favjit: the keyboard is this machine's",
            Self::TheMacs => "favjit: the keyboard is the Mac's",
        }
    }

    /// The line you press to change it.
    fn move_label(self) -> &'static str {
        match self {
            Self::TheMacs => "Bring the keyboard back",
            _ => "Send the keyboard to the Mac",
        }
    }

    /// What pressing it asks for.
    fn asks_for(self) -> Driving {
        match self {
            Self::TheMacs => Driving::ThisMachine,
            _ => Driving::TheSink,
        }
    }

    /// Whether pressing it could do anything.
    ///
    /// Nothing to ask when nothing is running, and nothing to ask for when the keyboard
    /// is this machine's with no link to send it over: the run is looking for the Mac
    /// already, and it goes over by itself when it is found.
    fn worth_pressing(self) -> bool {
        matches!(self, Self::Here | Self::TheMacs)
    }
}

/// What wakes the loop up.
enum Wake {
    Poll,
    Menu(MenuEvent),
}

/// The icon, drawn rather than shipped.
///
/// A square: hollow while the keyboard is this machine's, filled while it is the Mac's,
/// and hollow in grey when there is nothing running. An image file would be a second
/// thing to install and keep beside the binary, for a few hundred pixels.
///
/// Its own colour rather than the system's: Windows draws a tray icon as given, with no
/// equivalent of the Mac's template inking, and the taskbar it lands on can be either
/// light or dark. A blue that is legible on both is what that costs.
fn icon(where_it_is: Where) -> Option<Icon> {
    const SIDE: u32 = 16;
    const LIVE: [u8; 3] = [0x2E, 0x8B, 0xE5];
    const DEAD: [u8; 3] = [0x88, 0x88, 0x88];

    let colour = if where_it_is == Where::NotRunning {
        DEAD
    } else {
        LIVE
    };
    let filled = where_it_is == Where::TheMacs;

    let mut rgba = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let edge = x < 2 || y < 2 || x >= SIDE - 2 || y >= SIDE - 2;
            let ink = filled || edge;
            rgba.extend_from_slice(&[colour[0], colour[1], colour[2], if ink { 255 } else { 0 }]);
        }
    }
    Icon::from_rgba(rgba, SIDE, SIDE)
        .map_err(|error| error!("cannot draw the icon: {error}"))
        .ok()
}

/// Start logging into a file beside the identity.
///
/// A window-subsystem program has no console to write to, and the two failures worth
/// hearing about are the ones where there is no icon to look at instead: the notification
/// area refusing the item, and the icon refusing to be drawn. Without this they would be
/// a tray item that silently is not there.
///
/// Appended to rather than replaced, because what is wanted is the last few logons and
/// not the last one. Beside the identity because that directory is favjit's on this
/// machine already, and it belongs to the person whose session this is.
fn log_to_a_file() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    let path = link::favjit_directory().join("tray.log");
    let _ = std::fs::create_dir_all(link::favjit_directory());
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    // Unwrapped deliberately: stderr is where it goes when the file cannot be opened,
    // which is nowhere in this subsystem, and a tray item that will not start over a log
    // it cannot write would be the wrong way round.
    builder.init();
}

fn main() {
    log_to_a_file();

    let event_loop = EventLoopBuilder::<Wake>::with_user_event().build();

    MenuEvent::set_event_handler(Some({
        let proxy = event_loop.create_proxy();
        move |event| {
            let _ = proxy.send_event(Wake::Menu(event));
        }
    }));
    std::thread::spawn({
        let proxy = event_loop.create_proxy();
        move || {
            // Until the send fails, which is how this thread learns the loop is gone.
            while proxy.send_event(Wake::Poll).is_ok() {
                std::thread::sleep(POLL);
            }
        }
    });

    // Both lines say the state as things stand, because the polling below only rewrites
    // them when it changes: an item built with a placeholder would keep it until
    // something moved.
    let mut shown = Where::read();
    let state = MenuItem::new(shown.label(), false, None);
    let move_it = MenuItem::new(shown.move_label(), shown.worth_pressing(), None);
    // **Quit stops the forwarding and then this**, in that order and as one item, because
    // that is what a person reaching for it wants: not to stop being told about favjit
    // but to have their keyboard back for good. An item that only closed itself would
    // leave the thing it exists to control still running with no way to reach it.
    //
    // What comes back afterwards is the next logon, or `schtasks /run /tn favjit`. The
    // item does not offer to start one: a program that supervises nothing should not be
    // the thing that starts the supervisor (`docs/platform/windows/tray-item-as-its-own-program.md`).
    let quit = MenuItem::new("Quit favjit", true, None);
    let menu = Menu::new();
    if let Err(error) = menu.append_items(&[
        &state,
        &PredefinedMenuItem::separator(),
        &move_it,
        &PredefinedMenuItem::separator(),
        &quit,
    ]) {
        error!("cannot build the menu: {error}");
        std::process::exit(1);
    }

    let move_id = move_it.id().clone();
    let quit_id = quit.id().clone();
    // Built at `Init` rather than here, because the item needs the loop that owns its
    // window to be running.
    let mut tray_icon: Option<TrayIcon> = None;
    let mut menu = Some(menu);

    event_loop.run(move |event, _target, flow| {
        // Wait, not a deadline: the poll arrives as an event of its own, so a timeout
        // here would only add wake-ups with nothing to do.
        *flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                let menu = menu.take().expect("Init happens once");
                shown = Where::read();
                match TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_tooltip(shown.label())
                    .with_icon(icon(shown).unwrap_or_else(|| {
                        error!("cannot draw the icon, so there would be nothing to click");
                        std::process::exit(1)
                    }))
                    .build()
                {
                    Ok(built) => tray_icon = Some(built),
                    Err(error) => {
                        error!("cannot put an item in the notification area: {error}");
                        std::process::exit(1);
                    }
                }
                info!("{}", shown.label());
            }

            Event::UserEvent(Wake::Menu(clicked)) => {
                if clicked.id == move_id {
                    // Read again rather than trusting the label: the keyboard can have
                    // moved by chord between the menu being drawn and being clicked, and
                    // an ask names the state it wants so that a stale label asks for
                    // where the keyboard already is rather than for the other one.
                    let asked = Where::read().asks_for();
                    match tray::running() {
                        Some(run) if tray::ask(run, asked) => {
                            info!("asked for the keyboard to be {asked:?}");
                        }
                        Some(_) => error!("the ask could not be posted to the run"),
                        None => error!("nothing answered; favjit is not running"),
                    }
                }
                if clicked.id == quit_id {
                    // The run first, and this afterwards whether or not that worked:
                    // somebody who clicked Quit wants the item gone either way, and a
                    // favjit that could not be reached is one they will have to end
                    // another way regardless.
                    match tray::running() {
                        Some(run) if tray::stop(run) => info!("asked favjit to stop"),
                        Some(_) => error!("favjit is running and would not take the ask to stop"),
                        None => info!("nothing was running to stop"),
                    }
                    *flow = ControlFlow::Exit;
                }
            }

            Event::UserEvent(Wake::Poll) => {
                let now = Where::read();
                if shown != now {
                    shown = now;
                    state.set_text(now.label());
                    move_it.set_text(now.move_label());
                    move_it.set_enabled(now.worth_pressing());
                    if let Some(tray_icon) = tray_icon.as_ref() {
                        if let Err(error) = tray_icon.set_icon(icon(now)) {
                            error!("cannot change the icon: {error}");
                        }
                        if let Err(error) = tray_icon.set_tooltip(Some(now.label())) {
                            error!("cannot change the tooltip: {error}");
                        }
                    }
                    info!("{}", now.label());
                }
            }

            _ => {}
        }
    });
}
