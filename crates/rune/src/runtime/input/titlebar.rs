//! Presses that move or zoom the window: `.window_drag()` regions, and the titlebar band
//! AppKit drags itself when the app leaves window dragging to it. Also the window's chrome
//! over a custom titlebar (`App::titlebar`): its insets, the Windows caption buttons and the
//! window menu.
use crate::machine::Effects;

use super::*;

#[derive(Default)]
pub(super) struct Titlebar {
    /// What the press just delivered asks the shell to do, for `take_titlebar_action`.
    pub(super) action: Option<TitlebarAction>,
    /// The press in flight (`step`).
    pub(super) press: TitlebarPress,
    /// Window points from the top that AppKit drags natively, over the content (0 elsewhere).
    native_band: f32,
    /// The window menu the shell opens next.
    window_menu: Option<WindowMenu>,
    /// What `Cx::chrome_insets` reports.
    pub(super) insets: crate::ChromeInsets,
    /// The Windows caption buttons the view gets drawn over it, with their state.
    pub(super) caption: Option<crate::titlebar::Caption>,
    /// `Tx::toggle_maximized`, for the shell.
    pub(super) toggle_maximized: bool,
    /// `Tx::toggle_full_screen`, for the shell.
    pub(super) toggle_full_screen: bool,
    /// `Cx::set_titlebar` and `Cx::titlebar_height`, as the last view set them, and whether the
    /// shell has yet to see them.
    set: crate::titlebar::TitlebarSet,
    set_changed: bool,
    /// What a run without a window draws for `App::titlebar` (`headless_chrome`), so a new
    /// `Cx::set_titlebar` or `Cx::titlebar_height` draws again.
    headless: Option<(crate::Chrome, crate::Titlebar)>,
}

/// The window menu (Restore, Move, Size, Minimize, Maximize, Close) a custom titlebar opens
/// where the native title bar would.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowMenu {
    /// At the pointer: a secondary click on a bare `.window_drag()` region.
    Pointer,
    /// Under the window's top-left corner, from the keyboard: Alt+Space no handler took.
    Keyboard,
}

/// A press that may pair with the next one: when it went down and where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pair {
    /// Clock seconds it went down at.
    pub(super) at: f64,
    /// Window points it went down at.
    pub(super) point: (f32, f32),
}

impl Pair {
    /// Whether a press at `point` at `now` is this press's second: within the double-click
    /// interval and the click slop.
    fn second(self, now: f64, point: (f32, f32), interval: f64) -> bool {
        press_run::follows(self.at, self.point, now, point, interval)
    }
}

/// The titlebar's press: what the next press can pair with, and whether a secondary press on a
/// bare `.window_drag()` region still owes its release.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum TitlebarPress {
    /// No press is in flight.
    #[default]
    Rest,
    /// A press went down where a second one means something.
    Armed(Pair),
    /// A secondary press began on a bare drag region: its release there opens the window menu.
    /// `Some` is the primary run it didn't end.
    SecondaryOnRegion(Option<Pair>),
}

impl TitlebarPress {
    /// Whether a secondary press `SecondaryDown` began on a bare region owes its release.
    pub(super) fn secondary_armed(self) -> bool { matches!(self, TitlebarPress::SecondaryOnRegion(_)) }
}

/// What the titlebar's press sees.
pub(super) enum Event {
    /// A primary press on a bare `.window_drag()` region.
    Region { now: f64, point: (f32, f32), interval: f64 },
    /// A primary press that hit nothing; `band` when it is in the native titlebar band, where
    /// the system drags the window itself and only the double click is Rune's.
    Empty { now: f64, point: (f32, f32), interval: f64, band: bool },
    /// A secondary press; `region` when it landed on a bare drag region of a custom titlebar.
    SecondaryDown { region: bool },
    /// A secondary release; `region` when it is over a bare drag region now.
    SecondaryUp { region: bool },
    /// Pointer cancellation or stage replacement ends both halves of any pending press.
    Clear,
}

/// What a press asks the window for, in that press event: the shell starts a native drag inside
/// the press, and the window menu opens where the pointer is.
pub(super) enum Request {
    /// Move the window, or the titlebar's double-click action.
    Action(TitlebarAction),
    /// Open the window menu at the pointer.
    Menu,
}

/// The run a `Region` or `Empty` press carries on with or starts, and whether it was a double.
fn pair_of(last: Option<Pair>, now: f64, point: (f32, f32), interval: f64) -> (Option<Pair>, bool) {
    match last {
        Some(last) if last.second(now, point, interval) => (None, true),
        _ => (Some(Pair { at: now, point }), false),
    }
}

/// The press run `state` holds, whatever it is doing with a secondary press.
fn pair(state: &TitlebarPress) -> Option<Pair> {
    match *state {
        TitlebarPress::Rest => None,
        TitlebarPress::Armed(pair) => Some(pair),
        TitlebarPress::SecondaryOnRegion(pair) => pair,
    }
}

fn armed(pair: Option<Pair>) -> TitlebarPress {
    pair.map_or(TitlebarPress::Rest, TitlebarPress::Armed)
}

/// The state with its press run replaced, keeping a secondary press that still owes its release.
fn keep_secondary(state: &TitlebarPress, pair: Option<Pair>) -> TitlebarPress {
    if state.secondary_armed() { TitlebarPress::SecondaryOnRegion(pair) } else { armed(pair) }
}

pub(super) fn step(state: &mut TitlebarPress, event: Event) -> Effects<Request> {
    match event {
        // A press on a drag region always moves the window, unless it is the second of a pair.
        Event::Region { now, point, interval } => {
            let (next, double) = pair_of(pair(state), now, point, interval);
            *state = keep_secondary(state, next);
            Effects::one(Request::Action(if double { TitlebarAction::DoubleClick } else { TitlebarAction::Drag }))
        }
        // AppKit drags the band itself and never sees the double click over the content.
        Event::Empty { now, point, interval, band } => {
            if !band { return Effects::none(); }
            let (next, double) = pair_of(pair(state), now, point, interval);
            *state = keep_secondary(state, next);
            if double { Effects::one(Request::Action(TitlebarAction::DoubleClick)) } else { Effects::none() }
        }
        Event::SecondaryDown { region } => {
            *state = if region { TitlebarPress::SecondaryOnRegion(pair(state)) } else { armed(pair(state)) };
            Effects::none()
        }
        Event::SecondaryUp { region } => {
            let TitlebarPress::SecondaryOnRegion(pair) = *state else { return Effects::none() };
            *state = armed(pair);
            if region { Effects::one(Request::Menu) } else { Effects::none() }
        }
        Event::Clear => {
            // differs from dev: a cancelled secondary down cannot open a menu on release.
            *state = TitlebarPress::Rest;
            Effects::none()
        }
    }
}

impl Titlebar {
    /// A press on a `.window_drag()` region: drag, or the double-click action on its second press.
    pub(super) fn press_region(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        self.act(Event::Region { now, point: (x, y), interval });
    }

    /// A press that hit nothing. In the native band AppKit moves the window itself but doesn't
    /// see the double click over the content view, so the second press asks for it here.
    pub(super) fn press_empty(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        let band = y < self.native_band;
        self.act(Event::Empty { now, point: (x, y), interval, band });
    }

    pub(super) fn clear(&mut self) {
        self.act(Event::Clear);
    }

    /// Steps the press and hands its request to the host, where the shell takes it in that press.
    fn act(&mut self, event: Event) {
        for request in step(&mut self.press, event) {
            match request {
                Request::Action(action) => self.action = Some(action),
                Request::Menu => self.window_menu = Some(WindowMenu::Pointer),
            }
        }
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
    pub(crate) fn set_caption(&mut self, caption: Option<crate::titlebar::Caption>) {
        if caption == self.titlebar.caption { return; }
        self.titlebar.caption = caption;
        self.dirty = true;
    }

    /// What a run without a window reports and draws for `App::titlebar` on this platform, as
    /// its window would at rest: `Cx::titlebar_inset`, `Cx::chrome_insets` and the caption
    /// buttons. Returns the frame the headless traffic-light checks see.
    pub(crate) fn headless_chrome(&mut self, chrome: crate::Chrome, titlebar: Option<crate::Titlebar>, borderless: bool) -> crate::Chrome {
        let titlebar = titlebar.filter(|_| !borderless);
        self.titlebar.headless = titlebar.map(|t| (chrome, t));
        let titlebar = titlebar.map(|t| self.titlebar.set.over(t));
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
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_toggle_maximized(&mut self) -> bool {
        std::mem::take(&mut self.titlebar.toggle_maximized)
    }

    /// Takes a `Tx::toggle_full_screen`, for the shell to carry out.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_toggle_full_screen(&mut self) -> bool {
        std::mem::take(&mut self.titlebar.toggle_full_screen)
    }

    /// The window menu a secondary click on a drag region or an unclaimed Alt+Space asked for,
    /// for the shell to open.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_window_menu(&mut self) -> Option<WindowMenu> {
        self.titlebar.window_menu.take()
    }

    /// Alt+Space no handler took opens the window menu (`Unhandled::WindowMenu`).
    pub(super) fn request_window_menu(&mut self) {
        self.titlebar.window_menu = Some(WindowMenu::Keyboard);
    }

    /// `Cx::set_titlebar` and `Cx::titlebar_height`, as the last view set them; empty keeps
    /// `App::titlebar`'s.
    pub(crate) fn titlebar_set(&self) -> crate::titlebar::TitlebarSet {
        self.titlebar.set
    }

    /// Notes the titlebar a view set. A run without a window draws it at once; a window's shell
    /// resizes its frame and moves the traffic lights (`take_titlebar_changed`).
    pub(super) fn set_titlebar(&mut self, set: crate::titlebar::TitlebarSet) {
        if set == self.titlebar.set { return; }
        self.titlebar.set = set;
        self.titlebar.set_changed = true;
        if let Some((chrome, titlebar)) = self.titlebar.headless {
            self.headless_chrome(chrome, Some(titlebar), false);
        }
    }

    /// Whether the titlebar the view set changed since the shell last asked.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_titlebar_changed(&mut self) -> bool {
        std::mem::take(&mut self.titlebar.set_changed)
    }

    /// Notes whether a secondary press at (x, y) landed on a bare `.window_drag()` region of a
    /// custom titlebar, where its release opens the window menu.
    pub(super) fn secondary_region_down(&mut self, x: f32, y: f32, captured: bool) {
        let region = !captured && self.titlebar.caption.is_some()
            && self.primary_target(x, y).is_some_and(|t| self.bare_drag_region(t));
        self.titlebar.act(Event::SecondaryDown { region });
    }

    /// A secondary release that ends the press `secondary_region_down` noted on a drag region
    /// asks the shell for the window menu.
    pub(super) fn secondary_region_up(&mut self, x: f32, y: f32) {
        // A release with no press on a region has nothing to ask for, as `take` returned false.
        if !self.titlebar.press.secondary_armed() { return; }
        let region = self.primary_target(x, y).is_some_and(|t| self.bare_drag_region(t));
        self.titlebar.act(Event::SecondaryUp { region });
    }

    /// A `.window_drag()` element with nothing of its own to do with a press: a press there
    /// moves the window, and a secondary click opens the window menu.
    pub(super) fn bare_drag_region(&self, t: usize) -> bool {
        let n = &self.nodes[t];
        n.window_drag == crate::el::WindowDrag::Drag && n.handlers.on_press.is_none() && n.handlers.on_double_press.is_none() && n.handlers.on_drag.is_none()
            && n.handlers.drag_out.is_none() && self.textbox_input(t).is_none()
    }
}
