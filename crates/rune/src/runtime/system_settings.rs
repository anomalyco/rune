//! The host's system settings: appearance, accent, scroll bars, titlebar inset, keyboard access
//! and reduced motion.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

pub(super) struct ScrollBarHost {
    pub(super) state: crate::platform::scroll_bars::SystemScrollBars,
    pub(super) runtime_id: u64,
}

impl Default for ScrollBarHost {
    fn default() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            state: Default::default(),
            runtime_id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl window_requests::HostState {
    #[inline]
    pub(super) fn set_titlebar_inset(&mut self, points: f32) -> bool {
        if points == self.titlebar_inset { return false; }
        self.titlebar_inset = points;
        true
    }

    #[inline]
    pub(super) fn set_maximized(&mut self, maximized: bool) -> bool {
        if maximized == self.maximized { return false; }
        self.maximized = maximized;
        true
    }

    #[inline]
    pub(super) fn set_full_screen(&mut self, full_screen: bool) -> bool {
        if full_screen == self.full_screen { return false; }
        self.full_screen = full_screen;
        true
    }

    #[inline]
    pub(super) fn set_window_backdrop(&mut self, backdrop: crate::WindowBackdrop) -> bool {
        if backdrop == self.window_backdrop { return false; }
        self.window_backdrop = backdrop;
        true
    }

    #[inline]
    pub(super) fn set_window_backdrop_supported(&mut self, backdrop: crate::WindowBackdrop, supported: bool) -> bool {
        let bit = backdrop.support_bit();
        if bit == 0 {
            return false;
        }
        let next = if supported { self.window_backdrop_supported | bit } else { self.window_backdrop_supported & !bit };
        if next == self.window_backdrop_supported { return false; }
        self.window_backdrop_supported = next;
        true
    }

    #[inline]
    pub(super) fn set_window_backdrop_supported_mask(&mut self, mask: u8) -> bool {
        if mask == self.window_backdrop_supported { return false; }
        self.window_backdrop_supported = mask;
        true
    }

    #[inline]
    pub(super) fn set_renderer_kind(&mut self, kind: crate::RendererKind) -> bool {
        if kind == self.renderer_kind { return false; }
        self.renderer_kind = kind;
        true
    }

    #[inline]
    pub(super) fn set_backdrop_opaque_fallback(&mut self, fallback: bool) -> bool {
        if fallback == self.backdrop_opaque_fallback { return false; }
        self.backdrop_opaque_fallback = fallback;
        true
    }

    /// What [`Cx::accent`] and [`Cx::focus_color`] report, and the colour of default focus
    /// rings. Native windows set it from [`system_accent`](crate::system_accent) and again when
    /// the user picks another accent; a change re-runs the view. Headless runs keep
    /// [`SystemAccent::DEFAULT`](crate::SystemAccent::DEFAULT) unless a test sets one.
    #[inline]
    pub(crate) fn system_accent(&self) -> crate::SystemAccent {
        self.system_accent
    }

    #[inline]
    pub(super) fn set_system_accent(&mut self, accent: crate::SystemAccent) -> bool {
        if accent == self.system_accent { return false; }
        self.system_accent = accent;
        true
    }

    #[inline]
    pub(super) fn set_system_contrast(&mut self, contrast: Option<crate::SystemContrast>) -> bool {
        if contrast == self.system_contrast { return false; }
        self.system_contrast = contrast;
        true
    }

    #[inline]
    pub(super) fn set_system_increased_contrast(&mut self, on: bool) -> bool {
        if on == self.system_increased_contrast { return false; }
        self.system_increased_contrast = on;
        true
    }

    #[inline]
    pub(super) fn set_system_reduced_transparency(&mut self, reduced: bool) -> bool {
        if self.system_reduced_transparency == reduced { return false; }
        self.system_reduced_transparency = reduced;
        true
    }

    /// Host Reduce Transparency preference; false by default in a headless runtime.
    #[inline]
    pub(crate) fn system_reduced_transparency(&self) -> bool {
        self.system_reduced_transparency
    }

    #[inline]
    pub(super) fn set_system_reduced_motion(&mut self, reduced: bool) -> bool {
        if self.system_reduced_motion == reduced { return false; }
        self.system_reduced_motion = reduced;
        true
    }

    /// Host accessibility preference; false by default in a headless runtime.
    #[inline]
    pub(crate) fn system_reduced_motion(&self) -> bool {
        self.system_reduced_motion
    }

    #[inline]
    pub(super) fn set_system_text_scale(&mut self, scale: f32) -> bool {
        let clean = crate::platform::text_scale::sanitize(scale)
            .unwrap_or(crate::platform::text_scale::DEFAULT);
        if clean.to_bits() == self.system_text_scale.to_bits() { return false; }
        self.system_text_scale = clean;
        self.languages_gen = self.languages_gen.wrapping_add(1);
        true
    }

    /// Host system text-size scale factor; `1.0` by default in a headless runtime.
    #[inline]
    pub(crate) fn system_text_scale(&self) -> f32 {
        self.system_text_scale
    }

    pub(super) fn set_languages(&mut self, languages: Vec<String>) -> bool {
        let normalized = crate::platform::languages::normalize(languages);
        if self.system_languages.as_deref() == Some(normalized.as_slice()) {
            return false;
        }
        self.system_languages = Some(Rc::from(normalized));
        self.languages_gen = self.languages_gen.wrapping_add(1);
        true
    }

    #[inline]
    pub(crate) fn safe_area(&self) -> [f32; 4] {
        self.safe_area.unwrap_or_else(crate::runtime::cx::safe_area_insets)
    }

    pub(super) fn set_safe_area(&mut self, insets: [f32; 4]) -> bool {
        let clean = insets.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        let prev = self.safe_area();
        self.safe_area = Some(clean);
        if prev == clean {
            return false;
        }
        self.languages_gen = self.languages_gen.wrapping_add(1);
        true
    }

    #[inline]
    pub(crate) fn system_scroll_bars_state(&self) -> crate::platform::scroll_bars::SystemScrollBars {
        self.scroll_bars.state
    }

    #[inline]
    pub(super) fn set_system_scroll_bars_state(&mut self, next: crate::platform::scroll_bars::SystemScrollBars) -> bool {
        if next == self.scroll_bars.state { return false; }
        self.scroll_bars.state = next;
        true
    }

    #[inline]
    pub(super) fn set_system_scroll_bars(&mut self, bars: crate::ScrollBars) -> bool {
        self.set_system_scroll_bars_state(crate::platform::scroll_bars::SystemScrollBars {
            bars,
            ..self.scroll_bars.state
        })
    }

    #[inline]
    pub(crate) fn system_scroll_bars(&self) -> crate::ScrollBars {
        self.scroll_bars.state.bars
    }

    #[inline]
    pub(super) fn set_system_scroller_overlay(&mut self, overlay: bool) -> bool {
        self.set_system_scroll_bars_state(crate::platform::scroll_bars::SystemScrollBars {
            scroller_overlay: overlay,
            ..self.scroll_bars.state
        })
    }

    #[inline]
    pub(crate) fn system_scroller_overlay(&self) -> bool {
        self.scroll_bars.state.scroller_overlay
    }

    #[inline]
    pub(super) fn set_system_scroller_jumps(&mut self, jumps: bool) -> bool {
        self.set_system_scroll_bars_state(crate::platform::scroll_bars::SystemScrollBars {
            jump_to_click: jumps,
            ..self.scroll_bars.state
        })
    }

    #[inline]
    pub(crate) fn system_scroller_jumps(&self) -> bool {
        self.scroll_bars.state.jump_to_click
    }

    /// Whether scroll bars currently overlay and fade out at rest (`true` for
    /// [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling), `false` for
    /// [`ScrollBars::Always`](crate::ScrollBars::Always), and [`Self::system_scroller_overlay`]
    /// for [`ScrollBars::Automatic`](crate::ScrollBars::Automatic); always `true` on iOS).
    #[inline]
    pub(crate) fn overlay_scroll_bars(&self) -> bool {
        self.scroll_bars.state.is_overlay()
    }

    #[inline]
    pub(super) fn set_window_manager_state(&mut self, state: crate::WindowManagerState) -> bool {
        if state == self.wm_state { return false; }
        self.wm_state = state;
        true
    }

    #[inline]
    pub(super) fn set_primary_pointer(&mut self, pointer: cx::PrimaryPointer) -> bool {
        if pointer == self.primary_pointer { return false; }
        self.primary_pointer = pointer;
        self.languages_gen = self.languages_gen.wrapping_add(1);
        true
    }
}

impl RuntimeCore {
    pub fn set_system_dark(&mut self, dark: bool) {
        if dark != self.env.system_dark {
            self.env.system_dark = dark;
            self.commit.dirty = true;
        }
    }

    /// What [`Cx::titlebar_inset`] reports. Native macOS windows set it from AppKit's buttons on
    /// every resize, full screen included; a change re-runs the view.
    #[inline]
    pub fn set_titlebar_inset(&mut self, points: f32) {
        if self.host.set_titlebar_inset(points) { self.commit.dirty = true; }
    }

    /// What [`Cx::is_maximized`] reports. Native windows set it from confirmed OS window state
    /// on create, resize, scale change, and focus; a change re-runs the view and its memos.
    #[inline]
    pub fn set_maximized(&mut self, maximized: bool) {
        if self.host.set_maximized(maximized) { self.commit.dirty = true; }
    }

    /// What [`Cx::is_full_screen`] reports. On macOS this flips at the start of the Space
    /// transition (`WillEnter` / `WillExit`) and reconciles on completion or failure; on Windows
    /// and Linux it updates from confirmed OS window state. A change re-runs the view and its memos.
    #[inline]
    pub fn set_full_screen(&mut self, full_screen: bool) {
        if self.host.set_full_screen(full_screen) { self.commit.dirty = true; }
    }

    /// What [`Cx::window_backdrop`] currently reports.
    #[cfg_attr(not(all(target_os = "windows", feature = "windows-system-material")), allow(dead_code))]
    #[inline]
    pub(crate) fn window_backdrop(&self) -> crate::WindowBackdrop {
        self.host.window_backdrop
    }

    /// What [`Cx::window_backdrop`] reports. Native hosts set it only after the OS confirms the
    /// backdrop is active; a change re-runs the view and its memos.
    #[inline]
    pub fn set_window_backdrop(&mut self, backdrop: crate::WindowBackdrop) {
        if self.host.set_window_backdrop(backdrop) { self.commit.dirty = true; }
    }

    /// Sets whether `backdrop` is reported supported by [`Cx::window_backdrop_supported`] for
    /// headless tests or hosts; a change re-runs the view and its memos. `WindowBackdrop::None` is
    /// always supported and ignores changes.
    #[inline]
    pub fn set_window_backdrop_supported(&mut self, backdrop: crate::WindowBackdrop, supported: bool) {
        if self.host.set_window_backdrop_supported(backdrop, supported) { self.commit.dirty = true; }
    }

    /// Replaces the entire supported-backdrop bitmask atomically; a change re-runs the view and
    /// its memos.
    #[cfg_attr(
        not(any(
            target_os = "windows",
            all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")),
            test
        )),
        allow(dead_code)
    )]
    #[inline]
    pub(crate) fn set_window_backdrop_supported_mask(&mut self, mask: u8) {
        if self.host.set_window_backdrop_supported_mask(mask) { self.commit.dirty = true; }
    }

    /// What [`Cx::renderer_kind`] reports; a change re-runs the view and its memos.
    #[inline]
    pub fn set_renderer_kind(&mut self, kind: crate::RendererKind) {
        if self.host.set_renderer_kind(kind) { self.commit.dirty = true; }
    }

    #[cfg_attr(not(any(all(target_os = "windows", feature = "windows-system-material"), test)), allow(dead_code))]
    #[inline]
    pub(crate) fn renderer_kind(&self) -> crate::RendererKind {
        self.host.renderer_kind
    }

    #[cfg_attr(
        not(any(
            target_os = "windows",
            all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")),
            test
        )),
        allow(dead_code)
    )]
    #[inline]
    pub(crate) fn set_backdrop_opaque_fallback(&mut self, fallback: bool) {
        if self.host.set_backdrop_opaque_fallback(fallback) { self.commit.dirty = true; }
    }

    #[inline]
    pub(crate) fn system_accent(&self) -> crate::SystemAccent {
        self.host.system_accent()
    }

    #[inline]
    pub fn set_system_accent(&mut self, accent: crate::SystemAccent) {
        if self.host.set_system_accent(accent) { self.commit.dirty = true; }
    }

    /// Tab visits every control when on, else only text fields and lists, like macOS with
    /// Keyboard navigation (Full Keyboard Access) off. Native windows follow the system setting.
    pub fn set_full_keyboard_access(&mut self, on: bool) {
        self.kbd.full_keyboard_access = on;
    }
    /// Tab visits every control whatever [`Runtime::set_full_keyboard_access`] says
    /// (`App::tab_visits_controls`).
    pub fn set_tab_visits_controls(&mut self, on: bool) {
        self.kbd.tab_visits_controls = on;
    }
    /// What [`Cx::system_contrast`] currently reports.
    #[cfg_attr(not(any(test, target_os = "linux", all(target_os = "windows", feature = "windows-system-material"))), allow(dead_code))]
    #[inline]
    pub(crate) fn system_contrast(&self) -> Option<crate::SystemContrast> {
        self.host.system_contrast
    }

    /// What [`Cx::system_increased_contrast`] currently reports.
    #[cfg(test)]
    #[inline]
    pub(crate) fn system_increased_contrast(&self) -> bool {
        self.host.system_increased_contrast
    }

    /// What [`Cx::system_contrast`] reports. Native Windows windows set it from the system's
    /// contrast theme as they open and when it changes; headless runs keep None.
    /// Returns whether the stored contrast changed.
    #[inline]
    pub fn set_system_contrast(&mut self, contrast: Option<crate::SystemContrast>) -> bool {
        let changed = self.host.set_system_contrast(contrast);
        if changed { self.commit.dirty = true; }
        changed
    }
    /// What [`Cx::system_increased_contrast`] reports. Native windows and web hosts set it from
    /// the system as they open, gain focus, and when its contrast setting changes; headless runs
    /// keep false. Returns whether the stored value changed.
    #[inline]
    pub fn set_system_increased_contrast(&mut self, on: bool) -> bool {
        let changed = self.host.set_system_increased_contrast(on);
        if changed { self.commit.dirty = true; }
        changed
    }
    /// What [`Cx::system_reduced_transparency`] reports. Native windows and the web set it from
    /// the system as they open, gain focus, and when display options change; headless runs keep
    /// false. Returns whether the stored preference changed.
    #[inline]
    pub fn set_system_reduced_transparency(&mut self, reduced: bool) -> bool {
        let changed = self.host.set_system_reduced_transparency(reduced);
        if changed { self.commit.dirty = true; }
        changed
    }
    /// Returns whether the stored reduced-motion preference changed.
    #[inline]
    pub fn set_system_reduced_motion(&mut self, reduced: bool) -> bool {
        let changed = self.host.set_system_reduced_motion(reduced);
        if changed { self.commit.dirty = true; }
        changed
    }

    /// What [`Cx::system_text_scale`] currently reports (`1.0` by default in a headless runtime).
    #[inline]
    pub(crate) fn system_text_scale(&self) -> f32 {
        self.host.system_text_scale()
    }

    /// What [`Cx::system_text_scale`] reports. Native and web hosts set it from the OS or
    /// browser's text-size preference and update it when that setting changes; a change re-runs
    /// the view and its memos. Non-finite or non-positive values fall back to `1.0`; positive
    /// values clamp to `0.25..=5.0`. Returns whether the stored scale changed.
    #[inline]
    pub fn set_system_text_scale(&mut self, scale: f32) -> bool {
        let changed = self.host.set_system_text_scale(scale);
        if changed { self.commit.dirty = true; }
        changed
    }

    /// Sets the preferred language list reported by [`Cx::system_languages`] and [`Cx::locale`],
    /// as BCP 47 tags in preference order. Empty and duplicate tags are dropped. A change re-runs
    /// the view and its memos. Returns whether the normalized language list changed.
    pub fn set_languages(&mut self, languages: Vec<String>) -> bool {
        let changed = self.host.set_languages(languages);
        if changed { self.commit.dirty = true; }
        changed
    }

    /// Current safe-area insets `[top, right, bottom, left]` in logical points: the host override
    /// set via [`Self::set_safe_area`], else browser insets on `wasm32`, else `[0.0; 4]`.
    #[inline]
    pub(crate) fn safe_area(&self) -> [f32; 4] {
        self.host.safe_area()
    }

    /// Sets the safe-area insets `[top, right, bottom, left]` in logical points reported by
    /// [`Cx::safe_area`] and used for [`El::on_edge_drag`] bands. Negative or non-finite values
    /// clamp to `0.0`. A change re-runs the view and its memos.
    pub fn set_safe_area(&mut self, insets: [f32; 4]) {
        if self.host.set_safe_area(insets) { self.commit.dirty = true; }
    }

    #[inline]
    pub(crate) fn system_scroll_bars_state(&self) -> crate::platform::scroll_bars::SystemScrollBars {
        self.host.system_scroll_bars_state()
    }

    #[inline]
    pub(crate) fn set_system_scroll_bars_state(&mut self, next: crate::platform::scroll_bars::SystemScrollBars) {
        if self.host.set_system_scroll_bars_state(next) { self.commit.dirty = true; }
    }

    /// What [`Cx::scroll_bars`] reports. Native macOS windows set it from `AppleShowScrollBars` and
    /// `NSPreferredScrollerStyleDidChangeNotification`; a change re-runs the view and its memos.
    /// Headless runs default to [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling).
    #[inline]
    pub fn set_system_scroll_bars(&mut self, bars: crate::ScrollBars) {
        if self.host.set_system_scroll_bars(bars) { self.commit.dirty = true; }
    }

    /// Whether the connected pointing device prefers overlay scroll bars under
    /// [`ScrollBars::Automatic`](crate::ScrollBars::Automatic) (`true` for a trackpad or touch
    /// surface, `false` for a mouse). Headless runs default to `true`.
    #[inline]
    pub fn set_system_scroller_overlay(&mut self, overlay: bool) {
        if self.host.set_system_scroller_overlay(overlay) { self.commit.dirty = true; }
    }

    /// Whether a click in a scroll bar's track jumps to the spot clicked (`true`) or pages toward
    /// it (`false`), as [`crate::system_scroller_jumps`] reports. Native macOS windows set it from
    /// `AppleScrollerPagingBehavior`; headless runs default to paging. A change re-runs the view.
    #[inline]
    pub fn set_system_scroller_jumps(&mut self, jumps: bool) {
        if self.host.set_system_scroller_jumps(jumps) { self.commit.dirty = true; }
    }

    /// The last stored [`crate::WindowManagerState`].
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    #[inline]
    pub(crate) fn window_manager_state(&self) -> crate::WindowManagerState {
        self.host.wm_state
    }

    /// What [`Cx::decoration_mode`], [`Cx::tiled_edges`], and [`Cx::window_actions`] report.
    /// Updates the stored state and marks the runtime dirty when it changed and the last
    /// committed view read any of those accessors. Returns whether the runtime was dirtied.
    #[inline]
    pub(crate) fn sync_window_manager_state(&mut self, state: crate::WindowManagerState) -> bool {
        if !self.host.set_window_manager_state(state) { return false; }
        if !self.host.wm_state_read { return false; }
        self.commit.dirty = true;
        true
    }

    /// What [`Cx::decoration_mode`], [`Cx::tiled_edges`], and [`Cx::window_actions`] report.
    #[inline]
    pub fn set_window_manager_state(&mut self, state: crate::WindowManagerState) {
        let _ = self.sync_window_manager_state(state);
    }

    /// What [`Cx::touch_controls`] reports natively (`false` in headless runs until touch input or
    /// [`Self::set_touch_controls`] changes it).
    #[inline]
    pub(crate) fn touch_controls(&self) -> bool {
        self.host.primary_pointer.touch_controls()
    }

    /// Updates the window's native primary pointer mode (`Cx::touch_controls` and `Cx::hover_none`),
    /// dirtying the view only when it changed and the last committed view read either accessor.
    /// Returns whether the runtime was dirtied.
    #[inline]
    pub(crate) fn sync_primary_pointer(&mut self, pointer: cx::PrimaryPointer) -> bool {
        if !self.host.set_primary_pointer(pointer) { return false; }
        if !self.host.primary_pointer_read { return false; }
        self.commit.dirty = true;
        true
    }

    /// Reconciles an OS tablet/slate-mode posture probe (`true` when Windows reports slate mode
    /// or a ready touchscreen without a mouse). Updates the window's primary pointer on the initial
    /// sample or when the posture actually transitions, so focus changes on hybrid touchscreen
    /// laptops preserve live touch vs. mouse mode.
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    #[inline]
    pub(crate) fn sync_system_tablet_mode(&mut self, tablet: bool) -> bool {
        let prev = self.host.system_tablet_mode;
        self.host.system_tablet_mode = Some(tablet);
        if prev == Some(tablet) {
            return false;
        }
        let target = if tablet { cx::PrimaryPointer::Touch } else { cx::PrimaryPointer::Hover };
        self.sync_primary_pointer(target)
    }

    /// Samples Windows tablet/slate posture and records the current `WM_SETTINGCHANGE` generation.
    #[cfg(target_os = "windows")]
    #[inline]
    pub(crate) fn sync_windows_tablet_mode(&mut self) -> bool {
        self.host.tablet_setting_gen = crate::platform::win32::touch_input::setting_generation();
        self.sync_system_tablet_mode(crate::platform::win32::touch_input::system_tablet_mode())
    }

    /// Re-samples Windows tablet/slate posture only when `WM_SETTINGCHANGE` bumped the generation.
    #[cfg(target_os = "windows")]
    #[inline]
    pub(crate) fn sync_windows_tablet_mode_if_changed(&mut self) -> bool {
        let current_gen = crate::platform::win32::touch_input::setting_generation();
        if current_gen == self.host.tablet_setting_gen {
            return false;
        }
        self.host.tablet_setting_gen = current_gen;
        self.sync_system_tablet_mode(crate::platform::win32::touch_input::system_tablet_mode())
    }

    /// Sets whether [`Cx::touch_controls`] (and [`Cx::hover_none`]) report coarse touch controls
    /// natively. A change re-runs the view and its memos.
    #[inline]
    pub fn set_touch_controls(&mut self, touch: bool) -> bool {
        let pointer = if touch { cx::PrimaryPointer::Touch } else { cx::PrimaryPointer::Hover };
        let changed = self.host.set_primary_pointer(pointer);
        if changed { self.commit.dirty = true; }
        changed
    }
}
