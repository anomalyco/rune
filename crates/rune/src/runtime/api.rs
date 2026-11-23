//! Thin `#[inline(always)]` public and crate-internal forwarders on [`Runtime<S>`] delegating
//! to non-generic [`RuntimeCore`] (frame, lifecycle, settings, layout, elements, motion, a11y,
//! and web). Input, scroll, and selection forwarders live in `runtime/input/api.rs`.

use super::*;

#[allow(dead_code)]
impl<S: 'static> Runtime<S> {
    // ── Update ─────────────────────────────────────────────────────────────────────────────────

    /// Changes the state from outside any handler, for data another thread delivered
    /// (`AppHandle::update`). Runs now, so it works while no window shows; the next frame commits
    /// it with the motion `f` asked for.
    #[inline(always)]
    pub fn update(&mut self, f: impl FnOnce(&mut S, &mut Tx)) {
        let (core, mut app) = self.split_mut();
        let (mut one, stall) = core.begin_update(&app);
        f(app.state, &mut one);
        core.finish_update(&mut app, stall, one, true, false);
    }

    /// `update` for a change that may be no change: `f` returns whether it changed state the
    /// view reads, as `poll` does. Returns whether the last frame went out of date.
    #[inline(always)]
    pub fn update_if(&mut self, f: impl FnOnce(&mut S, &mut Tx) -> bool) -> bool {
        self.update_inner(f, false).0
    }

    /// `update_if` for an update a window drain runs, returning `(stale, minimize_order)`.
    #[inline(always)]
    pub(crate) fn update_if_ordered(&mut self, f: impl FnOnce(&mut S, &mut Tx) -> bool) -> (bool, Option<std::num::NonZeroU64>) {
        self.update_inner(f, true)
    }

    #[inline(always)]
    fn update_inner(&mut self, f: impl FnOnce(&mut S, &mut Tx) -> bool, ordered: bool) -> (bool, Option<std::num::NonZeroU64>) {
        let (core, mut app) = self.split_mut();
        let (mut one, stall) = core.begin_update(&app);
        let changed = f(app.state, &mut one);
        core.finish_update(&mut app, stall, one, changed, ordered)
    }

    // ── Frame & lifecycle ──────────────────────────────────────────────────────────────────────

    /// Advances animation time by `dt` real seconds, runs queued handlers, re-runs the view only
    /// if state changed, steps `Scene`, growth, scroll and reveals, and builds this frame's
    /// instances.
    #[inline(always)]
    pub fn frame(&mut self, dt: f64) -> bool {
        let (core, mut app) = self.split_mut();
        core.frame(&mut app, dt)
    }

    #[inline(always)]
    pub(crate) fn frame_split(&mut self, catch_up: f64, dt: f64) -> bool {
        let (core, mut app) = self.split_mut();
        core.frame_split(&mut app, catch_up, dt)
    }

    #[inline(always)]
    pub(crate) fn hidden_timer_frame(&mut self) {
        let (core, mut app) = self.split_mut();
        core.hidden_timer_frame(&mut app);
    }

    #[inline(always)]
    pub(crate) fn build_for_show(&mut self) -> bool {
        let (core, mut app) = self.split_mut();
        core.build_for_show(&mut app)
    }

    #[inline(always)]
    pub(crate) fn hidden_wake(&mut self) -> bool {
        let (core, mut app) = self.split_mut();
        core.hidden_wake(&mut app)
    }

    #[inline(always)]
    pub(crate) fn covered_wake(&mut self) -> bool {
        let (core, mut app) = self.split_mut();
        core.covered_wake(&mut app)
    }

    /// Queue a native focus transition. Repeated notifications do not repeat the callback.
    #[inline(always)]
    pub fn window_focus(&mut self, focused: bool) {
        let (core, mut app) = self.split_mut();
        core.window_focus(&mut app, focused);
    }

    #[inline(always)]
    pub(crate) fn keys_lost(&mut self) { self.core_mut().keys_lost(); }
    #[inline(always)]
    pub(crate) fn keys_queued(&self) -> bool { self.core().keys_queued() }
    #[inline(always)]
    pub(crate) fn needs_commit(&self) -> bool { self.core().needs_commit() }
    #[inline(always)]
    pub(crate) fn stale(&self) -> bool { self.core().stale() }
    #[inline(always)]
    pub(crate) fn patch_pending(&self) -> bool { self.core().patch_pending() }

    /// Marks app state changed outside a handler (the next frame re-runs the view).
    #[inline(always)]
    pub fn invalidate(&mut self) { self.core_mut().invalidate(); }

    /// Lands every track before the next frame: growth, reveals and follow-the-end.
    #[inline(always)]
    pub fn settle_all(&mut self) { self.core_mut().settle_all(); }

    #[inline(always)]
    pub(crate) fn ignore_minimize(&mut self, what: &str) { self.core_mut().ignore_minimize(what); }
    #[inline(always)]
    pub(crate) fn minimize_step(&mut self, event: crate::window::MinimizeEvent) -> bool { self.core_mut().minimize_step(event) }
    #[inline(always)]
    pub(crate) fn schedule_pending_wakes(&mut self) { self.core_mut().schedule_pending_wakes(); }

    /// Next requested wake on the unscaled app clock. None lets the shell sleep indefinitely.
    #[inline(always)]
    pub fn wake_deadline(&self) -> Option<f64> { self.core().wake_deadline() }

    /// The native window or overlay's frame counts; `None` headless.
    #[inline(always)]
    pub fn surface_stats(&self) -> Option<crate::SurfaceStats> { self.core().surface_stats() }

    #[cfg(any(test, feature = "frame-profile"))]
    #[inline(always)]
    pub(crate) fn polled_at(&self) -> f64 { self.core().polled_at() }
    #[cfg(feature = "frame-profile")]
    #[inline(always)]
    pub(crate) fn poll_wake_deadline(&self) -> Option<f64> { self.core().poll_wake_deadline() }
    #[cfg(feature = "frame-profile")]
    #[inline(always)]
    pub(crate) fn motion_wake_deadline(&self) -> Option<f64> { self.core().motion_wake_deadline() }

    #[inline(always)]
    pub fn tracing(&self) -> bool { self.core().tracing() }

    /// A scalar from the actual draw path (e.g. `shell.width`).
    #[inline(always)]
    pub fn trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        self.core_mut().trace_channel(name, kind, value);
    }

    /// Queues an app marker once for each attached writer. Formatting is skipped when off.
    #[inline(always)]
    pub fn trace_event(&mut self, event: impl std::fmt::Display) {
        self.core_mut().trace_event_dyn(&event);
    }

    #[cfg(feature = "frame-profile")]
    #[inline(always)]
    pub(crate) fn fp_loops(&self) -> Vec<String> { self.core().fp_loops() }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn loops_shown(&self) -> bool { self.core().loops_shown() }

    // ── Viewport, zoom & window requests ───────────────────────────────────────────────────────

    /// The window's size in its points and its scale.
    #[inline(always)]
    pub fn set_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        self.core_mut().set_viewport(viewport, scale);
    }

    #[inline(always)]
    pub(crate) fn step_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        self.core_mut().step_viewport(viewport, scale);
    }

    /// The view's UI zoom from the last commit (`Cx::zoom`), 1 when unzoomed.
    #[inline(always)]
    pub fn zoom(&self) -> f32 { self.core().host.zoom() }

    /// The window's size in its own points, before the view's zoom.
    #[inline(always)]
    pub fn window_viewport(&self) -> (f32, f32) { self.core().host.window_viewport() }

    /// The window's scale, before the view's zoom.
    #[inline(always)]
    pub fn window_scale(&self) -> f32 { self.core().host.window_scale() }

    /// Physical pixels per view point: what a host divides physical pointer coordinates by.
    #[inline(always)]
    pub fn input_scale(&self) -> f32 { self.core().input_scale() }

    /// A rect `[x, y, w, h]` from view points to window points.
    #[inline(always)]
    pub fn to_window(&self, r: [f32; 4]) -> [f32; 4] { self.core().host.to_window(r) }

    /// The root's natural height at the current width, in window points.
    #[inline(always)]
    pub fn content_height(&mut self) -> f32 {
        let (core, app) = self.split_mut();
        core.content_height(&app)
    }

    #[inline(always)]
    pub(crate) fn set_window_origin(&mut self, origin: (f32, f32)) -> bool {
        self.core_mut().set_window_origin(origin)
    }

    // ── System settings, fonts & blend ─────────────────────────────────────────────────────────

    #[inline(always)]
    pub fn set_system_dark(&mut self, dark: bool) { self.core_mut().set_system_dark(dark); }
    #[inline(always)]
    pub fn set_titlebar_inset(&mut self, points: f32) { self.core_mut().set_titlebar_inset(points); }
    #[inline(always)]
    pub(crate) fn system_accent(&self) -> crate::SystemAccent { self.core().system_accent() }
    #[inline(always)]
    pub fn set_system_accent(&mut self, accent: crate::SystemAccent) { self.core_mut().set_system_accent(accent); }
    #[inline(always)]
    pub fn set_full_keyboard_access(&mut self, on: bool) { self.core_mut().set_full_keyboard_access(on); }
    #[inline(always)]
    pub fn set_tab_visits_controls(&mut self, on: bool) { self.core_mut().set_tab_visits_controls(on); }
    #[inline(always)]
    pub fn set_system_contrast(&mut self, contrast: Option<crate::SystemContrast>) { self.core_mut().set_system_contrast(contrast); }
    #[inline(always)]
    pub fn set_system_increased_contrast(&mut self, on: bool) { self.core_mut().set_system_increased_contrast(on); }
    #[inline(always)]
    pub fn set_system_reduced_motion(&mut self, reduced: bool) { self.core_mut().set_system_reduced_motion(reduced); }
    #[inline(always)]
    pub fn system_reduced_motion(&self) -> bool { self.core().host.system_reduced_motion() }
    #[inline(always)]
    pub(crate) fn system_scroll_bars_state(&self) -> crate::platform::scroll_bars::SystemScrollBars { self.core().system_scroll_bars_state() }
    #[inline(always)]
    pub(crate) fn set_system_scroll_bars_state(&mut self, next: crate::platform::scroll_bars::SystemScrollBars) { self.core_mut().set_system_scroll_bars_state(next); }
    #[inline(always)]
    pub fn set_system_scroll_bars(&mut self, bars: crate::ScrollBars) { self.core_mut().set_system_scroll_bars(bars); }
    #[inline(always)]
    pub fn system_scroll_bars(&self) -> crate::ScrollBars { self.core().host.system_scroll_bars() }
    #[inline(always)]
    pub fn set_system_scroller_overlay(&mut self, overlay: bool) { self.core_mut().set_system_scroller_overlay(overlay); }
    #[inline(always)]
    pub fn system_scroller_overlay(&self) -> bool { self.core().host.system_scroller_overlay() }
    #[inline(always)]
    pub fn set_system_scroller_jumps(&mut self, jumps: bool) { self.core_mut().set_system_scroller_jumps(jumps); }
    #[inline(always)]
    pub fn system_scroller_jumps(&self) -> bool { self.core().host.system_scroller_jumps() }
    #[inline(always)]
    pub fn overlay_scroll_bars(&self) -> bool { self.core().host.overlay_scroll_bars() }

    #[inline(always)]
    pub fn set_font_families(&mut self, sans: &str, mono: &str) { self.core_mut().set_font_families(sans, mono); }
    #[inline(always)]
    pub fn font_families(&self) -> (&str, &str) { self.core().font_families() }
    #[inline(always)]
    pub(crate) fn follow_font_families(&mut self) -> bool { self.core_mut().follow_font_families() }

    #[inline(always)]
    pub fn set_srgb_blend(&mut self, enabled: bool) { self.core_mut().set_srgb_blend(enabled); }
    #[inline(always)]
    pub fn srgb_blend(&self) -> bool { self.core().srgb_blend() }
    #[inline(always)]
    pub fn fade_stops(&self) -> [f32; 2] { self.core().fade_stops() }
    #[inline(always)]
    pub fn set_font_smoothing(&mut self, mode: crate::FontSmoothing) { self.core_mut().set_font_smoothing(mode); }
    #[inline(always)]
    pub fn font_smoothing(&self) -> crate::FontSmoothing { self.core().font_smoothing() }
    #[inline(always)]
    pub fn clear_color(&self) -> [f32; 4] { self.core().clear_color() }

    // ── Elements & inspection ──────────────────────────────────────────────────────────────────

    #[inline(always)]
    pub fn element_is_secure(&self, path: &str) -> bool { self.core().tree.element_is_secure(path) }
    #[inline(always)]
    pub fn elements(&self) -> Vec<ElementInfo> { self.core().elements() }
    #[inline(always)]
    pub fn find(&self, key: &str) -> Option<(f32, f32)> { self.core().find(key) }
    #[inline(always)]
    pub fn presented(&self, key: &str) -> Option<([f32; 4], f32)> { self.core().presented(key) }
    #[inline(always)]
    pub fn element_at(&self, x: f32, y: f32) -> Option<Picked> { self.core().element_at(x, y) }
    #[inline(always)]
    pub fn element_at_with(&self, x: f32, y: f32, pick: Pick) -> Option<Picked> { self.core().element_at_with(x, y, pick) }
    #[inline(always)]
    pub fn element_at_in(&self, within: &str, x: f32, y: f32) -> Option<Picked> { self.core().element_at_in(within, x, y) }
    #[inline(always)]
    pub fn element_at_in_with(&self, within: &str, x: f32, y: f32, pick: Pick) -> Option<Picked> { self.core().element_at_in_with(within, x, y, pick) }
    #[inline(always)]
    pub fn contains(&self, key: &str, x: f32, y: f32) -> bool { self.core().contains(key, x, y) }
    #[inline(always)]
    pub fn contains_with(&self, key: &str, x: f32, y: f32, pick: Pick) -> bool { self.core().contains_with(key, x, y, pick) }
    #[inline(always)]
    pub fn presented_pose(&self, key: &str) -> Option<Pose> { self.core().presented_pose(key) }

    #[inline(always)]
    pub(crate) fn backdrop_dark(&self) -> Option<bool> { self.core().backdrop_dark() }

    // ── Motion, live, timers, paint, clipboard, memo & URLs ────────────────────────────────────

    #[inline(always)]
    pub fn live_only_events(&self) -> u64 { self.core().live_only_events() }
    #[inline(always)]
    pub fn check_live_only(&mut self, on: bool) { self.core_mut().check_live_only(on); }
    #[inline(always)]
    pub fn live_only_warnings(&self) -> &[String] { self.core().live_only_warnings() }

    #[inline(always)]
    pub(crate) fn live_wake(&self) -> &Arc<crate::live::Wake> { self.core().live_wake() }
    #[inline(always)]
    pub fn set_live_waker(&mut self, waker: Waker) { self.core_mut().set_live_waker(waker); }
    #[inline(always)]
    pub fn set_live_visible(&mut self, visible: bool) {
        let (core, mut app) = self.split_mut();
        core.set_live_visible(&mut app, visible);
    }
    #[inline(always)]
    pub(crate) fn live_notifier(&self) -> impl FnOnce() + Send + 'static { self.core().live_notifier() }
    #[inline(always)]
    pub(crate) fn live_visible(&self) -> bool { self.core().live_visible() }
    #[inline(always)]
    pub fn live_pending(&self) -> bool { self.core().live_pending() }

    #[inline(always)]
    pub fn motion_elements(&self, out: &mut Vec<MotionElement>) { self.core().motion_elements(out); }
    #[inline(always)]
    pub fn element_path(&self, id: NodeId) -> Option<(&str, Option<&str>)> { self.core().element_path(id) }
    #[inline(always)]
    pub fn rewind(&self) -> Option<&crate::rewind::Rewind> { self.core().motion.rewind() }

    #[inline(always)]
    pub(crate) fn poll_hidden(&mut self) -> bool {
        let (core, mut app) = self.split_mut();
        core.poll_hidden(&mut app)
    }
    #[inline(always)]
    pub(crate) fn resting_wake(&mut self) -> Option<bool> {
        let (core, mut app) = self.split_mut();
        core.resting_wake(&mut app)
    }
    #[inline(always)]
    pub(crate) fn resting_wake_frame(&mut self, frame: bool) { self.core_mut().resting_wake_frame(frame); }
    #[inline(always)]
    pub(crate) fn cancel_timer_frame(&mut self) { self.core_mut().cancel_timer_frame(); }
    #[inline(always)]
    pub(crate) fn rests(&self) -> bool { self.core().rests() }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn cycle_paint(&self) -> (u64, Vec<u32>) { self.core().cycle_paint() }
    #[inline(always)]
    pub(crate) fn paint_count(&self) -> u64 { self.core().paint_count() }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn paint_cycle_tick(&mut self) -> bool { self.core_mut().paint_cycle_tick() }
    #[inline(always)]
    pub(crate) fn loop_step(&mut self, dt: f64) -> Result<Vec<u32>, &'static str> { self.core_mut().loop_step(dt) }
    #[inline(always)]
    pub(crate) fn loop_check_on(&self) -> bool { self.core().loop_check_on() }
    #[inline(always)]
    pub(crate) fn loop_checked(&mut self) -> Option<(usize, f32)> { self.core_mut().loop_checked() }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn paints(&self) -> HashMap<(String, bool), crate::motion_gate::check::paint::Paint> { self.core().paints() }
    #[inline(always)]
    pub(crate) fn played_keys(&self) -> u64 {
        match self.hooks.playground.as_deref() {
            Some(p) => p.played,
            None => 0,
        }
    }
    #[inline(always)]
    pub(crate) fn interaction_presented(&mut self, submitted: crate::time::Instant, presents_at: Option<crate::time::Instant>) {
        self.core_mut().interaction_presented(submitted, presents_at);
    }
    #[inline(always)]
    pub(crate) fn interaction_flush(&mut self) { self.core_mut().interaction_flush(); }

    /// Use a shared in-memory clipboard (`clipboard::memory()`) instead of the OS one.
    #[inline(always)]
    pub fn use_clipboard(&mut self, clipboard: &crate::clipboard::MemoryClipboard) {
        self.core_mut().use_clipboard(clipboard);
    }

    /// Drops this window's cached subtrees whose memo id starts with `prefix`, then re-runs the view.
    #[inline(always)]
    pub fn invalidate_memos_by_prefix(&mut self, prefix: &str) {
        self.core_mut().invalidate_memos_by_prefix(prefix);
    }

    /// Takes the links handlers asked to open that this runtime didn't open.
    #[inline(always)]
    pub fn take_url_requests(&mut self) -> Vec<String> { self.core_mut().host.take_url_requests() }

    #[cfg(feature = "hot")]
    #[inline(always)]
    pub(crate) fn hot_scrolls(&self) -> Vec<(String, f64)> { self.core().hot_scrolls() }
    #[cfg(feature = "hot")]
    #[inline(always)]
    pub(crate) fn hot_motion(&self) -> Vec<(String, [f32; 4], [f32; 4])> { self.core().tree.hot_motion(&self.core().scene) }

    // ── Accessibility & web ────────────────────────────────────────────────────────────────────

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn a11y_action(&mut self, request: &accesskit::ActionRequest) -> bool { self.core_mut().a11y_action(request) }
    #[inline(always)]
    pub fn accessibility_action(&mut self, key: &str, action: accesskit::Action, data: Option<accesskit::ActionData>) -> bool {
        self.core_mut().accessibility_action(key, action, data)
    }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn a11y_activate(&mut self, name: &str) { self.core_mut().a11y_activate(name); }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn a11y_deactivate(&mut self) { self.core_mut().a11y_deactivate(); }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn a11y_changes(&self) -> crate::a11y::tree::Changes { self.core().a11y_changes() }
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn a11y_update(&mut self) -> Option<accesskit::TreeUpdate> { self.core_mut().a11y_update() }
    #[inline(always)]
    pub fn accessibility_tree(&self) -> accesskit::TreeUpdate { self.core().accessibility_tree() }
    #[inline(always)]
    pub fn accessibility_json(&self) -> serde_json::Value { self.core().accessibility_json() }
    #[inline(always)]
    pub fn a11y_json(&self) -> String { self.core().a11y_json() }

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn web_a11y_items(&self) -> Vec<web_a11y::A11yItem> { self.core().web_a11y_items() }
    #[cfg(target_arch = "wasm32")]
    #[inline(always)]
    pub(crate) fn page_scroll_x(&self) -> Vec<(String, f32)> { self.core().page_scroll_x() }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn web_a11y_scrolled_only(&self, mirror: &web_a11y::A11yMirror) -> bool { self.core().web_a11y_scrolled_only(mirror) }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_a11y<'a>(&self, mirror: &'a mut web_a11y::A11yMirror) -> Option<&'a str> { self.core().web_a11y(mirror) }
    #[cfg(target_arch = "wasm32")]
    #[inline(always)]
    pub(crate) fn web_a11y_focus<'a>(&self, mirror: &'a mut web_a11y::A11yMirror) -> Option<&'a str> { self.core().web_a11y_focus(mirror) }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_a11y_action(&mut self, event: &serde_json::Value) -> bool { self.core_mut().web_a11y_action(event) }
    #[inline(always)]
    pub(crate) fn export_nodes(&self) -> Vec<web_export::ExportNode> { self.core().export_nodes() }

    #[inline(always)]
    pub(crate) fn page_mode(&mut self) { self.core_mut().page_mode(); }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn page_height(&self) -> f32 { self.core().page_height() }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn set_band(&mut self, band: f32, view: f32) -> bool { self.core_mut().set_band(band, view) }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn set_page_scroll(&mut self, y: f32) -> bool { self.core_mut().set_page_scroll(y) }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn page_scroll_y(&self) -> f32 { self.core().page_scroll_y() }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn page_window(&self) -> (f32, f32) { self.core().page_window() }
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn set_page_scroll_x(&mut self, path: &str, x: f32) -> bool { self.core_mut().set_page_scroll_x(path, x) }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn page_offset(&self) -> f32 { self.core().page_offset() }

    #[cfg(target_arch = "wasm32")]
    #[inline(always)]
    pub(crate) fn web_release_needs_frame(&self) -> bool { self.core().web_release_needs_frame() }
    #[cfg(target_arch = "wasm32")]
    #[inline(always)]
    pub(crate) fn web_focus_field(&mut self) -> bool {
        let (core, mut app) = self.split_mut();
        core.web_focus_field(&mut app)
    }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_edit_context(&mut self, mirror: &mut web_runtime::Mirror) -> Option<bool> {
        let (core, mut app) = self.split_mut();
        core.web_edit_context(&mut app, mirror)
    }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_context(&self, mirror: &mut web_runtime::Mirror) -> bool { self.core().web_context(mirror) }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_key(&mut self, event: &serde_json::Value) -> bool {
        let (core, mut app) = self.split_mut();
        core.web_key(&mut app, event)
    }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_secure(&mut self, key: &str, text: String) { self.core_mut().web_secure(key, text); }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_secure_replace(&mut self, key: &str, text: String) {
        let (core, mut app) = self.split_mut();
        core.web_secure_replace(&mut app, key, text);
    }
    #[cfg(any(test, target_arch = "wasm32"))]
    #[inline(always)]
    pub(crate) fn web_edit(&mut self, event: &serde_json::Value, mirror: &mut web_runtime::Mirror) {
        let (core, mut app) = self.split_mut();
        core.web_edit(&mut app, event, mirror);
    }

    // ── Test-only helpers called directly on `Runtime<S>` inside unit tests ───────────────────

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn set_focus(&mut self, path: Option<String>, keyboard: bool) -> bool {
        let (core, mut app) = self.split_mut();
        core.set_focus(&mut app, path, keyboard)
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn transaction(&self) -> Tx {
        let (core, app) = self.split();
        core.transaction(&app)
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn commit(&mut self, tx: &Tx) {
        let (core, mut app) = self.split_mut();
        core.commit(&mut app, tx);
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn apply(&mut self, measured: Measured, tx: &Tx) {
        let (core, mut app) = self.split_mut();
        core.apply(&mut app, measured, tx);
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn measure(&mut self, tx: &Tx) -> Measured {
        let (core, app) = self.split_mut();
        core.measure(&app, tx)
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn commit_kind(&self, tx: &Tx) -> relayout::CommitKind { self.core().commit_kind(tx) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn build(&mut self) -> bool { self.core_mut().build() }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn capture_text_flight(&mut self, source: String, target: String) {
        let c = self.core_mut();
        c.motion.capture_text_flight(&c.tree.nodes, &mut c.paint, &mut c.text, c.env.scale, c.viewport, c.t, source, target);
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn color(&self, id: NodeId, prop: Prop<Color>, rest: Color) -> Color { self.core().color(id, prop, rest) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn drawn_box(&self, i: usize) -> [f32; 4] { self.core().drawn_box(i) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn focused_key(&self) -> Option<String> { self.core().focused_key() }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn focused_layout(&mut self) -> Option<TextLayout> { self.core_mut().focused_layout() }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn highlight_visible(&self, i: usize) -> bool { self.core().highlight_visible(i) }

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn interaction_tracker(&mut self) -> &mut crate::tooling::interaction::Tracker { self.core_mut().interaction_tracker() }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn is_exiting(&self, id: NodeId) -> bool { self.core().is_exiting(id) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn is_selectable(&self, i: usize) -> bool { self.core().is_selectable(i) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn link_boxes(&self, i: usize, rect: [f32; 4]) -> Vec<(usize, [f32; 4])> { self.core().link_boxes(i, rect) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn node(&self, path: &str) -> Option<&Node> { self.core().node(path) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn page_to(&mut self, i: usize, to: impl FnOnce(f64, f64, f64) -> f64) -> bool { self.core_mut().page_to(i, to) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn pointer_event(&self, i: usize, x: f32, y: f32) -> PointerEvent { self.core().pointer_event(i, x, y) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn prune_exited(&mut self) { self.core_mut().prune_exited(); }

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn record_rewind(&mut self) {
        let c = self.core();
        c.motion.record_rewind(c.clock, c.viewport, &c.tree.nodes, &c.paint);
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn route_key_up(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let (core, mut app) = self.split_mut();
        core.route_key_up(&mut app, ev, tx)
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn schedule_wakes_from(&mut self, now: f64, wakes: impl IntoIterator<Item = std::time::Duration>) {
        self.core_mut().schedule_wakes_from(now, wakes);
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn scroll_at(&self, x: f32, y: f32) -> Option<NodeId> { self.core().scroll_at(x, y) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn scroll_at_axis(&self, x: f32, y: f32, horizontal: bool) -> Option<NodeId> { self.core().scroll_at_axis(x, y, horizontal) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn scroll_viewport(&self, key: &str) -> Option<usize> { self.core().scroll_viewport(key) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn secondary_target(&self, x: f32, y: f32) -> Option<&str> { self.core().secondary_target(x, y) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn selection_menu(&mut self, path: &str, x: f32, y: f32) -> bool {
        let (core, app) = self.split_mut();
        core.selection_menu(&app, path, x, y)
    }

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn sync_selection_pins(&mut self) { self.core_mut().sync_selection_pins(); }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn touch_scroll_start(&mut self, x: f32, y: f32) -> Option<NodeId> { self.core_mut().touch_scroll_start(x, y) }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn touch_scroll_pan(&mut self, x: f32, y: f32, horizontal: bool, vertical: Option<NodeId>) -> (Option<NodeId>, bool) {
        self.core_mut().touch_scroll_pan(x, y, horizontal, vertical)
    }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn touch_scroll_move(&mut self, id: NodeId, d: f32) { self.core_mut().touch_scroll_move(id, d); }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn touch_scroll_end(&mut self, id: Option<NodeId>, velocity: f32) { self.core_mut().touch_scroll_end(id, velocity); }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn update_interaction(&mut self, held: Option<bool>) -> bool { self.core_mut().update_interaction(held) }

    #[cfg(all(test, debug_assertions))]
    #[inline(always)]
    pub(super) fn view_print(&self) -> live_check::ViewPrint { self.core().view_print() }

    #[cfg(test)]
    #[inline(always)]
    pub(super) fn wheel_lost(&mut self) { self.core_mut().wheel_lost(); }

    #[cfg(all(test, target_os = "macos"))]
    #[inline(always)]
    pub(crate) fn take_queued_pointer(&mut self) -> Vec<Option<Drag>> { self.core_mut().take_queued_pointer() }

    #[cfg(all(test, target_os = "macos"))]
    #[inline(always)]
    pub(crate) fn end_click_run(&mut self) { self.core_mut().end_click_run(); }
}
