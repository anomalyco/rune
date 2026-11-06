//! A titlebar the app draws itself (`App::titlebar`), and where the window's own controls sit
//! over it (`Cx::chrome_insets`).
//!
//! The app declares how tall its titlebar is; Rune takes the native one away and keeps the
//! window's controls over the app's: AppKit's traffic lights at the leading edge on macOS, and
//! the minimize, maximize and close buttons at the trailing edge on Windows, which Rune draws
//! as elements over the view. The app keeps clear of [`ChromeInsets`] and marks the parts
//! that move the window with `.window_drag()`; hit targets inside them keep their presses.

use crate::Chrome;

mod caption;

pub(crate) use caption::buttons;

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
/// - **Elsewhere:** the native frame stays and [`ChromeInsets`] is zero.
///
/// A borderless window has no titlebar to replace, and ignores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Titlebar {
    height: f32,
}

impl Titlebar {
    /// A caption button's width in points: Windows' and Electron's 46.
    pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;

    pub const fn new(height: f32) -> Self {
        Self { height }
    }

    pub fn height(&self) -> f32 {
        self.height
    }

    /// The frame macOS draws for this titlebar over `chrome`: full-size content, drag regions
    /// only, and the traffic lights in the band's middle unless the app placed them.
    pub(crate) fn mac_chrome(&self, chrome: Chrome) -> Chrome {
        let y = ((self.height - Chrome::TRAFFIC_LIGHT) / 2.0).max(0.0).round();
        Chrome { full_size_content: true, drag_regions: true, traffic_lights: chrome.traffic_lights.or(Some((16.0, y))) }
    }
}

/// What the window's own controls take of the app's titlebar, in points
/// (`Cx::chrome_insets`). The view re-runs when it changes: entering or leaving full screen,
/// a scale change. A `memo` closure that reads it needs it in its version.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ChromeInsets {
    /// From the window's leading edge to the end of the controls there: the macOS traffic
    /// lights (`Cx::titlebar_inset`). 0 in full screen and elsewhere.
    pub leading: f32,
    /// From the window's trailing edge to the start of the controls there: the Windows caption
    /// buttons. 0 in full screen and elsewhere.
    pub trailing: f32,
    /// The titlebar's height as the window draws it: [`Titlebar::height`] where Rune replaced
    /// the native titlebar, 0 where the native frame stays (no `App::titlebar`, Linux, a
    /// borderless window).
    pub height: f32,
}

impl ChromeInsets {
    pub(crate) fn new(leading: f32, trailing: f32, height: f32) -> Self {
        Self { leading, trailing, height }
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
    pub(crate) const ALL: [CaptionButton; 3] = [CaptionButton::Minimize, CaptionButton::Maximize, CaptionButton::Close];
}

/// The caption buttons Rune draws over a custom titlebar on Windows, and the state the window
/// procedure tracks for them: the system answers presses on them as non-client hits, so the
/// buttons see neither hover nor press as pointer events.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caption {
    pub height: f32,
    pub hover: Option<CaptionButton>,
    pub pressed: Option<CaptionButton>,
    pub maximized: bool,
}

impl Caption {
    pub(crate) fn new(height: f32) -> Self {
        Self { height, hover: None, pressed: None, maximized: false }
    }

    /// The buttons' rects, left to right, in points from the window's top-left for a window
    /// `width` points wide at `scale`: each a whole number of pixels, flush with the right edge.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn rects(&self, width: f32, scale: f32) -> [(CaptionButton, [f32; 4]); 3] {
        let (w, h) = caption::size(self.height, scale);
        std::array::from_fn(|i| (CaptionButton::ALL[i], [width - w * (3 - i) as f32, 0.0, w, h]))
    }

    /// Points the three buttons take from the trailing edge at `scale`.
    pub(crate) fn width(&self, scale: f32) -> f32 {
        caption::size(self.height, scale).0 * 3.0
    }
}
