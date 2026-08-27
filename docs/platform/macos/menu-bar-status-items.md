# Menu bar status items

Observed on macOS 26.6.2 (build 25G83, Darwin 25.6.0, arm64 T6050), 2026-08-29,
on the built-in display of a MacBook Pro.

## An item is drawn only if the bar has width for it

A status item created through `tray-icon` did not appear on a bar already holding
around a dozen items, and appeared as soon as one of them gave up its width:
shortening the input menu from `あ ひらがな (Google)`, about 130 points, to the `あ`
icon alone was enough. The clock is the other large one, about 110 points for
`8月29日(土) 12:32`. An icon-only item is 34 points wide, and 70 with a title.

Freeing width is not the same as freeing a place. Quitting an application that held
a visible item let a *different* application's item take the space, not the one that
had been waiting; which item wins is not something these measurements explain.

The visibility of each item is a per-display setting under System Settings → Menu
Bar. Underneath it is an `NSStatusItem VisibleCC <name>` key in the owning process's
defaults domain — `com.apple.controlcenter` for Wi-Fi, battery and the clock,
`com.apple.Spotlight` for the magnifier.

## Creation succeeds either way, and the frame does not say where it went

`statusItemWithLength` returns an item, its `button` is non-nil, and its frame comes
back populated whether or not anything is drawn. So there is nothing in the API to
tell "created and shown" from "created and dropped".

The frame is no help in finding it either. It reads as unplaced for the first half
second, and the settled reading does not correspond to where items are drawn: it
gave a physical x of 1888 while the leftmost drawn item on the same bar was at
roughly 1072 points, on a 1800×1169 point screen at 2×.

Both the invisible and the visible process had the same audit session id under
`launchctl print`, and Launch Services listed both as `UIElement`, so nothing about
the session or the registration distinguishes them.

## Reading the bar

`screencapture -x -R<x>,<y>,<w>,<h>` writes the strip to a file without the shutter
sound or the interactive selection, and the numbers are points on the display. It is
the way to see what is actually drawn, since the frame an item reports does not say.

`NSScreen` answers for the geometry around it: on this machine `frame` was
1800×1169 points at a `backingScaleFactor` of 2, `safeAreaInsets.top` 38, and the
two `auxiliaryTop*Area` rectangles `(0, 1131, 790, 38)` and `(1010, 1131, 790, 38)`,
which put the notch between x = 790 and x = 1010. `NSStatusBar.system.thickness`
was 22.

## Two ways to lose an item that was working

`set_icon` without the template flag leaves the item with no width at all: it
vanishes from the bar on the first change of state. The call that carries the flag —
`set_icon_with_as_template` in `tray-icon` — keeps it.

`open -a <bundle>` will not start a new instance while any process from that bundle
is running, and drops the arguments it was given. A bundle whose binaries include a
daemon needs `open -n`. `open -W` cannot block on an `LSUIElement` application: it
prints `Unable to block on application` and returns at once.
