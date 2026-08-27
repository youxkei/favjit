//! `favjit-menu` — the menu bar item that turns converting off and on.
//!
//! The escape that does not need the keyboard. The supervisor covers a favjit that
//! hangs, and nothing covers one that is alive and converting wrongly: it keeps
//! heartbeating, so it is never killed, and a converter producing the wrong keys is
//! one you cannot type `favjit --disable` into. This is reachable with the trackpad,
//! which favjit never captures whatever it is doing to the keyboards.
//!
//! It holds no state of its own. What "off" means is the control file in
//! `host-macos`, the same one the converter reads, so the tick in the menu and the
//! converter's behaviour cannot disagree.
//!
//! It runs in the user's login session and needs no privilege — writing that file is
//! all it does. The converter needs root and so cannot be here; this needs a window
//! server connection and so cannot be there.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use favjit_host_macos::control;
use log::{error, info};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// How often the menu looks at the control file.
///
/// The file is what says whether favjit is converting, and anything can write it —
/// `favjit --disable` from a terminal, or the installer taking favjit out. Polling
/// is what keeps the label honest about the state rather than about the last click
/// made here.
const POLL: Duration = Duration::from_millis(500);

/// How many polls to let pass before the item's position is worth reading.
///
/// Four, which is two seconds: the frame is populated well before that, and asking
/// too early gives a position from before the item was laid out — a number that
/// would say the wrong thing about where it ended up.
const SETTLED_POLLS: u32 = 4;

/// What wakes the loop up.
///
/// Both arrive on threads that are not the main one — the poll from a timer thread,
/// a click from muda's own callback — and both have to be handled where the status
/// item lives. Sending them into the loop is what puts them on the main thread;
/// touching the item from those threads is what `MainThreadMarker` refuses.
enum Wake {
    Poll,
    Menu(MenuEvent),
}

/// The icon, drawn rather than shipped.
///
/// A square with a hole when converting and a filled one when off: an image file
/// would be a second thing to install and to keep beside the binary, for eighteen
/// pixels of square.
///
/// The pixels are opaque white and the image is handed over as a template, so only
/// the alpha is kept and the system inks it — black on a light menu bar, white on a
/// dark one. A literal colour here is invisible under one appearance or the other.
fn icon(converting: bool) -> Option<Icon> {
    const SIDE: u32 = 18;
    let mut rgba = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let edge = x < 2 || y < 2 || x >= SIDE - 2 || y >= SIDE - 2;
            let ink = if converting { edge } else { true };
            let alpha = if ink { 255 } else { 0 };
            rgba.extend_from_slice(&[255, 255, 255, alpha]);
        }
    }
    Icon::from_rgba(rgba, SIDE, SIDE)
        .map_err(|error| error!("cannot draw the icon: {error}"))
        .ok()
}

/// The line that says what favjit is doing.
fn state_label(converting: bool) -> &'static str {
    if converting {
        "favjit: converting"
    } else {
        "favjit: off"
    }
}

/// The line you press to change it.
fn toggle_label(converting: bool) -> &'static str {
    if converting {
        "Stop converting"
    } else {
        "Start converting"
    }
}

fn control_file() -> Option<PathBuf> {
    // `--control` first, because launchd is what starts this and the path it passes
    // is the one the installer gave the converter. Deriving it from `HOME` as well
    // covers a run from a terminal, where there is nobody to pass it.
    let mut args = std::env::args().skip_while(|a| a != "--control").skip(1);
    match args.next() {
        Some(path) => Some(PathBuf::from(path)),
        None => control::console_home().map(|home| control::path(&home)),
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let Some(control) = control_file() else {
        error!("cannot tell whose control file to watch; pass --control PATH");
        std::process::exit(1);
    };
    info!("watching {}", control.display());

    let mut event_loop = EventLoopBuilder::<Wake>::with_user_event().build();
    // Accessory, or this appears in the Dock and in the application switcher as an
    // app with no windows — and `Prohibited` would be worse, since a prohibited
    // process gets no menu bar either.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    MenuEvent::set_event_handler(Some({
        let proxy = event_loop.create_proxy();
        move |event| {
            let _ = proxy.send_event(Wake::Menu(event));
        }
    }));
    std::thread::spawn({
        let proxy = event_loop.create_proxy();
        move || {
            // Until the send fails, which is how this thread learns the loop is
            // gone: there is nothing else to tell it, and a thread outliving the
            // event loop would keep the process alive after Quit.
            while proxy.send_event(Wake::Poll).is_ok() {
                std::thread::sleep(POLL);
            }
        }
    });

    // Both labels say the state favjit is in right now, because the polling below
    // only rewrites them when it changes: an item built with a placeholder would
    // keep it until the first click somebody made in the dark.
    let converting = control::is_converting(&control);
    let toggle = MenuItem::new(toggle_label(converting), true, None);
    let state = MenuItem::new(state_label(converting), false, None);
    let menu = Menu::new();
    // No Quit. launchd keeps this alive on purpose — an escape hatch that quietly
    // went missing at some point in the session would be missing exactly when it is
    // needed — so a Quit item would be a thing you press that does not happen. Taking
    // the item away for good is `favjit --uninstall`.
    if let Err(error) = menu.append_items(&[&state, &PredefinedMenuItem::separator(), &toggle]) {
        error!("cannot build the menu: {error}");
        std::process::exit(1);
    }

    let toggle_id = toggle.id().clone();
    // Populated at `Init` rather than here: the status item has to be created once
    // the loop is running, and one created before that misbehaves around fullscreen
    // apps.
    let mut tray: Option<TrayIcon> = None;
    let mut menu = Some(menu);
    let mut shown = Some(converting);
    let mut polls: u32 = 0;

    event_loop.run(move |event, _target, flow| {
        // Wait, not a deadline: the poll arrives as an event of its own, so a
        // timeout here would only add wake-ups that have nothing to do.
        *flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                let menu = menu.take().expect("Init happens once");
                match TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_tooltip("favjit")
                    .with_icon(icon(control::is_converting(&control)).unwrap_or_else(|| {
                        error!("cannot draw the icon, so there would be nothing to click");
                        std::process::exit(1)
                    }))
                    // Icon and no title: the title doubles the item's width, and
                    // width is what decides whether a crowded menu bar has room to
                    // draw it at all.
                    .with_icon_as_template(true)
                    .build()
                {
                    Ok(built) => tray = Some(built),
                    Err(error) => {
                        error!("cannot put an item in the menu bar: {error}");
                        std::process::exit(1);
                    }
                }
                shown = Some(control::is_converting(&control));
            }

            Event::UserEvent(Wake::Menu(clicked)) => {
                if clicked.id == toggle_id {
                    click(&control);
                }
            }

            Event::UserEvent(Wake::Poll) => {
                // Creating an item succeeds on a menu bar that has no room to draw
                // it, and nothing in the API says which of the two happened
                // (`docs/platform/macos/menu-bar-status-items.md`). So the frame goes
                // in the log: it does not locate the item, but a run that got one at
                // all is worth telling apart from a run that did not. Not on the
                // first poll — half a second in, it still reads as unplaced.
                polls += 1;
                if polls == SETTLED_POLLS {
                    if let Some(rect) = tray.as_ref().and_then(TrayIcon::rect) {
                        info!(
                            "at {},{} sized {}x{}; if there is nothing to click, the menu bar is \
                             full",
                            rect.position.x, rect.position.y, rect.size.width, rect.size.height
                        );
                    }
                }

                let converting = control::is_converting(&control);
                if shown != Some(converting) {
                    shown = Some(converting);
                    state.set_text(state_label(converting));
                    toggle.set_text(toggle_label(converting));
                    if let Some(tray) = tray.as_ref() {
                        // Said again with every icon: the plain `set_icon` drops the
                        // template flag, and the item loses its width along with it —
                        // it disappears from the bar on the first change of state,
                        // which is the moment it is most being looked at.
                        if let Err(error) = tray.set_icon_with_as_template(icon(converting), true) {
                            error!("cannot change the icon: {error}");
                        }
                    }
                }
            }

            _ => {}
        }
    });
}

/// Answer a click by writing the file.
///
/// What the label says afterwards comes from reading the file back on the next poll
/// rather than from assuming the write did what was asked: the file is the state,
/// and it can change from anywhere.
fn click(control: &Path) {
    let converting = control::is_converting(control);
    let outcome = if converting {
        control::disable(control)
    } else {
        control::enable(control)
    };
    match outcome {
        Ok(()) => info!(
            "{}",
            if converting {
                "off; the keyboards are the machine's own"
            } else {
                "converting again"
            }
        ),
        Err(error) => error!("cannot write {}: {error}", control.display()),
    }
}
