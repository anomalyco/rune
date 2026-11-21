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
    /// What [`Cx::system_contrast`] reports. Native Windows windows set it from the system's
    /// contrast theme as they open and when it changes; headless runs keep None.
    #[inline]
    pub fn set_system_contrast(&mut self, contrast: Option<crate::SystemContrast>) {
        if self.host.set_system_contrast(contrast) { self.commit.dirty = true; }
    }
    /// What [`Cx::system_increased_contrast`] reports. Native windows set it from the system as
    /// they open, gain focus, and when its display options change; headless runs keep false.
    #[inline]
    pub fn set_system_increased_contrast(&mut self, on: bool) {
        if self.host.set_system_increased_contrast(on) { self.commit.dirty = true; }
    }
    #[inline]
    pub fn set_system_reduced_motion(&mut self, reduced: bool) {
        if self.host.set_system_reduced_motion(reduced) { self.commit.dirty = true; }
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
}
