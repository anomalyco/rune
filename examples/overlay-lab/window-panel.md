# Ordinary quick panels

`AppSpec`, `WindowSpec`, `WindowOptions` and `OverlaySpec` literals retain their fields.
Configure focus callbacks on `App::on_focus`, `WindowSpec::on_focus`, or
`ConfiguredOverlay::on_focus`. Each callback takes `fn(&mut S, bool, &mut Tx)`.
Repeated focus notifications are deduplicated per runtime. Callbacks run on the event
thread even when the window is hidden/occluded, before requiring a drawable.
`Tx::close_window()` dismisses a keep-running ordinary window or hides an overlay.

```rust,ignore
let quick = App::new(spec).windowless().on_focus(|_, focused, tx| {
    if !focused { tx.close_window(); }
});
let handle = quick.handle();
let window = WindowSpec::from_app("quick", quick).configured_options(
    WindowOptions { borderless: true, ..Default::default() }
        .macos(MacWindowOptions::default().nonactivating_panel(true))
);
// From a shortcut callback on any thread:
handle.place(DisplayTarget::UnderPointer, OverlayPlacement::Edge {
    edge: ScreenEdge::Top, margin: 160.0, align: Align::Center, inset: 0.0,
});
handle.open();
```

The macOS panel takes keys without activating its application. `show()` does not take
keys; `open()`/`focus()` do. Hiding lets AppKit return keys to the prior application.
For a single-window app use `App::macos_window(...)`. Use `App::accessory()` or
`run_windows_with(RunOptions { accessory: true }, ...)` to prevent launch activation too.
Panel policy is ignored on other platforms; poll, scenes, on_key and headless tooling
remain ordinary-window features.

Both AppHandle and WindowHandle have `place(display, placement)`. UnderPointer samples
at the calling thread, not after a delayed event-loop wake. Placement is retained across
recreation and re-resolved on open and scale changes; disconnected display IDs fall back
to primary. Rect and FullScreen resize as well as place; Edge uses the current window size.
Coordinates are display-local points, as for overlays. Wayland ignores placement requests.
`set_position` replaces a pending/retained display placement, and vice versa.

Headless scripts accept `{"t": 0.1, "focus": false}` (time defaults to zero), delivering
the same app callback as native input. Existing root `El::on_window_focus` callbacks also
receive the queued focus event when Runtime next advances.

`cargo run -p rune --example window-panel --release` supplies two independent windows with
the same state type. `WINDOW_PANEL_SOCKET` enables the fixture's open/focus/hide/status/quit
commands. `native/run-window-panel-probe.py` holds the Pro GUI lock for a bounded Cua proof.
