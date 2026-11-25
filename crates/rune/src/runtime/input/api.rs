//! Thin `#[inline(always)]` public and crate-internal forwarders on [`Runtime<S>`] delegating
//! to non-generic [`RuntimeCore`] for pointer, keyboard, gesture, touch, scroll, drop, popup,
//! file dialog, titlebar, help, and text selection.

use std::path::PathBuf;

use super::*;
#[cfg(test)]
#[cfg(test)]
use crate::file_dialog::DialogOutcome;
use crate::file_dialog::{FileDialogError, FileDialogKind, FileDialogOptions};
#[cfg(any(test, target_arch = "wasm32"))]
use crate::popup_menu::Show;
use crate::popup_menu::{PopupMenu, PopupMenuError};
use crate::{MagnifyEvent, PhysicalInput, PointerButton, RotateEvent, WheelEvent};

#[allow(dead_code)]
impl<S: 'static> Runtime<S> {
    // ── Pointer & secondary ────────────────────────────────────────────────────────────────────

    #[inline(always)]
    pub fn pointer_move(&mut self, x: f32, y: f32) {
        let (core, app) = self.split_mut();
        core.pointer_move(&app, x, y);
    }

    #[inline(always)]
    pub(crate) fn observe_pointer(&mut self, x: f32, y: f32) -> bool {
        let (core, app) = self.split_mut();
        core.observe_pointer(&app, x, y)
    }

    #[inline(always)]
    pub(crate) fn pointer_captured(&self) -> bool { self.core().pointer_captured() }
    #[inline(always)]
    pub(crate) fn set_window_pixels(&mut self, pixels: Option<(i32, i32)>) { self.core_mut().set_window_pixels(pixels); }
    #[inline(always)]
    pub(crate) fn window_moved(&mut self, dx: f32, dy: f32) { self.core_mut().window_moved(dx, dy); }
    /// Cancel any active pointer capture, drag or hover: ends a held drag with a cancelled
    /// [`Phase::Up`] ([`Drag::cancelled`]), clears
    /// press, secondary, tint, touch-hold, drag-out and autoscroll state, and leaves hover.
    ///
    /// ```
    /// # use rune::{Cx, Drag, El, Env, Phase, Runtime, col, el};
    /// # #[derive(Default)] struct S { phases: Vec<(Phase, bool)> }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// #     col().size(200.0, 100.0).child(
    /// #         el().key("tile").size(100.0, 60.0)
    /// #             .on_drag(|s: &mut S, d: &mut Drag, _| s.phases.push((d.phase, d.cancelled()))),
    /// #     )
    /// # }
    /// let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// let (x, y) = rt.find("tile").unwrap();
    /// rt.pointer_down(x, y);
    /// rt.pointer_move(x + 12.0, y);
    /// rt.cancel_pointer();
    /// rt.frame(0.0);
    /// assert!(!rt.pressing());
    /// assert_eq!(rt.state.phases.last(), Some(&(Phase::Up, true)));
    /// ```
    #[inline(always)]
    pub fn cancel_pointer(&mut self) { self.core_mut().cancel_pointer(); }
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn disarm_drag_out(&mut self) { self.core_mut().disarm_drag_out(); }

    #[inline(always)]
    pub fn pointer_down(&mut self, x: f32, y: f32) {
        let (core, mut app) = self.split_mut();
        core.pointer_down(&mut app, x, y);
    }

    #[inline(always)]
    pub fn pointer_up(&mut self, x: f32, y: f32) {
        let (core, app) = self.split_mut();
        core.pointer_up(&app, x, y);
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn pointer_release_lost(&mut self, x: f32, y: f32) {
        let (core, app) = self.split_mut();
        core.pointer_release_lost(&app, x, y);
    }

    #[inline(always)]
    pub fn pointer_leave(&mut self) { self.core_mut().pointer_leave(); }
    #[inline(always)]
    pub fn pressing(&self) -> bool { self.core().pressing() }
    #[inline(always)]
    pub fn pointer(&self) -> Option<(f32, f32)> { self.core().pointer() }
    #[inline(always)]
    pub fn hit_regions(&self) -> &[[f32; 4]] { self.core().hit_regions() }
    #[inline(always)]
    pub fn cursor(&self) -> crate::el::Cursor { self.core().cursor() }
    #[inline(always)]
    pub fn wheel(&mut self, x: f32, y: f32, dy: f32) { self.core_mut().wheel(x, y, dy); }
    #[inline(always)]
    pub fn set_wheel_glide(&mut self, on: bool) { self.core_mut().scroll.set_wheel_glide(on); }
    #[inline(always)]
    pub fn fling(&mut self, x: f32, y: f32, velocity: f32) { self.core_mut().fling(x, y, velocity); }
    #[inline(always)]
    pub fn has_pending_input(&self) -> bool { self.core().has_pending_input() }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn has_input_waiting(&self) -> bool { self.core().has_input_waiting() }

    #[inline(always)]
    pub fn secondary_down(&mut self, x: f32, y: f32) { self.core_mut().secondary_down(x, y); }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn secondary_claim(&self) -> &'static str { self.core().secondary_claim() }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn secondary_abandon(&mut self) { self.core_mut().secondary_abandon(); }
    #[inline(always)]
    pub fn secondary_up(&mut self, x: f32, y: f32) {
        let (core, app) = self.split_mut();
        core.secondary_up(&app, x, y);
    }

    // ── Gestures, touch & scroll ───────────────────────────────────────────────────────────────

    #[inline(always)]
    pub fn wheel_event(&mut self, ev: WheelEvent) { self.core_mut().wheel_event(ev); }
    #[inline(always)]
    pub fn magnify_event(&mut self, ev: MagnifyEvent) { self.core_mut().magnify_event(ev); }
    #[inline(always)]
    pub fn rotate_event(&mut self, ev: RotateEvent) { self.core_mut().rotate_event(ev); }
    #[inline(always)]
    pub fn smart_magnify(&mut self, x: f32, y: f32) { self.core_mut().smart_magnify(x, y); }
    #[inline(always)]
    pub(crate) fn pinch_wheel(&mut self, x: f32, y: f32, dy: f32, ticks: Option<f32>) -> bool { self.core_mut().pinch_wheel(x, y, dy, ticks) }
    #[inline(always)]
    pub fn button_down(&mut self, x: f32, y: f32, button: PointerButton) {
        let (core, mut app) = self.split_mut();
        core.button_down(&mut app, x, y, button);
    }
    #[inline(always)]
    pub fn button_up(&mut self, x: f32, y: f32, button: PointerButton) {
        let (core, app) = self.split_mut();
        core.button_up(&app, x, y, button);
    }
    #[inline(always)]
    pub fn key_up(&mut self, ev: KeyEvent) -> bool {
        let (core, mut app) = self.split_mut();
        core.key_up(&mut app, ev)
    }
    #[inline(always)]
    pub(crate) fn hears_wheel(&self) -> bool { self.core().hears_wheel() }
    #[inline(always)]
    pub(crate) fn wants_key_up(&self) -> bool { self.core().wants_key_up() }

    #[inline(always)]
    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        let (core, mut app) = self.split_mut();
        core.touch(&mut app, id, phase, x, y, time);
    }
    #[inline(always)]
    pub fn touching(&self) -> bool { self.core().touching() }

    #[inline(always)]
    pub fn wheel_xy(&mut self, x: f32, y: f32, dx: f32, dy: f32) { self.core_mut().wheel_xy(x, y, dx, dy); }
    #[inline(always)]
    pub fn wheel_began(&mut self) { self.core_mut().wheel_began(); }
    #[inline(always)]
    pub fn wheel_ended(&mut self) { self.core_mut().wheel_ended(); }
    #[inline(always)]
    pub fn fling_xy(&mut self, x: f32, y: f32, vx: f32, vy: f32) { self.core_mut().fling_xy(x, y, vx, vy); }
    #[cfg(target_os = "windows")]
    #[inline(always)]
    pub(crate) fn windows_wheel_points(&self, delta: winit::event::MouseScrollDelta, shift: bool) -> f32 {
        self.core().windows_wheel_points(delta, shift)
    }
    #[inline(always)]
    pub fn scroll_at_bottom(&self, key: &str) -> Option<bool> { self.core().scroll_at_bottom(key) }

    // ── Keys, IME, physical & modifiers ────────────────────────────────────────────────────────

    #[inline(always)]
    pub fn key(&mut self, ev: KeyEvent) { self.core_mut().key(ev); }
    #[inline(always)]
    pub fn repeat_key(&mut self, ev: KeyEvent) { self.core_mut().repeat_key(ev); }
    #[inline(always)]
    pub fn ime(&mut self, ev: Ime) { self.core_mut().ime(ev); }
    #[inline(always)]
    pub fn finish_composition(&mut self) { self.core_mut().finish_composition(); }
    #[inline(always)]
    pub fn paste(&mut self, text: impl Into<String>) { self.core_mut().paste(text.into()); }
    #[inline(always)]
    pub fn paste_content(&mut self, content: crate::ClipboardContent) { self.core_mut().paste_content(content); }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn accepts_paste(&self) -> bool { self.core().accepts_paste() }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn paste_target(&self) -> Option<String> { self.core().paste_target() }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn paste_forms(&self, target: Option<&str>) -> crate::clipboard::Forms { self.core().paste_forms(target) }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn paste_result(&mut self, target: Option<String>, content: crate::ClipboardContent) { self.core_mut().paste_result(target, content); }
    #[inline(always)]
    pub fn has_secure_focus(&self) -> bool { self.core().has_secure_focus() }
    #[inline(always)]
    pub fn has_focus(&self) -> bool { self.core().has_focus() }

    #[inline(always)]
    pub fn set_modifier_keys(&mut self, enabled: bool) { self.core_mut().set_modifier_keys(enabled); }
    #[inline(always)]
    pub fn modifier_keys(&self) -> bool { self.core().modifier_keys() }

    /// Opt into hardware media transport keys ([`Key::Media`](crate::Key::Media)), matching
    /// [`App::media_keys`](crate::App::media_keys). Off by default so apps without it receive
    /// no media key events.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Key, KeyEvent, MediaKey, Runtime, el};
    /// # fn view(_: &bool, _: &mut Cx<bool>) -> El { el().size(100.0, 100.0) }
    /// let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    /// rt.set_media_keys(true);
    /// rt.on_key = Some(|playing, ev, _| {
    ///     if ev.key == Key::Media(MediaKey::PlayPause) { *playing = !*playing; return true; }
    ///     false
    /// });
    /// rt.frame(0.0);
    /// rt.key(KeyEvent::from(Key::Media(MediaKey::PlayPause)));
    /// rt.frame(0.0);
    /// assert!(rt.state);
    /// ```
    #[inline(always)]
    pub fn set_media_keys(&mut self, enabled: bool) { self.core_mut().set_media_keys(enabled); }

    /// Whether hardware media transport keys ([`Key::Media`](crate::Key::Media)) are enabled
    /// for this runtime.
    ///
    /// ```
    /// # use rune::{Env, Runtime, el};
    /// let mut rt = Runtime::new((), |_, _| el(), (100.0, 100.0), Env::new(true, 1.0));
    /// assert!(!rt.media_keys());
    /// rt.set_media_keys(true);
    /// assert!(rt.media_keys());
    /// ```
    #[inline(always)]
    pub fn media_keys(&self) -> bool { self.core().media_keys() }

    /// Queue a key press carrying its physical key code ([`Tx::physical_key`](crate::Tx::physical_key))
    /// and unmodified layout character ([`Tx::key_base`](crate::Tx::key_base)), as a window host
    /// delivers a key press. Use [`Self::physical_key_input`] to also send auto-repeat or release
    /// physical events.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Key, KeyEvent, Keybind, Mods, PhysicalKeyCode, Runtime, col};
    /// # fn view(_: &bool, _: &mut Cx<bool>) -> El { col().size(100.0, 100.0) }
    /// let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    /// rt.on_key = Some(|matched, ev, tx| {
    ///     *matched = Keybind::new("j").matches_tx(ev, tx);
    ///     true
    /// });
    /// rt.frame(0.0);
    /// rt.key_with(
    ///     KeyEvent { key: Key::Text("о".into()), mods: Mods::default() },
    ///     PhysicalKeyCode::KeyJ,
    ///     Some('j'),
    /// );
    /// rt.frame(0.0);
    /// assert!(rt.state);
    /// ```
    #[inline(always)]
    pub fn key_with(&mut self, event: KeyEvent, physical: crate::PhysicalKeyCode, base: Option<char>) {
        let input = PhysicalInput::Key { key: physical, mods: event.mods, pressed: true, repeat: false };
        self.physical_key_input(input, event, base);
    }

    #[inline(always)]
    pub fn physical_input(&mut self, event: PhysicalInput) { self.core_mut().physical_input(event); }

    /// Queue a physical keyboard event paired with its logical [`KeyEvent`] and unmodified layout
    /// character ([`Tx::key_base`](crate::Tx::key_base)), as a native host delivers presses,
    /// repeats and releases. Focused [`El::on_physical_input`](crate::El::on_physical_input)
    /// handlers see `event` first; if unconsumed, `key` routes to key handlers (or `on_key_up`
    /// when `pressed` is false) with `tx.physical_key()` and `tx.key_base()` set.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode, Runtime, col};
    /// # fn view(_: &bool, _: &mut Cx<bool>) -> El { col().size(100.0, 100.0) }
    /// let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    /// rt.on_key = Some(|repeated, _, tx| { *repeated = tx.key_repeat(); true });
    /// rt.frame(0.0);
    /// let mods = Mods::default();
    /// rt.physical_key_input(
    ///     PhysicalInput::Key { key: PhysicalKeyCode::KeyJ, mods, pressed: true, repeat: true },
    ///     KeyEvent { key: Key::Text("j".into()), mods },
    ///     Some('j'),
    /// );
    /// rt.frame(0.0);
    /// assert!(rt.state);
    /// ```
    #[inline(always)]
    pub fn physical_key_input(&mut self, event: PhysicalInput, key: KeyEvent, base: Option<char>) {
        let (core, mut app) = self.split_mut();
        core.physical_key_input(&mut app, event, key, base);
    }
    #[cfg_attr(any(target_os = "windows", target_os = "ios", target_os = "android"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn physical_with_fallback(&mut self, event: PhysicalInput, key: Option<KeyEvent>) -> bool {
        let (core, mut app) = self.split_mut();
        core.physical_with_fallback(&mut app, event, key)
    }
    #[inline(always)]
    pub(crate) fn physical_then(&mut self, event: PhysicalInput, key: Option<KeyEvent>, base: Option<char>, unhandled: Option<Unhandled>) -> bool {
        let (core, mut app) = self.split_mut();
        core.physical_then(&mut app, event, key, base, unhandled)
    }
    #[inline(always)]
    pub(crate) fn physical_observation(&mut self, event: PhysicalInput) -> bool {
        let (core, mut app) = self.split_mut();
        core.physical_observation(&mut app, event)
    }

    // ── Drag & drop, drag-out, help, titlebar, popup, file dialog & selection ─────────────────

    #[inline(always)]
    pub fn drag_files(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) { self.core_mut().drag_files(paths, at); }
    #[inline(always)]
    pub fn drag_files_to(&mut self, at: Option<(f32, f32)>) -> bool { self.core_mut().drag_files_to(at) }
    #[inline(always)]
    pub fn drag_files_leave(&mut self) { self.core_mut().drag_files_leave(); }
    #[inline(always)]
    pub fn drop_files(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) { self.core_mut().drop_files(paths, at); }
    #[inline(always)]
    pub fn dragging_files(&self) -> bool { self.core().dragging_files() }
    #[inline(always)]
    pub fn drop_hovered(&self) -> Option<String> { self.core().drop_hovered() }
    #[inline(always)]
    pub fn drop_rejected_hovered(&self) -> Option<String> { self.core().drop_rejected_hovered() }

    #[inline(always)]
    pub(crate) fn take_drag_out(&mut self) -> Option<crate::drag_out::DragOutStart> { self.core_mut().take_drag_out() }
    #[inline(always)]
    pub(crate) fn drag_end_sink(&self) -> crate::drag_out::DragEnds { self.core().drag_end_sink() }
    #[inline(always)]
    pub(crate) fn drag_ends_pending(&self) -> bool { self.core().drag_ends_pending() }
    #[inline(always)]
    pub(crate) fn set_os_drag(&mut self, supported: bool) { self.core_mut().set_os_drag(supported); }

    #[cfg(target_os = "macos")]
    #[inline(always)]
    pub(crate) fn set_native_help(&mut self, on: bool) { self.core_mut().set_native_help(on); }
    #[inline(always)]
    pub fn help_regions(&self) -> &[([f32; 4], String)] { self.core().help_regions() }
    #[inline(always)]
    pub fn help_tooltip(&self) -> Option<(&str, [f32; 4])> { self.core().help_tooltip() }

    #[inline(always)]
    pub fn set_native_titlebar_band(&mut self, points: f32) { self.core_mut().set_native_titlebar_band(points); }
    #[inline(always)]
    pub fn take_titlebar_action(&mut self) -> Option<TitlebarAction> { self.core_mut().take_titlebar_action() }
    /// A window resize edge requested by the press just delivered on an
    /// [`El::window_resize`](crate::El::window_resize) region. The shell must act on it
    /// before returning from that press event.
    ///
    /// ```no_run
    /// # fn handle(rt: &mut rune::Runtime<()>, window: &winit::window::Window) {
    /// if let Some(edge) = rt.take_resize_action() {
    ///     let _ = window.drag_resize_window(edge.into());
    /// }
    /// # }
    /// ```
    #[inline(always)]
    pub fn take_resize_action(&mut self) -> Option<crate::ResizeEdge> { self.core_mut().take_resize_action() }
    #[inline(always)]
    pub fn set_chrome_insets(&mut self, insets: crate::ChromeInsets) { self.core_mut().set_chrome_insets(insets); }
    #[inline(always)]
    pub fn chrome_insets(&self) -> crate::ChromeInsets { self.core().chrome_insets() }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn set_caption(&mut self, caption: Option<crate::titlebar::Caption>) { self.core_mut().set_caption(caption); }
    #[inline(always)]
    pub(crate) fn headless_chrome(&mut self, chrome: crate::Chrome, titlebar: Option<crate::Titlebar>, borderless: bool) -> crate::Chrome {
        self.core_mut().headless_chrome(chrome, titlebar, borderless)
    }
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    #[inline(always)]
    pub(crate) fn take_toggle_maximized(&mut self) -> bool { self.core_mut().take_toggle_maximized() }
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    #[inline(always)]
    pub(crate) fn take_full_screen_request(&mut self) -> Option<crate::el::FullScreenRequest> { self.core_mut().take_full_screen_request() }
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    #[inline(always)]
    pub(crate) fn take_window_menu(&mut self) -> Option<WindowMenu> { self.core_mut().take_window_menu() }
    #[inline(always)]
    pub(crate) fn titlebar_set(&self) -> crate::titlebar::TitlebarSet { self.core().titlebar_set() }
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    #[inline(always)]
    pub(crate) fn take_titlebar_changed(&mut self) -> bool { self.core_mut().take_titlebar_changed() }

    #[inline(always)]
    pub fn popup_menu(&self) -> Option<&PopupMenu> { self.core().popup_menu() }
    #[inline(always)]
    pub fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), PopupMenuError> {
        let (core, mut app) = self.split_mut();
        core.resolve_popup(&mut app, picked)
    }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn take_popup(&mut self) -> Option<Option<Show>> { self.core_mut().take_popup() }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn popup_waits_for_layout(&self) -> bool { self.core().popup_waits_for_layout() }
    #[inline(always)]
    pub(crate) fn cancel_popups(&mut self) {
        let (core, mut app) = self.split_mut();
        core.cancel_popups(&mut app);
    }
    #[inline(always)]
    pub(crate) fn open_popup_headless(&mut self) {
        let (core, mut app) = self.split_mut();
        core.open_popup_headless(&mut app);
    }

    #[inline(always)]
    pub fn file_dialog(&self) -> Option<(FileDialogKind, &FileDialogOptions)> { self.core().file_dialog() }
    /// True when the pending file dialog allows selecting multiple files ([`Tx::open_files`]).
    ///
    /// ```no_run
    /// # fn check(rt: &rune::Runtime<()>) {
    /// assert!(rt.file_dialog_multiple());
    /// # }
    /// ```
    #[inline(always)]
    pub fn file_dialog_multiple(&self) -> bool { self.core().file_dialog_multiple() }
    #[inline(always)]
    pub fn resolve_file_dialog(&mut self, path: Option<PathBuf>) -> Result<(), FileDialogError> {
        self.core_mut().resolve_file_dialog(path)
    }
    /// Answers the open file dialog as if the user chose `paths`, or cancelled with `None`.
    /// For a single-file dialog, the first path (if any) is delivered. Its callback runs on
    /// the next frame. For tests and headless drivers; native dialogs answer themselves.
    ///
    /// ```no_run
    /// # fn answer(rt: &mut rune::Runtime<()>) {
    /// rt.resolve_file_dialog_files(Some(vec!["/a.csv".into(), "/b.csv".into()])).unwrap();
    /// rt.frame(1.0 / 60.0);
    /// # }
    /// ```
    #[inline(always)]
    pub fn resolve_file_dialog_files(&mut self, paths: Option<Vec<PathBuf>>) -> Result<(), FileDialogError> {
        self.core_mut().resolve_file_dialog_files(paths)
    }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn take_file_dialog(&mut self) -> Option<(u64, FileDialogKind, bool, FileDialogOptions)> {
        self.core_mut().take_file_dialog()
    }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn answer_file_dialog(&mut self, id: u64, result: DialogOutcome) {
        self.core_mut().answer_file_dialog(id, result);
    }
    #[inline(always)]
    pub(crate) fn refuse_file_dialog(&mut self, why: &str) { self.core_mut().refuse_file_dialog(why); }
    #[inline(always)]
    pub(crate) fn cancel_file_dialogs(&mut self) {
        let (core, mut app) = self.split_mut();
        core.cancel_file_dialogs(&mut app);
    }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn file_dialog_json(&self) -> Option<serde_json::Value> { self.core().file_dialog_json() }

    #[inline(always)]
    pub fn text_selection(&self) -> Option<(&str, std::ops::Range<usize>)> { self.core().text_selection() }
    #[inline(always)]
    pub fn selected_text(&self) -> Option<String> {
        let (core, app) = self.split();
        core.selected_text(&app)
    }
    #[inline(always)]
    pub fn selection(&self) -> Option<TextSelection> {
        let (core, app) = self.split();
        core.selection(&app)
    }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn copy_text(&self) -> Option<String> {
        let (core, app) = self.split();
        core.copy_text(&app)
    }
}
