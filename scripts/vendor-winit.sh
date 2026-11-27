#!/usr/bin/env bash
# Re-makes vendor/winit: the winit crate exactly as published on crates.io at <version>, plus
# vendor/winit.patch (Windows: a new window isn't added to the taskbar list it is already on, which
# cost every window CoInitializeEx and a TaskbarList COM object, about 7,800 µs and nine shell
# DLLs on its UI thread before its first frame; and a window given a position is made there, at
# its display's scale, not moved there and resized; and a new window's system menu, whose Close
# is already enabled, isn't copied for the window just to enable it again; and a window is made
# with the minimize and maximize boxes it asks for, not given them by a second style change; and winit's own
# helper window is cloaked, so screen readers and window pickers don't meet it; and Windows system DLLs
# load via `LoadLibraryExA` with `LOAD_LIBRARY_SEARCH_SYSTEM32`; and on macOS the input method reads the app's text around its selection, to convert it again, and its marked text's clauses reach the app, beside `Ime::Preedit`, through `platform::macos::{set_ime_text, take_ime_clauses, take_ime_replacement}`; and
# `platform::wayland::WindowExtWayland::content_corner_radius` says what radius a Wayland client-side frame
# leaves the window's content's bottom corners at, 0 when it draws none or draws it square; and
# `platform::wayland::ActiveEventLoopExtWayland::set_wake_on_wayland_read` opts into waking from
# `ControlFlow::Wait` on any successful Wayland display read so foreign queues such as layer-shell
# overlays dispatch without background reader threads; and Windows `handle_os_dragging` packs signed
# screen coordinates by value into `WM_NCLBUTTONDOWN`'s `LPARAM`, skips the dummy `WM_MOUSEMOVE(0, 0)`
# during programmatic drags, and clears `state.dragging` if `DefWindowProcW` exits without
# `WM_EXITSIZEMOVE`; and Wayland `set_window_icon` sets the window's icon through `xdg-toplevel-icon-v1`,
# backported from rust-windowing/winit#4288; and `WindowExtWayland::wayland_wm_state` + configure
# wake on decoration/capability changes, `WindowExtX11::take_wm_state_changed` on `PropertyNotify`,
# and Wayland `current_monitor` skipping destroyed outputs + deduplicating `WinitState.monitors` (`new_output` upsert / `output_destroyed` `retain`) + waking `AboutToWait` on output changes; and Windows `set_fullscreen` updates styles (clearing `WS_EX_*EDGE` flags) and bounds atomically in one synchronous `SetWindowPos(..., SWP_FRAMECHANGED)` before `ITaskbarList2::MarkFullscreenWindow`, while borderless `WM_NCCALCSIZE` leaves fullscreen rects uninset).
# Run it when winit is bumped, then build: a patch
# that no longer applies fails here, and the change goes back in by hand. If upstream winit takes
# the change, delete vendor/winit and its `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-winit.sh <version>     e.g. scripts/vendor-winit.sh 0.30.13
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-winit.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/winit/winit-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/winit
mv "$tmp/winit-$version" vendor/winit
git apply --directory=vendor/winit vendor/winit.patch
echo "vendor/winit: winit $version + vendor/winit.patch"
