//! A titlebar the app draws itself (`App::titlebar`), and where the window's own controls sit
//! over it (`Cx::chrome_insets`).
//!
//! The app declares how tall its titlebar is; Rune takes the native one away and keeps the
//! window's controls over the app's: AppKit's traffic lights at the leading edge on macOS, and
//! the minimize, maximize and close buttons at the trailing edge on Windows, which Rune draws
//! as elements over the view. On Linux, where the Wayland compositor draws no frame of its own
//! (GNOME), Rune draws the buttons the desktop's button layout names, and the window keeps the
//! client-side frame's shadow, rounded corners and resize handles (`linux`). The app keeps clear of [`ChromeInsets`] and marks the parts
//! that move the window with `.window_drag()`; hit targets inside them keep their presses.

use crate::Chrome;

mod caption;
#[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
pub(crate) mod linux;

pub(crate) use caption::{BUTTONS_KEY, EDGES_KEY, FRAME_KEY, frame};

/// The app draws its own titlebar across the top of the window, `height` points tall
/// (`App::titlebar`, `WindowSpec::titlebar`). Plain data, the same on every platform:
///
/// - **macOS:** a full-size content view under a transparent titlebar, `.window_drag()`
///   regions only (`Chrome::drag_regions`), and the traffic lights centred in the band unless
///   `Chrome::traffic_lights` places them.
/// - **Windows:** no native title bar. The window keeps its resize edges, Aero Snap, the snap
///   layouts flyout over maximize (Windows 11), the window menu (Alt+Space, or a right-click on
///   a `.window_drag()` region) and its animations; Rune draws the three caption buttons over
///   the trailing edge at the band's height, as Electron's `titleBarOverlay` does.
/// - **Linux on Wayland, where the compositor draws no window frame (GNOME):** no title bar
///   either, in place of the grey bar winit would draw above the app's. Rune draws the caption
///   buttons the desktop's button layout names (GNOME's default, `appmenu:close`, is close
///   alone; a layout on the left puts them there) as round buttons in the band. `.window_drag()`
///   regions move it, maximize it on a double click and open the compositor's window menu on a
///   right click. The window keeps the client-side frame's shadow, border and four rounded
///   corners while it floats, and resizes from the frame's handles outside its edges; where that
///   frame can't be made it is square and resizes from a band just inside its edges.
///   `RUNE_LINUX_FRAME=0` keeps winit's frame, and `=1` draws Rune's under any Wayland compositor.
/// - **Elsewhere** (X11, and Wayland compositors that draw the frame themselves: KDE, Sway,
///   Hyprland): the native frame stays and [`ChromeInsets`] is zero.
///
/// `Cx::titlebar_height` changes the height while the app runs, as a UI zoom does, and
/// `Cx::set_titlebar` the whole titlebar, as a switch between layouts does. A borderless window
/// has no titlebar to replace, and ignores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Titlebar {
    height: f32,
    traffic_lights: Option<(f32, f32)>,
    controls: bool,
}

impl Titlebar {
    /// A caption button's width in points: Windows' and Electron's 46.
    pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;

    pub const fn new(height: f32) -> Self {
        Self { height, traffic_lights: None, controls: true }
    }

    /// macOS: the close button's top-left, in points from the window's top-left, for this
    /// titlebar (Electron's `trafficLightPosition`), in place of `Chrome::traffic_lights` and
    /// the band's middle. Ignored elsewhere and in full screen.
    pub const fn traffic_lights(mut self, x: f32, y: f32) -> Self {
        self.traffic_lights = Some((x, y));
        self
    }

    /// Whether the window shows its own caption controls over the titlebar: traffic lights on
    /// macOS and the minimize/maximize/close buttons on Windows. Defaults to `true`.
    ///
    /// When `false`, Windows still installs the custom frame (keeping invisible outer side and
    /// bottom resize borders, the top inside resize edge, Aero Snap, Alt+Space and right-click
    /// window menu, and `Tx::minimize()`/`Tx::toggle_maximized()`), macOS hides its traffic
    /// lights while keeping the full-size content view and `.window_drag()` regions, and
    /// [`ChromeInsets`] reports zero `leading` and `trailing` insets. Use this when the app
    /// draws both its own titlebar and its own window controls.
    pub const fn controls(mut self, shown: bool) -> Self {
        self.controls = shown;
        self
    }

    pub fn height(&self) -> f32 {
        self.height
    }

    /// Where [`Self::traffic_lights`] put the close button, if it did.
    pub fn traffic_lights_origin(&self) -> Option<(f32, f32)> {
        self.traffic_lights
    }

    /// Whether the window shows its own caption controls over the titlebar (`true` by default).
    pub const fn has_controls(&self) -> bool {
        self.controls
    }

    /// The frame macOS draws for this titlebar over `chrome`: full-size content, drag regions
    /// only, and the traffic lights where this titlebar or the app placed them, else in the
    /// band's middle (`None` when [`Self::controls`] is `false`).
    pub(crate) fn mac_chrome(&self, chrome: Chrome) -> Chrome {
        if !self.controls {
            return Chrome { full_size_content: true, drag_regions: true, traffic_lights: None };
        }
        let y = ((self.height - Chrome::TRAFFIC_LIGHT) / 2.0).max(0.0).round();
        let lights = self.traffic_lights.or(chrome.traffic_lights).or(Some((16.0, y)));
        Chrome { full_size_content: true, drag_regions: true, traffic_lights: lights }
    }
}

/// What the view set of the window's titlebar on its last run: `Cx::set_titlebar`, then
/// `Cx::titlebar_height` over it. Empty keeps `App::titlebar`'s.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TitlebarSet {
    pub titlebar: Option<Titlebar>,
    pub height: Option<f32>,
}

impl TitlebarSet {
    /// The titlebar the window draws in place of the app's `base`.
    pub(crate) fn over(self, base: Titlebar) -> Titlebar {
        let titlebar = self.titlebar.unwrap_or(base);
        self.height.map_or(titlebar, |height| Titlebar { height, ..titlebar })
    }

    /// Whether the window draws another titlebar for this set than it did for `before`, over
    /// the app's `base`. A view that sets the titlebar the window opened with changes nothing,
    /// and without `App::titlebar` nothing the view sets applies.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn changes(self, before: TitlebarSet, base: Option<Titlebar>) -> bool {
        base.is_some_and(|base| self.over(base) != before.over(base))
    }
}

#[cfg(test)]
mod set_tests {
    use super::*;

    #[test]
    fn a_view_that_sets_the_titlebar_the_window_opened_with_changes_nothing() {
        let base = Titlebar::new(36.0).traffic_lights(14.0, 15.0);
        let unset = TitlebarSet::default();
        let same = TitlebarSet { titlebar: Some(base), height: None };
        assert!(!same.changes(unset, Some(base)));
        assert!(!TitlebarSet { titlebar: None, height: Some(36.0) }.changes(unset, Some(base)));
        assert!(!unset.changes(unset, Some(base)));
    }

    #[test]
    fn another_height_or_other_traffic_lights_change_it() {
        let base = Titlebar::new(48.0).traffic_lights(20.0, 25.0);
        let unset = TitlebarSet::default();
        assert!(TitlebarSet { titlebar: Some(Titlebar::new(36.0).traffic_lights(14.0, 15.0)), height: None }.changes(unset, Some(base)));
        assert!(TitlebarSet { titlebar: Some(Titlebar::new(48.0).traffic_lights(14.0, 15.0)), height: None }.changes(unset, Some(base)));
        assert!(TitlebarSet { titlebar: None, height: Some(52.0) }.changes(unset, Some(base)));
        // Lights the titlebar leaves to `App::chrome` against lights it places itself.
        assert!(TitlebarSet { titlebar: Some(Titlebar::new(48.0)), height: None }.changes(unset, Some(base)));
    }

    #[test]
    fn a_window_without_a_titlebar_of_its_own_never_changes() {
        let set = TitlebarSet { titlebar: Some(Titlebar::new(36.0)), height: Some(40.0) };
        assert!(!set.changes(TitlebarSet::default(), None));
    }

    #[test]
    fn controls_builder_defaults_true_and_is_tracked_by_titlebar_set() {
        let base = Titlebar::new(36.0);
        assert!(base.has_controls());
        let hidden = base.controls(false);
        assert!(!hidden.has_controls());
        assert!(hidden.controls(true).has_controls());

        let unset = TitlebarSet::default();
        let hide_set = TitlebarSet { titlebar: Some(hidden), height: None };
        assert!(hide_set.changes(unset, Some(base)));
        assert!(!hide_set.changes(unset, Some(hidden)));
        assert!(!TitlebarSet { titlebar: None, height: Some(44.0) }.over(hidden).has_controls());
        assert_eq!(hidden.mac_chrome(Chrome::DEFAULT), Chrome { full_size_content: true, drag_regions: true, traffic_lights: None });
    }
}

/// What the window's own controls take of the app's titlebar, in points
/// (`Cx::chrome_insets`). The view re-runs when it changes: entering or leaving full screen,
/// a scale change, `Cx::titlebar_height`, `Cx::set_titlebar`. Memos rebuild with it too, so a
/// `memo` closure that reads it needs nothing in its version.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ChromeInsets {
    /// From the window's left edge to the end of the controls there: the macOS traffic lights
    /// (`Cx::titlebar_inset`), or the Windows caption buttons when the app's language reads right
    /// to left (`labels::set_direction`), where the traffic lights move to the right instead, or a
    /// Linux frame's buttons when the desktop's layout puts them on the left. 0 in full
    /// screen and elsewhere.
    pub leading: f32,
    /// From the window's right edge to the start of the controls there: the Windows caption
    /// buttons, unless they're mirrored to the left, a Linux frame's buttons, or the macOS
    /// traffic lights in a right-to-left app. 0 in full screen and elsewhere.
    pub trailing: f32,
    /// The titlebar's height as the window draws it: [`Titlebar::height`] where Rune replaced
    /// the native titlebar, 0 where the native frame stays (no `App::titlebar`, X11 and Wayland
    /// compositors that draw the frame themselves, a borderless window).
    pub height: f32,
}

impl ChromeInsets {
    pub(crate) fn new(leading: f32, trailing: f32, height: f32) -> Self {
        Self { leading, trailing, height }
    }

    /// The insets, in window points, in the points of a view zoomed `k` times (`Cx::zoom`).
    pub(crate) fn zoomed(self, k: f32) -> Self {
        Self { leading: self.leading / k, trailing: self.trailing / k, height: self.height / k }
    }

    /// The bits a memo scope hashes, so memos rebuild when the insets change.
    pub(crate) fn bits(&self) -> [u32; 3] {
        [self.leading, self.trailing, self.height].map(f32::to_bits)
    }
}

/// One of the Windows caption buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

impl CaptionButton {
    const ALL: [CaptionButton; 3] = [CaptionButton::Minimize, CaptionButton::Maximize, CaptionButton::Close];
}

/// The caption buttons a window draws ([`Caption::buttons`]), at most one of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Buttons {
    list: [CaptionButton; 3],
    len: usize,
}

impl Buttons {
    fn of(buttons: impl IntoIterator<Item = CaptionButton>) -> Self {
        let mut out = Buttons { list: CaptionButton::ALL, len: 0 };
        for button in buttons.into_iter().take(3) {
            out.list[out.len] = button;
            out.len += 1;
        }
        out
    }
}

impl std::ops::Deref for Buttons {
    type Target = [CaptionButton];

    fn deref(&self) -> &[CaptionButton] {
        &self.list[..self.len]
    }
}

/// Where the pointer is with the caption buttons. The system answers presses on them as
/// non-client hits, so the buttons see neither hover nor press as pointer events: the window
/// procedure keeps this, and the buttons draw from it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CaptionState {
    #[default]
    Idle,
    /// Over `button` with no press.
    Hovered(CaptionButton),
    /// A press began on `button`; `over` while the pointer is still on it. Releasing there acts.
    Pressed { button: CaptionButton, over: bool },
}

/// The caption buttons Rune draws over a custom titlebar: on Windows, and on Linux where the
/// window has a frame of Rune's own (`linux`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caption {
    pub height: f32,
    pub state: CaptionState,
    pub maximized: bool,
    /// The window has a minimize box (`WS_MINIMIZEBOX`); without one, the button is dimmed.
    pub minimizable: bool,
    /// The window has a maximize box (`WS_MAXIMIZEBOX`); a window without one, a window that
    /// isn't resizable, never maximizes, and the button is dimmed.
    pub maximizable: bool,
    /// Full screen: the window draws no buttons, but keeps the frame that hosts them.
    pub hidden: bool,
    /// The app's language reads right to left (`labels::set_direction`): the buttons sit at the
    /// window's top-left, Close first, as Windows mirrors a right-to-left window's caption and
    /// Chrome and Edge draw theirs in Arabic or Hebrew. A Linux frame's sit at the left
    /// when the desktop's layout puts them there.
    pub mirrored: bool,
    /// A Linux frame's buttons, as the desktop lays them out, in GNOME's look
    /// (`caption::round_buttons`); `None` is the Windows caption.
    pub layout: Option<linux::ButtonLayout>,
    /// The sides the frame resizes the window from, by a band inside them (`caption::edges`):
    /// a Linux frame's, on a resizable window that is neither maximized nor full screen, less
    /// the sides the compositor tiled; none where winit's frame around the window has the
    /// resize handles.
    pub edges: linux::Sides,
}

impl Caption {
    pub(crate) fn new(height: f32) -> Self {
        Self { height, state: CaptionState::Idle, maximized: false, minimizable: true, maximizable: true, hidden: false, mirrored: crate::labels::rtl(), layout: None, edges: linux::Sides::NONE }
    }

    /// The caption of Rune's own frame on Linux (`linux`) for a window in this state:
    /// `layout`'s buttons, at the left when the layout puts them there (the right, in an app
    /// that reads right to left), the ones the compositor allows, and resize edges along the
    /// sides that are free while the window floats, unless a frame around it resizes it
    /// (`linux::WindowState::outer_frame`).
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    pub(crate) fn own_frame(height: f32, layout: linux::ButtonLayout, window: linux::WindowState) -> Self {
        let floats = window.resizable && !window.maximized && !window.full_screen && !window.outer_frame;
        let free = |tiled: bool| floats && !tiled;
        Self {
            maximized: window.maximized,
            minimizable: window.minimizable,
            maximizable: window.maximizable,
            hidden: window.full_screen,
            mirrored: layout.leading != crate::labels::rtl(),
            layout: Some(layout),
            edges: linux::Sides {
                top: free(window.tiled.top),
                right: free(window.tiled.right),
                bottom: free(window.tiled.bottom),
                left: free(window.tiled.left),
            },
            ..Self::new(height)
        }
    }

    /// The buttons drawn, left to right (right to left when `mirrored`). A window that can
    /// neither minimize nor maximize shows only Close, as Windows draws its native caption. A
    /// Linux frame shows the ones the desktop's layout names, less those the window can't do,
    /// as GTK's window controls do.
    pub(crate) fn buttons(&self) -> Buttons {
        match (self.hidden, self.layout, self.minimizable || self.maximizable) {
            (true, ..) => Buttons::of([]),
            (false, Some(layout), _) => Buttons::of(layout.buttons.into_iter().flatten().filter(|&button| self.enabled(button))),
            (false, None, true) => Buttons::of(CaptionButton::ALL),
            (false, None, false) => Buttons::of([CaptionButton::Close]),
        }
    }

    /// Whether `button` does anything: a press on a dimmed button doesn't.
    pub(crate) fn enabled(&self, button: CaptionButton) -> bool {
        match button {
            CaptionButton::Minimize => self.minimizable,
            CaptionButton::Maximize => self.maximizable,
            CaptionButton::Close => true,
        }
    }

    /// The buttons' rects, in [`Caption::buttons`]' order, in points from the window's top-left for
    /// a window `width` points wide at `scale`: each a whole number of pixels, flush with the right
    /// edge, or the left one mirrored (Close leftmost).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn rects(&self, width: f32, scale: f32) -> impl Iterator<Item = (CaptionButton, [f32; 4])> + use<> {
        let (w, h) = caption::size(self.height, scale);
        let buttons = self.buttons();
        let (n, mirrored) = (buttons.len(), self.mirrored);
        (0..n).map(move |i| (buttons[i], [if mirrored { w * (n - 1 - i) as f32 } else { width - w * (n - i) as f32 }, 0.0, w, h]))
    }

    /// The enabled button at (x, y), in points from the window's top-left, in a window `width`
    /// points wide at `scale`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn hit(&self, width: f32, scale: f32, (x, y): (f32, f32)) -> Option<CaptionButton> {
        self.rects(width, scale)
            .find(|(_, [rx, ry, rw, rh])| x >= *rx && x < rx + rw && y >= *ry && y < ry + rh)
            .map(|(button, _)| button)
            .filter(|&b| self.enabled(b))
    }

    /// Points the buttons take from their edge at `scale`: the right, or the left mirrored.
    pub(crate) fn width(&self, scale: f32) -> f32 {
        let n = self.buttons().len();
        if self.layout.is_some() { return caption::round_width(n, scale); }
        caption::size(self.height, scale).0 * n as f32
    }
}
