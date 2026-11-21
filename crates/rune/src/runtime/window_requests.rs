//! What a commit asks of the window besides its pixels: the content height a window that
//! fits its content asks for (`App::fit_content_height`), and the title (`Tx::set_title`).
use super::*;

/// Window/host boundary state owned by [`RuntimeCore`]: system settings, zoom, titlebar, window
/// frame/title/fit/minimize requests, popups, file dialogs, open-URL requests, accessibility and
/// web page bands.
pub(crate) struct HostState {
    /// The view's UI zoom and the window it applies to (`Cx::zoom`).
    pub(in crate::runtime) zoom: zoom::Zoom,
    pub(in crate::runtime) titlebar: titlebar::Titlebar,
    pub(in crate::runtime) titlebar_inset: f32,
    pub(in crate::runtime) system_reduced_motion: bool,
    pub(in crate::runtime) system_accent: crate::SystemAccent,
    /// What [`Cx::system_contrast`] reports.
    pub(in crate::runtime) system_contrast: Option<crate::SystemContrast>,
    /// What [`Cx::system_increased_contrast`] reports.
    pub(in crate::runtime) system_increased_contrast: bool,
    pub(in crate::runtime) scroll_bars: system_settings::ScrollBarHost,
    pub(crate) native_overlay: bool,
    /// The window or overlay surface's frame counts, published by its host.
    pub(crate) surface_stats: Option<std::sync::Arc<std::sync::Mutex<crate::SurfaceStats>>>,
    /// `App::dev_stats`.
    pub(crate) app_stats: Option<fn() -> String>,
    /// The window's content top-left in global screen points, as its host last saw it.
    window_origin: (f32, f32),
    /// The window's content top-left in desktop pixels, as its host last saw it (`Drag::screen_point`).
    pub(in crate::runtime) window_pixels: Option<(i32, i32)>,
    pub(in crate::runtime) window_focused: Option<bool>,
    /// The host's window frame isn't moving or about to: set by the host before each frame.
    pub(crate) window_resting: bool,
    /// A frame change for the window (`Tx::resize_window`), with its spring resolved.
    pub(crate) window_frame: Option<crate::window::frame::FrameRequest>,
    /// The window's title (`Tx::set_title`), from `AppSpec`'s.
    pub(crate) window_title: Title,
    /// The window's height follows the root's natural height (`App::fit_content_height`).
    pub(crate) fit_height: bool,
    /// The height a window that fits its content last asked for, and the frame its move
    /// starts with. The host takes the hold as it starts the move.
    pub(crate) fit: FitWindow,
    /// A handler or `update` asked to minimize the window (`Tx::minimize`); the shell takes it,
    /// after a show the same turn asked for (`window::Minimize`).
    pub(crate) minimize: crate::window::Minimize,
    /// Links handlers asked to open that this runtime left unopened (`Runtime::take_url_requests`).
    url_requests: Vec<String>,
    /// True when attached to a live window, overlay or web canvas, not a headless or scripted run.
    pub(crate) open_urls_natively: bool,
    /// Pop-up menus asked for (`Tx::popup_menu`) and not yet answered, oldest first.
    pub(in crate::runtime) popups: popup_dispatch::Popups,
    /// The file dialog asked for (`Tx::open_file`, `Tx::save_file`) and not yet answered.
    pub(in crate::runtime) file_dialog: crate::file_dialog::DialogState,
    /// File dialog callbacks, by id, until their answer runs.
    pub(in crate::runtime) file_dialog_answers: Vec<(u64, crate::file_dialog::AnswerFn)>,
    pub(in crate::runtime) next_file_dialog: u64,
    /// `Tx::announce`'s last words, for the accessibility tree's live region.
    pub(crate) announcement: crate::a11y::Announcement,
    /// The accessibility tree, while an assistive client listens (`a11y/tree.rs`).
    pub(in crate::runtime) a11y: Option<Box<crate::a11y::tree::A11yTree>>,
    /// The band of a taller document the window draws (`App::web_document`).
    pub(in crate::runtime) page: Option<web_page::Page>,
}

impl HostState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new(viewport: (f32, f32), env: &Env) -> Self {
        Self {
            zoom: zoom::Zoom { k: 1.0, window: viewport, window_scale: env.scale },
            titlebar: Default::default(),
            titlebar_inset: 0.0,
            system_reduced_motion: false,
            system_accent: crate::SystemAccent::DEFAULT,
            system_contrast: None,
            system_increased_contrast: false,
            scroll_bars: Default::default(),
            native_overlay: false,
            surface_stats: None,
            app_stats: None,
            window_origin: (0.0, 0.0),
            window_pixels: None,
            window_focused: None,
            window_resting: false,
            window_frame: None,
            window_title: Default::default(),
            fit_height: false,
            fit: Default::default(),
            minimize: Default::default(),
            url_requests: Vec::new(),
            open_urls_natively: false,
            popups: Default::default(),
            file_dialog: Default::default(),
            file_dialog_answers: Vec::new(),
            next_file_dialog: 0,
            announcement: Default::default(),
            a11y: None,
            page: None,
        }
    }

    #[inline]
    pub(super) fn window_origin(&self) -> (f32, f32) {
        self.window_origin
    }

    /// Updates the cached global screen origin; returns true when it changed.
    #[inline]
    pub(super) fn set_window_origin(&mut self, origin: (f32, f32)) -> bool {
        if self.window_origin == origin { return false; }
        self.window_origin = origin;
        true
    }

    /// Takes the links handlers asked to open that this runtime didn't open: all of them in a
    /// headless run or test, none in a window. Each was already checked.
    #[inline]
    pub(crate) fn take_url_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.url_requests)
    }

    #[inline]
    pub(crate) fn push_url_request(&mut self, url: String) {
        self.url_requests.push(url);
    }

    #[inline]
    pub(crate) fn a11y(&self) -> Option<&crate::a11y::tree::A11yTree> {
        self.a11y.as_deref()
    }

    #[inline]
    pub(crate) fn a11y_mut(&mut self) -> Option<&mut crate::a11y::tree::A11yTree> {
        self.a11y.as_deref_mut()
    }

    #[inline]
    pub(crate) fn a11y_or_insert(&mut self) -> &mut crate::a11y::tree::A11yTree {
        self.a11y.get_or_insert_with(Default::default)
    }

    #[inline]
    pub(crate) fn a11y_deactivate(&mut self) {
        self.a11y = None;
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn popup_is_open(&self) -> bool {
        self.popups.is_open()
    }

    #[inline]
    pub(super) fn clear_titlebar(&mut self) {
        self.titlebar.clear();
    }

    #[inline]
    pub(crate) fn set_app_stats(&mut self, stats: Option<fn() -> String>) {
        self.app_stats = stats;
    }

    /// Asks the window for the content's new height, on this commit's spring. The first
    /// commit, a viewport change and `Tx::instant` jump.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fit_window(
        &mut self,
        height: f32,
        viewport: (f32, f32),
        scale: f32,
        reduced_motion: bool,
        headless: bool,
        tx: &Tx,
        first: bool,
    ) {
        let height = height.ceil_to(scale).max(1.0);
        if !self.fit.ask(height, viewport.1) { return; }
        let jump = first || tx.commit.snaps() || tx.relayout || reduced_motion;
        // Only a macOS window and headless runs move the frame on a spring.
        if !jump && self.window_resting && (cfg!(target_os = "macos") || headless) { self.fit.hold = FitHold::Asked; }
        let request = crate::window::frame::FrameRequest {
            size: Some((viewport.0, height)), animate: !jump,
            spring: (!jump).then(|| tx.motion.map_or(crate::tok::motion::DEFAULT, MotionSource::spring)), center: false,
        };
        self.window_frame = Some(self.window_frame.unwrap_or_default().merge(request));
    }

    #[inline]
    pub(super) fn content_height_from(&self, view_height: f32) -> f32 {
        (view_height * self.zoom.k).ceil_to(self.zoom.window_scale).max(1.0)
    }

    /// Someone resized a window that fits its content (not a page's band) to `height`: it
    /// goes back to the content's height.
    #[inline]
    pub(super) fn fit_resized(&mut self, height: f32, current_height: f32) -> bool {
        self.fit_height && self.page.is_none() && self.window_resting && self.fit.resized(height, current_height)
    }

    /// Whether this frame holds its motion for the window (see [`FitHold`]).
    #[inline]
    pub(super) fn fit_frame_holds(&mut self) -> bool {
        self.fit.frame_holds()
    }
}

/// The window's title: `AppSpec`'s until a handler sets another, and whether the window has
/// yet to show the last one set.
#[derive(Debug, Default)]
pub(crate) enum Title {
    /// No title yet: the window hasn't taken `AppSpec`'s.
    #[default]
    Unset,
    /// The title the window shows.
    Shown(String),
    /// A title set since the window last showed one.
    Pending(String),
}

/// What moves the title on.
#[derive(Debug)]
pub(super) enum TitleEvent {
    /// The window opens with `AppSpec`'s title.
    Start(String),
    /// A handler set `title`.
    Set(String),
    /// The window showed the title it was given.
    Shown,
}

impl Title {
    /// The title the window opens with, before any handler sets one.
    pub(crate) fn start(&mut self, title: &str) {
        *self = std::mem::take(self).step(TitleEvent::Start(title.to_owned())).0;
    }

    pub(super) fn set(&mut self, title: String) {
        *self = std::mem::take(self).step(TitleEvent::Set(title)).0;
    }

    pub(crate) fn current(&self) -> Option<&str> {
        match self {
            Title::Unset => None,
            Title::Shown(title) | Title::Pending(title) => Some(title),
        }
    }

    /// A title set since the window last took one, for the window to show.
    pub(crate) fn take_change(&mut self) -> Option<&str> {
        let (title, showed) = std::mem::take(self).step(TitleEvent::Shown);
        *self = title;
        showed.then(|| self.current()).flatten()
    }

    /// The title after `event`, and whether the window shows something new.
    fn step(self, event: TitleEvent) -> (Title, bool) {
        match (self, event) {
            (Title::Unset, TitleEvent::Start(title)) => (Title::Shown(title), false),
            (title, TitleEvent::Start(_)) => (title, false),
            (title, TitleEvent::Set(next)) if title.current() == Some(next.as_str()) => (title, false),
            (_, TitleEvent::Set(title)) => (Title::Pending(title), false),
            (Title::Pending(title), TitleEvent::Shown) => (Title::Shown(title), true),
            (title, TitleEvent::Shown) => (title, false),
        }
    }
}

/// A window that fits its content (`App::fit_content_height`): the height it last asked the
/// window for, and the frame its move starts with.
#[derive(Debug, Default)]
pub(crate) struct FitWindow {
    /// None until it asks, and again once someone else resizes the window.
    asked: Option<f32>,
    hold: FitHold,
}

/// A fitting window at rest asked to move. The window can't move until the next frame, so the
/// frame that asked shows the commit's motion at its start, and the host starts the window's
/// own move from that frame: both then start moving on the same frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum FitHold {
    #[default]
    None,
    /// A commit asked; the frame's motion step holds.
    Asked,
    /// The last frame held, for the host to start the window's move from.
    Held,
}

impl FitWindow {
    /// Whether the window should be asked for `height`: it isn't already asked for it, nor
    /// (asked for nothing since it was last resized) already `current` tall.
    fn ask(&mut self, height: f32, current: f32) -> bool {
        if self.asked.unwrap_or(current) == height { return false; }
        self.asked = Some(height);
        true
    }

    /// The window was resized to `height` from outside: forgets what it asked, so the next
    /// commit asks for the content's height again. True when that differs from what it asked
    /// (or, never asked, from `current`).
    fn resized(&mut self, height: f32, current: f32) -> bool {
        if (self.asked.unwrap_or(current) - height).abs() <= 0.5 { return false; }
        self.asked = None;
        true
    }

    /// The frame steps its motion: whether a commit asked it to hold.
    fn frame_holds(&mut self) -> bool {
        self.hold = if self.hold == FitHold::Asked { FitHold::Held } else { FitHold::None };
        self.hold == FitHold::Held
    }

    /// Whether the last frame held its motion for the window, for the host starting the
    /// window's move. The hold ends with it.
    pub(crate) fn take_held(&mut self) -> bool {
        std::mem::take(&mut self.hold) == FitHold::Held
    }
}

impl RuntimeCore {
    /// Asks the window for the content's new height, on this commit's spring. The first
    /// commit, a viewport change and `Tx::instant` jump.
    #[inline]
    pub(super) fn fit_window(&mut self, height: f32, tx: &Tx, first: bool) {
        self.host.fit_window(height, self.viewport, self.env.scale, self.reduced_motion, self.wall_start.is_none(), tx, first);
    }

    /// The root's natural height at the current width, in window points (the view's times its
    /// zoom), measured without committing, so a window that fits its content can open at that
    /// height.
    pub(crate) fn content_height(&mut self, app: &dyn ViewDriver) -> f32 {
        let measured = self.measure_pass(app, &Tx::default(), false);
        let height = measured.nodes.first().map_or(self.viewport.1, |root| root.local.h);
        self.discard_measured(measured);
        self.host.content_height_from(height)
    }

    /// Someone resized a window that fits its content (not a page's band) to `height`: it
    /// goes back to the content's height.
    #[inline]
    pub(super) fn fit_resized(&mut self, height: f32) -> bool {
        self.host.fit_resized(height, self.viewport.1)
    }

    /// Whether this frame holds its motion for the window (see [`FitHold`]).
    #[inline]
    pub(super) fn fit_frame_holds(&mut self) -> bool {
        self.host.fit_frame_holds()
    }
}

#[cfg(test)]
#[path = "window_requests_tests.rs"]
mod tests;
