# Global shortcuts

```rust,ignore
let app = App::new(spec)
    .global_shortcut("cmd+shift+space", |state, tx| {
        state.quick_panel_open = true;
        if let Some(token) = tx.activation_token() {
            state.app_handle.activate(token);
        } else {
            state.app_handle.open();
        }
    })
    .windowless();
app.run();
```

The handler receives the same state as its `App`/`WindowSpec`, on the app thread.
It runs through `Runtime::update`, including animation and shell requests. Native
activation wakes the loop even while no window is visible.

Static builder failures print once and can be drained with
`handle.take_shortcut_errors()`. The builder defers registration until the app's
host exists, so `run_windows_with` factories can construct apps normally.

Runtime registration returns the actual result:

```rust,ignore
let id = handle.register_shortcut("ctrl+alt+k".parse()?, |state, tx| {
    state.open_quick_panel(tx.activation_token());
})?;
handle.unregister_shortcut(id)?;
```

After a `Conflict`, `handle.shortcut_owner(&shortcut)` names the holder when Rune or
the desktop knows it: this App or another Rune App in the process (its `AppSpec`
title), or a macOS system shortcut ("Spotlight", "Mission Control", "Input Sources",
"Screenshot", "App Switcher", "macOS" for other enabled symbolic hotkeys), at the chord
the user remapped it to. The macOS lookup answers only on the main thread. A chord
another Rune App in the process holds is refused with `Conflict` on every backend. Other apps'
grabs, Windows, X11 and the portal name nobody, so it returns None. Headless runs
answer only for their own virtual registrations. `ShortcutError::Conflict` itself is
unchanged.

A chord needs ⌘, ⌃ or ⌥ (Ctrl, Alt or Super elsewhere), or an F1–F24 key, because a
global Space or ⇧A would take that key from typing in every app. Both the builder and
`register_shortcut` refuse one without them with `InvalidAccelerator`
(`shortcut.require_modifier()` checks it), unless the App opts in with
`.allow_unmodified_shortcuts()`.

Call runtime registration after the host starts (an `on_window` or queued
`handle.update` callback works, including one on another window of the same
`run_windows`). Calls before startup or after shutdown return `NotRunning`. On macOS/Windows other threads may call it, but wait for the app
thread, so the app thread must not join those callers. Linux registration can
run off-thread. Dropping the host unregisters everything,
even if other threads retain `AppHandle`s.

## Desktop behavior

- macOS: Carbon `RegisterEventHotKey`, no Accessibility permission. Command is
  the Super modifier. Hardware key positions follow the US layout; F21–F24 have
  no Carbon key code and return `InvalidAccelerator`.
- Windows: `RegisterHotKey` on a message-only window owned by the event thread.
  Repeat is disabled. Windows-reserved combinations may fail registration.
- X11: checked passive grabs on every root, including CapsLock/NumLock variants.
  A failed variant rolls back the registration. Autorepeat is filtered, so a held
  chord fires once, as on macOS and Windows. Keys use the current keymap's
  base keysyms; restart/re-register after changing the keymap.
- Wayland: xdg-desktop-portal GlobalShortcuts. No portal returns `Unsupported`.
  The portal may ask for permission or let the user choose another trigger;
  `preferred_trigger` is a preference, not an exclusive-grab guarantee. Cancellation
  is `PermissionDenied`, not `Conflict`. Each shortcut owns a portal session so
  unregister also works with version 1. The activation token is available only
  during its handler, through `Tx::activation_token()`.

Registration is synchronous, including portal consent, with a two-minute bound.
Register startup shortcuts before showing windows; use a worker thread for
runtime Wayland binding so a consent dialog does not block the event thread.

Only an actual registration creates native resources: macOS/Windows need no
listener thread; X11/Wayland each create a listener that is joined on teardown.
No-shortcut apps never open a shortcut display or portal connection.

## Deterministic testing

Headless scripts register virtually and invoke the same handlers:

```json
[{"t": 0, "shortcut": "cmd+shift+space"}]
```

Unknown/unregistered shortcuts fail the script. Parsing is case-insensitive;
Display normalizes modifiers to `ctrl+alt+shift+super`, then the lowercase key.
`cmd`, `command`, `meta`, and `logo` alias `super`; `option` aliases `alt`.

`scripts/check-global-shortcut-macos.py <shortcut_probe>` checks two-process
conflicts and real CuaDriver input on a GUI test Mac, under an exclusive GUI lock file.
The probe stays windowless and verifies handler thread identity.

## Shortcut recorder

`rune_kit::ShortcutRecorder` uses app-owned `ShortcutRecorderState` and renders the
existing `Keybind` component:

```rust,ignore
ShortcutRecorder::new("shortcut", &state.shortcut, |s: &mut State| &mut s.shortcut)
    .on_change(|state, shortcut, tx| state.replace_shortcut(shortcut, tx))
    .build(&kit)
```

The change callback returns `Result<(), ShortcutError>`. A registration failure
retains the old binding; the recorder keeps showing the refused chord with a message
until another modifier starts the next chord. Unchanged chords skip the callback,
unless `state.mark_unregistered()` says the shortcut isn't registered (it failed at
launch): then recording it again retries it through the callback. The
default message names the chord ("⌃J is already in use."); `.error_message(|error,
chord| ...)` rewords it, falling back on `rune_kit::shortcut_error_message`, and
`ShortcutRecorderState::refused()` reads the chord. A chord without a modifier (other
than F1–F24) is refused before the callback with "Add ⌘, ⌃ or ⌥." unless
`.allow_unmodified()`.
Click or keyboard activation starts recording, held modifiers preview immediately,
Escape cancels, Backspace clears, and focus loss or disabling cancels recording.
Repeated key presses are consumed without registering again.
`scripts/shortcut-recorder.json` records a chord headlessly.

`Shortcut` stores a `PhysicalKeyCode`. Parsing and Display use canonical US physical
names; `keycaps()` is for display only. On the macOS main thread, printable labels
use `TISCopyCurrentKeyboardLayoutInputSource` and `UCKeyTranslate` without changing
the input source. Other platforms, background callers and unavailable layouts use
US labels. Persist the canonical shortcut, not its translated keycaps.

Physical input and its logical fallback are processed together in event order.
Focused capture runs before modal Escape, text editing and paste. A consumed chord
never reads the clipboard. Otherwise eligible Cmd-V requests the asynchronous
clipboard worker and consumes the command even for empty/image-only clipboards.
The captured paste target prevents a later focus change from receiving the reply.
Single-line and secure editors retain their ordinary editing and copy protections.
Key-taking launcher panels retain winit's input view/delegate and use the window
input path; overlays and Wayland layer surfaces also forward physical input.

## Native application and Dock menus

```rust,ignore
let app = App::new(spec)
    .menu_item(MenuItem::new("Open Quick Panel", open_panel).id("quick")
        .key_equivalent("cmd+shift+space".parse()?))
    .menu_item(MenuItem::new("Preferences", preferences).menu("Tools"))
    .menu_item(MenuItem::new("Open Quick Panel", open_panel).id("dock-quick").dock());

handle.set_menu([
    MenuItem::new("Open Quick Panel", open_panel).id("quick")
        .enabled(true).checked(true),
])?;
```

Actions are `fn(&mut S, &mut Tx)` queued on the app thread. `set_menu` replaces all
custom items for that App; an empty list removes them. It validates atomically and
queues installation. `take_menu_errors()` reports deferred startup/installation
errors. Replaced, disabled or removed items invalidate stale queued actions.

On macOS, one App owns the process-wide custom menus. Default Quit/Hide items and
existing named-menu items are preserved. The Dock adapter adds an absent
`applicationDockMenu:` selector to winit's existing delegate class, retaining the
delegate and all existing methods. An existing non-Rune selector is an error; the
Rune hook returns nil after teardown. Key equivalents are native menu equivalents,
not global registrations. Reapply the snapshot after changing keyboard layout to
refresh translated equivalents.

An accessory app that calls `set_dock_icon(true)` gets winit's standard app menu
if `NSApp.mainMenu` is absent: About, Services, Hide, Hide Others, Show All and Quit.
Creating custom menu items first uses that same default root; adding them later
inserts alongside the defaults. Existing roots are never replaced, and clearing
custom items or calling `set_dock_icon(false)` leaves the default menu intact.
No Edit menu is installed: native edit roles are not implemented, and AppKit's
standard edit selectors could intercept Rune's focused-field shortcuts.

Native Windows/Linux return `MenuError::Unsupported`; startup builders report it
through `take_menu_errors()`. Headless virtual menus work on every platform:
`{"menu":"Open Quick Panel"}` invokes by exact id or unambiguous title. Unknown,
disabled and ambiguous items fail the script.

`scripts/check-native-menu-macos.py <menu_probe>` exercises the app-menu click,
key equivalent, named-menu click, runtime enabled/checked updates, preservation of
Quit/Hide and the Dock delegate selector on a GUI test Mac, under the same lock.
`scripts/check-dock-menu-macos.py <dock_menu_probe>` checks the accessory-to-Regular
default menu in both orders. `scripts/menu.json` exercises title/id dispatch headlessly.
