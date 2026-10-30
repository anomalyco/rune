//! A resizable, collapsible pane's state, owned by the app so it can persist it, and how
//! panes move: [`PaneMotion`] for docking, [`SidebarPeek`] for a closed sidebar's peek.

use std::time::Duration;

use rune::{Anim, Curve, Easing, KeyEvent, Spring, Velocity};

use crate::Kit;

/// A side pane's width and whether it shows: a sidebar's or an inspector's. The app owns it
/// (a field of its state), so it can save the width and restore it with [`PaneState::new`].
/// Widths are whole points within the pane's range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneState {
    width: f32,
    open: bool,
    min: f32,
    max: f32,
    resizing: bool,
    peek: Peek,
    /// The width and the width it was made with (the default), as asked, before clamping, so a
    /// later `range` can widen to them.
    asked: [f32; 2],
}

/// Where a closed sidebar's peek stands. Kept after it ends, so the view knows which way the
/// sidebar left: sliding out of a peek, or closing from docked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Peek {
    /// Not peeked since it last closed.
    None,
    /// Floating over the content.
    Shown,
    /// Sliding back out of a peek (or out of it already).
    Left,
    /// Docked from a peek: open, and still raised over the content until it closes again.
    Docked,
}

impl PaneState {
    /// Range of a sidebar unless set: 160 to 480 points.
    pub const MIN: f32 = 160.0;
    pub const MAX: f32 = 480.0;

    /// An open pane `width` points wide, which double-clicking its edge also returns to.
    pub fn new(width: f32) -> PaneState {
        let mut p = PaneState { width, open: true, min: Self::MIN, max: Self::MAX, resizing: false, peek: Peek::None, asked: [width; 2] };
        p.clamp();
        p
    }
    /// Its width range. The width and the default clamp into it, from the values given to
    /// [`Self::new`] and [`Self::with_width`], so `new(640.0).range(360.0, 960.0)` is 640.
    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.min = min.max(1.0);
        self.max = max.max(self.min);
        self.width = self.asked[0];
        self.clamp();
        self
    }
    /// Starts collapsed (a saved state, or an inspector that opens on demand).
    pub fn collapsed(mut self) -> Self {
        self.open = false;
        self
    }
    /// A saved width, clamped and rounded (the width it starts with; the default stays).
    pub fn with_width(mut self, width: f32) -> Self {
        if width.is_finite() {
            self.asked[0] = width;
        }
        self.set_width(width);
        self
    }

    pub fn width(&self) -> f32 {
        self.width
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn min_width(&self) -> f32 {
        self.min
    }
    pub fn max_width(&self) -> f32 {
        self.max
    }
    pub fn default_width(&self) -> f32 {
        self.asked[1].round().clamp(self.min, self.max)
    }
    /// Whether its edge is being dragged.
    pub fn is_resizing(&self) -> bool {
        self.resizing
    }
    /// Points it takes in the layout: its width when open, else 0.
    pub fn shown_width(&self) -> f32 {
        if self.open { self.width } else { 0.0 }
    }

    /// Clamps to the range and rounds to a whole point. Doesn't open the pane.
    pub fn set_width(&mut self, width: f32) {
        if width.is_finite() {
            self.width = width.round().clamp(self.min, self.max);
        }
    }
    /// Opening a peeking sidebar docks it where it floats; closing ends any peek.
    pub fn set_open(&mut self, open: bool) {
        if open && !self.open {
            self.peek = if self.peek == Peek::Shown { Peek::Docked } else { Peek::None };
        } else if !open && self.open {
            self.peek = Peek::None;
        }
        self.open = open;
    }
    pub fn toggle(&mut self) {
        self.set_open(!self.open);
    }

    /// Whether the closed sidebar floats over the content (a `SplitView` with
    /// [`SidebarPeek`]).
    pub fn is_peeking(&self) -> bool {
        !self.open && self.peek == Peek::Shown
    }
    /// Slides a peeking sidebar away (Escape, say). Leaving it with the pointer, or the window
    /// losing focus, ends a peek too.
    pub fn end_peek(&mut self) {
        if self.is_peeking() {
            self.peek = Peek::Left;
        }
    }
    /// Shows the closed sidebar as a peek, over the content.
    pub fn peek(&mut self) {
        if !self.open {
            self.peek = Peek::Shown;
        }
    }
    pub(super) fn peek_phase(&self) -> Peek {
        self.peek
    }
    /// Back to the width it was made with (double-clicking its edge).
    pub fn reset_width(&mut self) {
        self.width = self.default_width();
    }
    /// Where dragging its edge to `width` leaves it: clamped, or collapsed (keeping its last
    /// width) once dragged below half its minimum, as AppKit's split views do. Dragging back
    /// out opens it again.
    pub fn drag_to(&mut self, width: f32) {
        if width < self.min * 0.5 {
            self.set_open(false);
        } else {
            self.set_open(true);
            self.set_width(width);
        }
    }

    /// A drag on its edge began or ended. The grip stays while a drag that collapsed the pane
    /// goes on, so the drag can open it again.
    pub(super) fn set_resizing(&mut self, on: bool) {
        self.resizing = on;
    }

    fn clamp(&mut self) {
        self.width = self.width.round().clamp(self.min, self.max);
    }
}

impl Default for PaneState {
    /// A 248 point sidebar, open.
    fn default() -> Self {
        PaneState::new(248.0)
    }
}

/// How side panes open and close: `PaneMotion::new().spring(Spring::new(0.3, 0.0))
/// .scale_with_width(480.0, 0.15, 0.3).fade_tail(0.15)`. A `SplitView` gives both its panes the
/// same motion, so they move as one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneMotion {
    spring: Option<Spring>,
    scale: Option<[f32; 3]>,
    fade_tail: Option<f32>,
    reduced_fade: f32,
}

impl PaneMotion {
    /// The kit's disclosure spring at every width; contents fade out early in a close and
    /// in late in an open; Reduce Motion snaps.
    pub fn new() -> PaneMotion {
        PaneMotion { spring: None, scale: None, fade_tail: None, reduced_fade: 0.0 }
    }
    /// The spring a pane's width moves on (by default the kit's disclosure spring).
    pub fn spring(mut self, s: Spring) -> Self {
        self.spring = Some(s);
        self
    }
    /// Scales the spring's duration by the square root of the pane's width over `reference`
    /// points, kept within `min..=max` seconds, so a narrow pane doesn't crawl and a wide one
    /// doesn't snap.
    pub fn scale_with_width(mut self, reference: f32, min: f32, max: f32) -> Self {
        let min = min.max(0.001);
        self.scale = Some([reference.max(1.0), min, max.max(min)]);
        self
    }
    /// Fades contents only across the last `fraction` of the way to closed: solid through most
    /// of a close, and there almost at once in an open.
    pub fn fade_tail(mut self, fraction: f32) -> Self {
        self.fade_tail = Some(fraction.clamp(0.001, 1.0));
        self
    }
    /// Under Reduce Motion the pane snaps; its contents fade over `seconds` (0 snaps them too).
    pub fn reduced_motion_fade(mut self, seconds: f32) -> Self {
        self.reduced_fade = seconds.max(0.0);
        self
    }

    /// The spring a pane `width` points wide moves on.
    pub fn spring_for(&self, k: &Kit, width: f32) -> Spring {
        let base = self.spring.unwrap_or(k.motion.disclosure);
        match self.scale {
            Some([reference, min, max]) => base.with_duration((base.duration * (width.max(0.0) / reference).sqrt()).clamp(min, max)),
            None => base,
        }
    }

    /// The curve a pane's contents fade on as it opens (`open`) or closes on `spring`.
    pub(super) fn fade(&self, spring: Spring, open: bool, reduced: bool) -> Anim {
        if reduced {
            return if self.reduced_fade > 0.0 { Curve::tween(self.reduced_fade, Easing::EASE_IN_OUT).into() } else { Anim::INSTANT };
        }
        match self.fade_tail {
            Some(f) => tail_fade(spring, f, open),
            None if open => Curve::tween(spring.duration * 0.5, Easing::EASE_OUT).into(),
            None => Curve::tween(spring.duration * 0.35, Easing::EASE_IN).into(),
        }
    }
}

impl Default for PaneMotion {
    fn default() -> Self {
        Self::new()
    }
}

/// Opacity `min(1, open / fraction)` for a pane whose open fraction moves on `spring`. Closing,
/// it holds until the pane is `fraction` from closed, then follows the spring's own state from
/// there scaled by `1 / fraction`, which is exact; opening, it reaches 1 as the pane passes
/// `fraction` open, on an ease-in close to the spring's start.
pub(super) fn tail_fade(spring: Spring, fraction: f32, open: bool) -> Anim {
    let step = 1.0 / 480.0;
    if open {
        // Opening, the pane passes `fraction` open early, on the steep start of the spring.
        let mut t = 0.0;
        let mut x = 0.0;
        while x < fraction && t < 4.0 {
            t += step;
            x = spring.sample(0.0, 0.0, 1.0, t).0;
        }
        return Curve::tween(t.max(step), Easing::EASE_IN).into();
    }
    // Closing, the open fraction (1 -> 0) reaches `fraction` at `t`, found to well under a
    // microsecond so the fade leaves 1 exactly then.
    let mut t: f32 = 0.0;
    while spring.sample(1.0, 0.0, 0.0, t).0 > fraction && t < 4.0 {
        t += step;
    }
    let (mut lo, mut hi) = ((t - step).max(0.0), t);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if spring.sample(1.0, 0.0, 0.0, mid).0 > fraction { lo = mid } else { hi = mid }
    }
    let t = hi;
    let v = spring.sample(1.0, 0.0, 0.0, t).1;
    let mut a = Anim::new(Curve::Spring(spring));
    a.delay = t;
    a.velocity = Velocity::Uniform(v / fraction);
    a
}

/// A closed sidebar that peeks: resting the pointer on the window's leading edge, or on the
/// sidebar's toggle, slides the sidebar in over the content as a floating surface, without
/// moving the layout. Leaving it closes it again; pressing the toggle docks it where it is.
/// `SplitView::sidebar_peek(SidebarPeek::new().edge(16.0, ms(40)).toggle_delay(ms(80)))`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarPeek {
    pub(super) edge: f32,
    pub(super) edge_delay: Duration,
    pub(super) toggle_delay: Duration,
    pub(super) below: Option<f32>,
    pub(super) margin: f32,
    pub(super) grace: Duration,
    pub(super) spring: Option<Spring>,
    pub(super) reduced_fade: f32,
}

impl SidebarPeek {
    /// A 16 point edge zone and a 12 point margin; 50 ms on the edge, 100 ms on the toggle, and
    /// 150 ms of grace before it closes; the kit's control spring.
    pub fn new() -> SidebarPeek {
        SidebarPeek {
            edge: 16.0,
            edge_delay: Duration::from_millis(50),
            toggle_delay: Duration::from_millis(100),
            below: None,
            margin: 12.0,
            grace: Duration::from_millis(150),
            spring: None,
            reduced_fade: 0.15,
        }
    }
    /// The zone along the window's leading edge, `width` points into the content past the
    /// frame inset, and how long the pointer rests in it before the sidebar peeks.
    pub fn edge(mut self, width: f32, delay: Duration) -> Self {
        self.edge = width.max(0.0);
        self.edge_delay = delay;
        self
    }
    /// How long the pointer rests on the closed sidebar's toggle before it peeks. The toggle is
    /// crossed on the way to other title bar controls, so this is usually longer than the edge's.
    pub fn toggle_delay(mut self, delay: Duration) -> Self {
        self.toggle_delay = delay;
        self
    }
    /// Where the edge zone starts, in points from the window's top (default: below the title
    /// bar band, which holds the traffic lights and the toggle).
    pub fn below(mut self, y: f32) -> Self {
        self.below = Some(y.max(0.0));
        self
    }
    /// How far past the sidebar and toggle the pointer may stray, and for how long, before the
    /// peek closes.
    pub fn hold(mut self, margin: f32, grace: Duration) -> Self {
        self.margin = margin.max(0.0);
        self.grace = grace;
        self
    }
    /// The spring the peek slides on (by default the kit's control spring).
    pub fn spring(mut self, s: Spring) -> Self {
        self.spring = Some(s);
        self
    }
    /// Under Reduce Motion the peek fades in place over `seconds` instead of sliding.
    pub fn reduced_motion_fade(mut self, seconds: f32) -> Self {
        self.reduced_fade = seconds.max(0.0);
        self
    }
}

impl Default for SidebarPeek {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether `ev` is the sidebar's show/hide shortcut: ⇧⌘B (Ctrl+Shift+B off macOS), and on macOS
/// also ⌃⌘S, AppKit's Toggle Sidebar. Call it from the app's key handler (`AppSpec::on_key`)
/// and toggle the sidebar's [`PaneState`] when it matches.
pub fn is_sidebar_toggle(ev: &KeyEvent) -> bool {
    let m = ev.mods;
    let shift_b = ev.is_command('b') && m.shift && !m.alt && !(cfg!(target_os = "macos") && m.ctrl);
    let ctrl_s = cfg!(target_os = "macos") && ev.is_command('s') && m.ctrl && !m.shift && !m.alt;
    shift_b || ctrl_s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peek_docks_where_it_floats_and_ends_when_closed() {
        let mut p = PaneState::new(276.0).collapsed();
        p.peek();
        assert!(p.is_peeking());
        p.end_peek();
        assert!(!p.is_peeking() && p.peek_phase() == Peek::Left);
        p.peek();
        p.toggle();
        assert!(p.is_open() && !p.is_peeking() && p.peek_phase() == Peek::Docked, "docks from the peek");
        p.peek();
        assert_eq!(p.peek_phase(), Peek::Docked, "an open sidebar doesn't peek");
        p.toggle();
        assert_eq!(p.peek_phase(), Peek::None, "a close from docked isn't a peek");
        p.toggle();
        assert_eq!(p.peek_phase(), Peek::None, "an open without a peek isn't docked from one");
    }

    #[test]
    fn the_default_follows_the_range_from_the_width_it_was_made_with() {
        let p = PaneState::new(640.4).with_width(500.0);
        assert_eq!((p.width(), p.default_width()), (480.0, 480.0));
        let p = p.range(360.0, 960.0);
        assert_eq!((p.width(), p.default_width()), (500.0, 640.0));
        let p = p.range(100.0, 200.0).range(360.0, 960.0);
        assert_eq!((p.width(), p.default_width()), (500.0, 640.0), "ranges don't accumulate");
        let mut p = p;
        p.set_width(400.0);
        p.reset_width();
        assert_eq!(p.width(), 640.0);
    }

    #[test]
    fn the_tail_fade_follows_the_spring_across_the_last_fraction() {
        let spring = Spring::new(0.3, 0.0);
        let a = tail_fade(spring, 0.15, false);
        let Velocity::Uniform(v) = a.velocity else { panic!() };
        // From the delay on, the faded opacity is the pane's open fraction over 0.15.
        for t in [0.02, 0.05, 0.1, 0.2] {
            let open = spring.sample(1.0, 0.0, 0.0, a.delay + t).0;
            let faded = spring.sample(1.0, v, 0.0, t).0;
            assert!((open / 0.15 - faded).abs() < 0.01, "t {t}: {} vs {faded}", open / 0.15);
        }
        assert!((spring.sample(1.0, 0.0, 0.0, a.delay).0 - 0.15).abs() < 0.01);
        let Curve::Tween { duration, .. } = tail_fade(spring, 0.15, true).curve else { panic!() };
        assert!(duration > 0.01 && duration < 0.06, "an open is solid almost at once: {duration}");
    }

    #[test]
    fn the_motion_scales_with_width_within_its_bounds() {
        let k = Kit::neutral(true);
        let m = PaneMotion::new().spring(Spring::new(0.3, 0.0)).scale_with_width(480.0, 0.15, 0.3);
        assert!((m.spring_for(&k, 276.0).duration - 0.3 * (276.0f32 / 480.0).sqrt()).abs() < 1e-5);
        assert_eq!(m.spring_for(&k, 1000.0).duration, 0.3);
        assert_eq!(m.spring_for(&k, 50.0).duration, 0.15);
        assert_eq!(PaneMotion::new().spring_for(&k, 50.0), k.motion.disclosure);
    }

    #[test]
    fn widths_clamp_round_and_collapse_past_half_the_minimum() {
        let mut p = PaneState::new(276.0).range(136.0, 640.0);
        p.set_width(1000.0);
        assert_eq!(p.width(), 640.0);
        p.set_width(100.4);
        assert_eq!(p.width(), 136.0);
        p.set_width(300.6);
        assert_eq!(p.width(), 301.0);
        p.drag_to(80.0);
        assert!(p.is_open() && p.width() == 136.0, "above half the minimum it clamps");
        p.drag_to(60.0);
        assert!(!p.is_open() && p.width() == 136.0, "below half it collapses and keeps its width");
        assert_eq!(p.shown_width(), 0.0);
        p.drag_to(200.0);
        assert!(p.is_open() && p.width() == 200.0);
        p.reset_width();
        assert_eq!(p.width(), 276.0);
        assert_eq!(PaneState::new(20.0).width(), PaneState::MIN);
        p.set_width(f32::NAN);
        assert_eq!(p.width(), 276.0);
    }
}
