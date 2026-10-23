//! Rune, first slice: keyed element tree from plain view functions, Taffy layout, analytic
//! spring tracks, an instanced wgpu uber-quad renderer with Parley text, pointer and keyboard
//! input, scroll viewports, growth stacks, streaming reveals, themes as data, and headless agent
//! tooling.

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod activation;
mod app;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod layer_shell;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod x11_overlay;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod x11_pointer;
mod overlay;
mod overlay_options;
#[cfg(any(test, target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
mod pointer_observer;
#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
mod win32_pointer;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_panel_bridge;
pub mod color;
pub mod compose;
pub mod edit;
pub mod el;
mod key_diagnostics;
pub mod growth;
pub mod live;
mod pixels;
pub mod gpu_springs;
pub mod image;
pub mod render;
mod surface_alpha;
pub mod native_surface;
pub mod runtime;
pub mod shape;
mod virtual_list;
mod flight;
pub mod file_dialog;
pub mod text;
pub mod theme;
pub mod tok;
pub mod tooling;
mod window;
mod secure_input;
mod clipboard;
pub use clipboard::ClipboardWrite;
pub mod windows;
pub use window::WindowHandle;
pub use windows::{ConfiguredWindowOptions, Display, MacWindowOptions, RunOptions, WindowOptions, WindowSpec};
pub use winit::raw_window_handle;

pub use app::{App, AppHandle};
pub use overlay::{Availability, OverlayHandle, OverlaySpec, ScreenEdge};
pub use overlay_options::{ConfiguredOverlay, DisplayId, DisplayTarget, NativeDisplayId, KeyPolicy, OverlayCapabilities, OverlayCreated, OverlayInput, OverlayLevel, OverlayOptions, OverlayPlacement, OverlaySupport};
pub use color::{Color, oklch, oklcha};
pub use compose::{Compositor, Frame};
pub use edit::{Ime, Key, KeyEvent, Mods, TextEdit};
pub use el::{
    Align, Anchor, Cursor, Cycle, Drag, El, GrowthSpec, Outline, Phase, PointerPress, ScrollSpec, Shadow, Shell, Side, Tx, canvas, col, edit_access, el, icon, rich,
    row, spacer, text, text_input,
};
pub use growth::{GROWTH, StreamEdge, edge};
pub use live::{Live, Tween};
pub use pixels::Pixels;
pub use file_dialog::{FileDialogError, FileDialogKind, FileDialogOptions, FileDialogs, ScriptedFileDialog};
pub use image::{Image, ImageCache, ImageError, ImageId};
pub use rune_motion::{
    Anim, AnimExt, Clock, Curve, Easing, Edge, LayoutAnim, NodeId, Phase as PresencePhase, Presented, Spring,
    SpringPair, Transition, Travel,
};
pub use runtime::{Cx, Env, Runtime, TitlebarAction, Waker};
pub use shape::{LineCap, Path, Shape};
pub use native_surface::{NativeSurface, SurfaceFit};
pub use text::{Icon, Span};
pub use theme::Theme;
pub use tok::{Family, TextStyle};
pub use tooling::{AppSpec, Chrome, extra_args, run, run_windows, run_windows_with, run_with, set_app_id};
