//! Rune, first slice: keyed element tree from plain view functions, Taffy layout, analytic
//! spring tracks, an instanced wgpu uber-quad renderer with Parley text, pointer and keyboard
//! input, scroll viewports, growth stacks, streaming reveals, themes as data, and headless agent
//! tooling.

#[cfg(all(target_arch = "wasm32", any(feature = "hot", feature = "native-file-dialog")))]
compile_error!("web does not support Rune's hot or native-file-dialog features; disable them for wasm32");

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod activation;
mod app;
pub mod time;
#[cfg(target_arch = "wasm32")]
pub mod web;
/// The logical clock a scene fn (`AppSpec::scenes`) builds its state against. `--scene` starts
/// at time zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SceneClock {
    pub now: f64,
    pub dt: f64,
}

thread_local! {
    static SCENE_CLOCK: std::cell::Cell<SceneClock> = const { std::cell::Cell::new(SceneClock { now: 0.0, dt: 0.0 }) };
}

/// Readable from scene fns, `AppSpec::init` and `AppSpec::setup`; zero anywhere else.
pub fn scene_clock() -> SceneClock {
    SCENE_CLOCK.get()
}

/// Runs `f` (a scene fn or `init`) with `clock` as the scene clock.
pub(crate) fn with_scene_clock<T>(clock: SceneClock, f: impl FnOnce() -> T) -> T {
    let before = SCENE_CLOCK.replace(clock);
    let out = f();
    SCENE_CLOCK.set(before);
    out
}

/// An XDG activation token from the latest pointer press on a Wayland overlay, for another app
/// to take focus with: a notification server's `ActivationToken` before `ActionInvoked`. Call it
/// from the press's handler. `app_id` names the app that will use it. Blocks for the
/// compositor's answer; None off Wayland, before any press, or without `xdg_activation_v1`.
pub fn press_activation_token(app_id: Option<&str>) -> Option<String> {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    return activation::press_token(app_id);
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    {
        let _ = app_id;
        None
    }
}

pub mod playground;
pub use playground::{Play, Playground, Stage, Step, Story};
mod states;
mod shortcuts;
pub use shortcuts::{PhysicalKeyCode, Shortcut, ShortcutError, ShortcutId, modifier_keycaps};
mod physical_input;
pub use physical_input::PhysicalInput;
mod canvas_input;
pub use canvas_input::{GesturePhase, MagnifyEvent, PointerButton, RotateEvent, WheelEvent};
mod layout_watch;
pub use layout_watch::ScrollPos;
mod tx_keys;
mod tx_scroll;
mod native_menu;
pub use native_menu::{MenuError, MenuItem};
mod popup_menu;
pub use popup_menu::{PopupAnchor, PopupItem, PopupMenu, PopupMenuError, popup_menu_supported};
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod layer_shell;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod x11_overlay;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod x11_pointer;
mod overlay;
mod overlay_options;
mod cursor;
mod cursor_image;
mod os_cursor;
#[cfg(any(test, target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
mod pointer_observer;
#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
mod win32_pointer;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_cursor;
#[cfg(target_os = "macos")]
mod macos_drop;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub mod macos_panel_bridge;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub mod macos_appearance;
#[cfg(target_os = "macos")]
mod display_link;
#[cfg(target_os = "macos")]
mod macos_help;
#[cfg(target_os = "macos")]
mod macos_traffic_lights;
#[cfg(target_os = "macos")]
mod macos_window_shape;
#[cfg(target_os = "macos")]
mod fn_key;
pub mod color;
pub mod compose;
pub mod edit;
pub mod el;
mod el_anchor;
mod el_backdrop;
mod el_child_exit;
mod el_drop;
mod el_effect;
mod el_pills;
mod el_pixel_snap;
mod el_ride_trailing;
mod el_sticky;
mod el_tx;
mod input_time;
mod key_diagnostics;
pub mod growth;
pub mod live;
pub mod memo;
pub mod motion_gate;
mod pixels;
pub mod gpu_springs;
pub mod image;
#[cfg(feature = "hot")]
pub mod hot;
pub mod render;
mod translucency;
pub use render::MAX_BACKDROP_BLURS;
pub use render::prewarm_gpu;
mod record;
mod headless_run;
mod keycaps;
mod surface_alpha;
mod surface_stats;
pub use surface_stats::{Pacing, SurfaceCounts, SurfaceStats};
pub mod native_surface;
mod system_material;
mod video;
mod recorder;
pub mod runtime;
pub mod shape;
mod virtual_list;
mod virtual_rows;
mod virtual_desktop;
mod desktop;
mod flight;
mod frame_profile;
#[cfg(feature = "frame-profile")]
#[doc(hidden)]
pub use frame_profile::{PHASES, take_frame_phases, thread_cpu_ms};
pub mod file_dialog;
pub mod tray;
mod open_url;
mod open_url_in;
pub mod prefs;
pub use open_url::OpenUrlError;
pub mod text;
pub mod theme;
pub mod tok;
pub mod tooling;
mod window;
mod reopen;
mod drop_pointer;
mod window_frame;
mod frame_clock;
mod frame_encoded;
pub use frame_encoded::{FrameEncoded, FrameOutcome};
pub use input_time::last_input;
mod present_time;
mod secure_input;
pub mod clipboard;
mod preferences;
mod accent;
pub use accent::{SystemAccent, system_accent};
pub use clipboard::{ClipboardContent, ClipboardImage, ClipboardWrite, SecretCopy, SecretState};
pub mod drag_out;
pub use drag_out::{DragImage, DragItem, DragOperation, DragOutcome};
pub mod windows;
pub use window::WindowHandle;
pub use windows::{AnyWindowSpec, ConfiguredWindowOptions, Display, MacWindowOptions, RunOptions, WindowOptions, WindowSpec};
pub use winit::raw_window_handle;

pub use app::{App, AppHandle};
pub use overlay::{Availability, OverlayHandle, OverlaySpec, ScreenEdge};
pub use overlay_options::{ConfiguredOverlay, DisplayId, DisplayTarget, NativeDisplayId, KeyPolicy, OverlayCapabilities, OverlayCreated, OverlayInput, OverlayLevel, OverlayOptions, OverlayPlacement, OverlaySupport};
pub use color::{Color, oklch, oklcha};
pub use compose::{Compositor, Frame};
pub use edit::{Block, Ime, Key, KeyEvent, KeySide, MAX_LIST_LEVEL, Mark, MarkRange, Modifier, Mods, ParagraphInfo, RichStyle, TextEdit};
pub use el::{
    Align, Anchor, Cursor, CursorImage, Cycle, Drag, El, GrowthSpec, Outline, Phase, PillStyle, PointerEvent, PointerPress, RadialRadius, ScrollSpec, Shadow, Shell, Side, Tx, auto_grow, canvas, col, edit_access, effect, el, icon, rich,
    row, spacer, text, text_input,
};
pub use growth::{GROWTH, StreamEdge, edge};
pub use live::{Live, LiveText, Tween};
pub use memo::memo;
pub use pixels::Pixels;
pub use tray::{Tray, TrayError, TrayEvent, TrayIcon, TrayItem, TraySpec};
pub use file_dialog::{FileDialogError, FileDialogKind, FileDialogOptions, FileDialogResult, FileDialogs, ScriptedFileDialog};
pub use image::{Image, ImageCache, ImageError, ImageId};
pub use rune_motion::{
    Anim, AnimExt, Beat, Blend, Clock, CubicBezier, Curve, Decay, Easing, Edge, Effect, LayoutAnim, LoopKind, LoopSpec, NodeId,
    Phase as PresencePhase, Presented, Reveal, Sequence, Spring, SpringPair, StepPosition, Transition, Travel, Vec2, Velocity,
};
pub use runtime::{Cx, ElementInfo, Env, MotionElement, Pick, Picked, Pose, Runtime, SelectedText, SelectionChange, TextSelection, TitlebarAction, Waker, commit_generation};
pub use shape::{LineCap, Path, Shape};
pub use native_surface::{NativeSurface, SurfaceFit};
pub use system_material::{MAX_NATIVE_MATERIALS, SystemMaterial};
pub use video::{MediaRequest, MediaSource, Player, PlayerState, Video};
pub use recorder::{FakeMicrophone, Level, MicPermission, RecordFormat, Recorder, RecorderState, Recording};
pub use text::{Icon, Link, LinkPaint, Span, TextCase, TextHighlight};
pub use theme::Theme;
pub use tok::{Family, OpticalSize, TextStyle};
pub use tooling::{AppSpec, Chrome, extra_args, run, run_windows, run_windows_with, run_windows_with_args, run_with, set_app_id,
    try_run_windows_with, try_run_windows_with_args};
