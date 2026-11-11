//! What a commit asks of the window besides its pixels: the content height a window that
//! fits its content asks for (`App::fit_content_height`), and the title (`Tx::set_title`).
use super::*;

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

impl<S: 'static> Runtime<S> {
    /// Asks the window for the content's new height, on this commit's spring. The first
    /// commit, a viewport change and `Tx::instant` jump.
    pub(super) fn fit_window(&mut self, height: f32, tx: &Tx, first: bool) {
        let height = height.ceil_to(self.env.scale).max(1.0);
        if !self.fit.ask(height, self.viewport.1) { return; }
        let jump = first || tx.instant || tx.relayout || self.reduced_motion;
        // Only a macOS window and headless runs move the frame on a spring.
        if !jump && self.window_resting && (cfg!(target_os = "macos") || self.wall_start.is_none()) { self.fit.hold = FitHold::Asked; }
        let request = crate::window::frame::FrameRequest {
            size: Some((self.viewport.0, height)), animate: !jump,
            spring: (!jump).then(|| tx.motion.unwrap_or(motion::DEFAULT)), center: false,
        };
        self.window_frame = Some(self.window_frame.unwrap_or_default().merge(request));
    }

    /// The root's natural height at the current width, in window points (the view's times its
    /// zoom), measured without committing, so a window that fits its content can open at that
    /// height.
    pub fn content_height(&mut self) -> f32 {
        let measured = self.measure_pass(&Tx::default(), false);
        let height = measured.nodes.first().map_or(self.viewport.1, |root| root.local.h);
        self.discard_measured(measured);
        (height * self.zoom.k).ceil_to(self.zoom.window_scale).max(1.0)
    }

    /// Someone resized a window that fits its content (not a page's band) to `height`: it
    /// goes back to the content's height.
    pub(super) fn fit_resized(&mut self, height: f32) -> bool {
        self.fit_height && self.page.is_none() && self.window_resting && self.fit.resized(height, self.viewport.1)
    }

    /// Whether this frame holds its motion for the window (see [`FitHold`]).
    pub(super) fn fit_frame_holds(&mut self) -> bool { self.fit.frame_holds() }
}

#[cfg(test)]
#[path = "window_requests_tests.rs"]
mod tests;
