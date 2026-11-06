//! Presses that move or zoom the window: `.window_drag()` regions, and the titlebar band
//! AppKit drags itself when the app leaves window dragging to it. Also the window's chrome
//! over a custom titlebar (`App::titlebar`): its insets and the Windows caption buttons.
use super::*;

#[derive(Default)]
pub(super) struct Titlebar {
    /// What the press just delivered asks the shell to do.
    pub(super) action: Option<TitlebarAction>,
    /// The last press that could start a double press: when (clock seconds) and where.
    last_press: Option<(f64, f32, f32)>,
    /// Window points from the top that AppKit drags natively, over the content (0 elsewhere).
    native_band: f32,
    /// A secondary press began on a bare `.window_drag()` region of a custom titlebar.
    secondary_on_region: bool,
    /// What `Cx::chrome_insets` reports.
    pub(super) insets: crate::ChromeInsets,
    /// The Windows caption buttons the view gets drawn over it, with their state.
    pub(super) caption: Option<crate::titlebar::Caption>,
    /// `Tx::toggle_maximized`, for the shell.
    pub(super) toggle_maximized: bool,
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

    /// What [`Cx::chrome_insets`] reports. Native windows set it from their controls on every
    /// resize, full screen included; a change re-runs the view.
    pub fn set_chrome_insets(&mut self, insets: crate::ChromeInsets) {
        if insets != self.titlebar.insets {
            self.titlebar.insets = insets;
            self.dirty = true;
        }
    }

    pub fn chrome_insets(&self) -> crate::ChromeInsets {
        self.titlebar.insets
    }

    /// The caption buttons to draw over the view, and their hover and press state; `None`
    /// draws none. A change re-runs the view.
    pub(crate) fn set_caption(&mut self, caption: Option<crate::titlebar::Caption>) -> bool {
        if caption == self.titlebar.caption { return false; }
        self.titlebar.caption = caption;
        self.dirty = true;
        true
    }

    /// What a run without a window reports and draws for `App::titlebar` on this platform, as
    /// its window would at rest: `Cx::titlebar_inset`, `Cx::chrome_insets` and the caption
    /// buttons. Returns the frame the headless traffic-light checks see.
    pub(crate) fn headless_chrome(&mut self, chrome: crate::Chrome, titlebar: Option<crate::Titlebar>, borderless: bool) -> crate::Chrome {
        let titlebar = titlebar.filter(|_| !borderless);
        let chrome = match titlebar {
            Some(t) if cfg!(target_os = "macos") => t.mac_chrome(chrome),
            _ => chrome,
        };
        let inset = if borderless { 0.0 } else { chrome.titlebar_inset() };
        self.set_titlebar_inset(inset);
        let caption = titlebar.filter(|_| cfg!(target_os = "windows")).map(|t| crate::titlebar::Caption::new(t.height()));
        let height = titlebar.filter(|_| cfg!(any(target_os = "macos", target_os = "windows"))).map_or(0.0, |t| t.height());
        let leading = if cfg!(target_os = "macos") { inset } else { 0.0 };
        let trailing = caption.map_or(0.0, |c| c.width(self.env.scale));
        self.set_chrome_insets(crate::ChromeInsets::new(leading, trailing, height));
        self.set_caption(caption);
        chrome
    }

    /// Takes a `Tx::toggle_maximized`, for the shell to carry out.
    pub(crate) fn take_toggle_maximized(&mut self) -> bool {
        std::mem::take(&mut self.titlebar.toggle_maximized)
    }

    /// Notes whether a secondary press at (x, y) landed on a bare `.window_drag()` region of a
    /// custom titlebar, where its release opens the window menu.
    pub(super) fn secondary_region_down(&mut self, x: f32, y: f32, captured: bool) {
        self.titlebar.secondary_on_region = !captured && self.titlebar.caption.is_some() && self.bare_drag_region(x, y);
    }

    /// A secondary release that ends the press `secondary_region_down` noted on a drag region
    /// asks the shell for the window menu.
    pub(super) fn secondary_region_up(&mut self, x: f32, y: f32) {
        if std::mem::take(&mut self.titlebar.secondary_on_region) && self.bare_drag_region(x, y) {
            self.titlebar.action = Some(TitlebarAction::SystemMenu);
        }
    }

    fn bare_drag_region(&self, x: f32, y: f32) -> bool {
        self.primary_target(x, y).is_some_and(|t| {
            let n = &self.nodes[t];
            n.window_drag && n.handlers.on_press.is_none() && n.handlers.on_double_press.is_none() && n.handlers.on_drag.is_none()
        })
    }
}
