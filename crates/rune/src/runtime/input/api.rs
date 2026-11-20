//! Thin `#[inline(always)]` public and crate-internal forwarders on [`Runtime<S>`] delegating
//! to non-generic [`RuntimeCore`] for pointer, keyboard, gesture, touch, scroll, drop, popup,
//! file dialog, titlebar, help, and text selection.

use std::path::PathBuf;

use super::*;
#[cfg(test)]
use crate::file_dialog::FileDialogResult;
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
    #[inline(always)]
    pub(crate) fn cancel_pointer(&mut self) { self.core_mut().cancel_pointer(); }
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
    pub(crate) fn pinch_wheel(&mut self, x: f32, y: f32, dy: f32) -> bool { self.core_mut().pinch_wheel(x, y, dy) }
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

    #[inline(always)]
    pub fn physical_input(&mut self, event: PhysicalInput) { self.core_mut().physical_input(event); }
    #[inline(always)]
    pub(crate) fn physical_key_input(&mut self, event: PhysicalInput, key: KeyEvent, base: Option<char>) {
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
    pub(crate) fn take_toggle_full_screen(&mut self) -> bool { self.core_mut().take_toggle_full_screen() }
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
    #[inline(always)]
    pub fn resolve_file_dialog(&mut self, path: Option<PathBuf>) -> Result<(), FileDialogError> {
        self.core_mut().resolve_file_dialog(path)
    }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn take_file_dialog(&mut self) -> Option<(u64, FileDialogKind, FileDialogOptions)> {
        self.core_mut().take_file_dialog()
    }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn answer_file_dialog(&mut self, id: u64, result: FileDialogResult) {
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
