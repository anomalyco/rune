//! Presses that move or zoom the window: `.window_drag()` regions, and the titlebar band
//! AppKit drags itself when the app leaves window dragging to it.
use super::*;

#[derive(Default)]
pub(super) struct Titlebar {
    /// What the press just delivered asks the shell to do.
    pub(super) action: Option<TitlebarAction>,
    /// The last press that could start a double press: when (clock seconds) and where.
    last_press: Option<(f64, f32, f32)>,
    /// Window points from the top that AppKit drags natively, over the content (0 elsewhere).
    native_band: f32,
}

impl Titlebar {
    /// A press on a `.window_drag()` region: drag, or the double-click action on its second press.
    pub(super) fn press_region(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        self.action = Some(if self.double(now, x, y, interval) { TitlebarAction::DoubleClick } else { TitlebarAction::Drag });
    }

    /// A press that hit nothing. In the native band AppKit moves the window itself but doesn't
    /// see the double click over the content view, so the second press asks for it here.
    pub(super) fn press_empty(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        if y < self.native_band && self.double(now, x, y, interval) {
            self.action = Some(TitlebarAction::DoubleClick);
        }
    }

    pub(super) fn clear(&mut self) {
        self.last_press = None;
    }

    fn double(&mut self, now: f64, x: f32, y: f32, interval: f64) -> bool {
        let double = self.last_press.is_some_and(|(t, px, py)| {
            now - t <= interval && (x - px).abs() <= CLICK_SLOP && (y - py).abs() <= CLICK_SLOP
        });
        self.last_press = if double { None } else { Some((now, x, y)) };
        double
    }
}

impl<S: 'static> Runtime<S> {
    /// The titlebar band, in window points from the top, that the system drags natively while
    /// the content view fills it (macOS full-size content without drag regions). Empty presses
    /// there take the titlebar double-click action. 0, the default, leaves them alone.
    pub fn set_native_titlebar_band(&mut self, points: f32) {
        self.titlebar.native_band = points;
    }

    /// A window drag or titlebar double-click requested by the press just delivered. The shell
    /// must act on it before returning from that press event.
    pub fn take_titlebar_action(&mut self) -> Option<TitlebarAction> {
        self.titlebar.action.take()
    }
}
