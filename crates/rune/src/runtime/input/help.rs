//! Tooltips (`El::help`), as SwiftUI's `.help`. Help elements are hit-tested on their own, so
//! they take no hover or press from anything. The pointer resting on one for `help_delay`
//! shows its tooltip under the element; a press, scroll or key hides it until the pointer
//! leaves the element, as does the pointer leaving. The tooltip draws over everything and
//! follows its element while that moves under the pointer. A text's links can carry their own
//! help (`Link::help`), hit-tested on their glyphs and shown under the link.
//!
//! A macOS window shows AppKit's own tooltips instead (`set_native_help`): the host registers
//! [`Runtime::help_regions`] with its view and AppKit owns the delay, look and hiding, unless
//! the view sets its own look (`Cx::help_look`), which Rune draws everywhere.
//!
//! Where the tooltip is in its life is one [`Tip`], stepped by [`step`] once a frame with what
//! the pointer is over, and on a dismissal; it mirrors `@opencode/ui`'s tooltip machine
//! (`ui/src/overlays/tooltip/machine.ts`) with Kobalte's open and skip delays. A press on the
//! element dismisses its tooltip; a popup or modal over the element suppresses it until
//! the pointer leaves. The look's fade (`HelpLook::fade`) plays as the tooltip opens and
//! closes, the closing one fading out under the next.
use crate::machine::Effects;

use super::*;

#[derive(Default)]
pub(in crate::runtime) struct HelpState {
    state: Tip,
    /// The tooltip showing this frame: its text and box in points.
    tip: Option<(String, [f32; 4])>,
    /// The open tooltip as it last drew, for its fade out.
    drawn: Option<Drawn>,
    /// A closed tooltip fading out, and when (clock seconds) it closed.
    leaving: Option<(Drawn, f64)>,
    /// The host shows system tooltips from `help_regions`.
    native: bool,
    regions: Vec<([f32; 4], String)>,
}

/// A help element, or one link of its text, by path.
#[derive(Clone, Debug, PartialEq)]
struct Target {
    path: String,
    link: Option<usize>,
}

/// Where the tooltip is in its life. Times are clock seconds.
#[derive(Clone, Debug, Default, PartialEq)]
enum Tip {
    /// No help element under the pointer, or a tooltip hid more than the skip delay ago.
    #[default]
    Idle,
    /// A tooltip hid as the pointer left its element: until `until`, the next one opens at once.
    Warm { until: f64 },
    /// The pointer has rested on `target` since this clock time.
    Waiting { target: Target, since: f64 },
    /// `target`'s tooltip shows, fading in from `since`.
    Open { target: Target, since: f64 },
    /// A press, a scroll or key, or a popup over `target` hid its tooltip until the
    /// pointer leaves it.
    Suppressed { target: Target },
}

impl Tip {
    fn target(&self) -> Option<&Target> {
        match self {
            Tip::Waiting { target, .. } | Tip::Open { target, .. } | Tip::Suppressed { target } => Some(target),
            Tip::Idle | Tip::Warm { .. } => None,
        }
    }
}

/// What the pointer is over as a frame draws.
#[derive(Debug, PartialEq)]
enum Over {
    /// No pointer in the window: it left, or a menu took it.
    Away,
    /// No help element.
    Nothing,
    Help(Target),
    /// A help element, under a popup or modal that the pointer can't reach it through.
    Covered(Target),
    /// The help element the keyboard focused, with the pointer on no other (`HelpLook::on_focus`).
    Focused(Target),
    /// A help element held open (`El::help_open`), whatever the pointer does.
    Forced(Target),
}

#[derive(Debug, PartialEq)]
enum Event {
    Frame { over: Over, now: f64 },
    /// A press, a scroll or a key.
    Dismiss,
}

#[derive(Debug, PartialEq)]
enum Effect {
    /// Wake at this clock time, when a wait ends.
    WakeAt(f64),
    /// The open tooltip closed: fade it out.
    FadeOut,
}

/// The look's open and skip delays, in seconds.
#[derive(Clone, Copy)]
struct Delays {
    open: f64,
    skip: f64,
}

fn step(tip: &mut Tip, event: Event, delays: Delays) -> Effects<Effect> {
    let (over, now) = match event {
        Event::Dismiss => return suppress(tip),
        Event::Frame { over, now } => (over, now),
    };
    let effects = match over {
        // A menu the element opened took the pointer: the tooltip stays hidden over it after.
        Over::Away if matches!(tip, Tip::Suppressed { .. }) => Effects::none(),
        Over::Forced(at) if matches!(tip, Tip::Open { target, .. } if *target == at) => Effects::none(),
        Over::Forced(at) => open_now(tip, at, now),
        Over::Focused(at) if tip.target() == Some(&at) => Effects::none(),
        Over::Focused(at) => open_now(tip, at, now),
        Over::Covered(at) if tip.target() == Some(&at) => suppress(tip),
        Over::Help(at) if tip.target() == Some(&at) => Effects::none(),
        Over::Help(at) => leave(tip, Some(at), now, delays),
        Over::Away | Over::Nothing | Over::Covered(_) => leave(tip, None, now, delays),
    };
    match std::mem::take(tip) {
        Tip::Warm { until } if now > until => {}
        // The wait is over (or there was none: a zero delay opens in the frame it starts).
        Tip::Waiting { target, since } if now + 1e-6 >= since + delays.open => *tip = Tip::Open { target, since: now },
        other => *tip = other,
    }
    effects
}

/// The pointer left the tooltip's element for `at` (or nothing): an open tooltip closes, and
/// within the skip delay of one closing the next opens at once.
fn leave(tip: &mut Tip, at: Option<Target>, now: f64, delays: Delays) -> Effects<Effect> {
    let open = matches!(tip, Tip::Open { .. });
    let warm = (open && delays.skip > 0.0) || matches!(tip, Tip::Warm { until } if now <= *until);
    let mut effects = if open { Effects::one(Effect::FadeOut) } else { Effects::none() };
    *tip = match at {
        Some(target) if warm => Tip::Open { target, since: now },
        Some(target) => {
            let due = now + delays.open;
            effects.push(Effect::WakeAt(due));
            Tip::Waiting { target, since: now }
        }
        None if open && delays.skip > 0.0 => Tip::Warm { until: now + delays.skip },
        None if warm => std::mem::take(tip),
        None => Tip::Idle,
    };
    effects
}

/// `at`'s tooltip shows now, with no wait: the one open closes.
fn open_now(tip: &mut Tip, at: Target, now: f64) -> Effects<Effect> {
    let effects = if matches!(tip, Tip::Open { .. }) { Effects::one(Effect::FadeOut) } else { Effects::none() };
    *tip = Tip::Open { target: at, since: now };
    effects
}

/// Hide the tooltip until the pointer leaves its element.
fn suppress(tip: &mut Tip) -> Effects<Effect> {
    match std::mem::take(tip) {
        Tip::Open { target, .. } => {
            *tip = Tip::Suppressed { target };
            Effects::one(Effect::FadeOut)
        }
        Tip::Waiting { target, .. } | Tip::Suppressed { target } => {
            *tip = Tip::Suppressed { target };
            Effects::none()
        }
        idle @ (Tip::Idle | Tip::Warm { .. }) => {
            *tip = idle;
            Effects::none()
        }
    }
}

/// A tooltip as drawn: its text, the box it hangs from (window points, x0 y0 x1 y1), the side
/// of that box it asks for (None: under it), and its opacity.
#[derive(Clone, Debug)]
struct Drawn {
    text: String,
    anchor: [f32; 4],
    side: Option<crate::Side>,
    alpha: f32,
    scale: f32,
}

/// One row of a tooltip as laid out: its words (when it has any) `tw` by `th` points, its
/// keycaps and their width with the lead before them, and the row's height.
struct TipRow {
    words: bool,
    layout: std::rc::Rc<parley::Layout<crate::text::Ink>>,
    tw: f32,
    th: f32,
    ch: f32,
    caps: Vec<(std::rc::Rc<parley::Layout<crate::text::Ink>>, f32)>,
    keys_w: f32,
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// `a` minus `b` (x0 y0 x1 y1), as up to four rects: the bands above and below `b`, then the
/// pieces left and right of it.
fn subtract(a: [f32; 4], b: [f32; 4]) -> Vec<[f32; 4]> {
    if !overlaps(a, b) { return vec![a]; }
    let (y0, y1) = (b[1].max(a[1]), b[3].min(a[3]));
    [[a[0], a[1], a[2], y0], [a[0], y1, a[2], a[3]], [a[0], y0, b[0].max(a[0]), y1], [b[2].min(a[2]), y0, a[2], y1]]
        .into_iter().filter(|r| r[2] > r[0] && r[3] > r[1]).collect()
}

impl capture::PointerState {
    /// The host shows AppKit's tooltips: on macOS, while the view keeps the system look.
    #[inline]
    pub(super) fn native_help(&self, custom_look: bool) -> bool { self.help.native && !custom_look }

    /// The host shows the system's own tooltips for [`Runtime::help_regions`], so Rune draws none.
    #[cfg(target_os = "macos")]
    #[inline]
    pub(in crate::runtime) fn set_native_help(&mut self, on: bool) { self.help.native = on; }

    /// Each visible help element's rect (window points, x0 y0 x1 y1) and text, in paint order,
    /// for a host that shows system tooltips.
    #[inline]
    pub(in crate::runtime) fn help_regions(&self) -> &[([f32; 4], String)] { &self.help.regions }

    /// The tooltip Rune shows now: its text and box (window points, x y w h).
    #[inline]
    pub(in crate::runtime) fn help_tooltip(&self) -> Option<(&str, [f32; 4])> {
        self.help.tip.as_ref().map(|(t, r)| (t.as_str(), *r))
    }

    /// The open tooltip fades in, or a closed one fades out, so frames keep coming.
    #[inline]
    fn tooltip_fading(&self, clock: f64, fade: [f64; 2]) -> bool {
        let opening = matches!(self.help.state, Tip::Open { since, .. } if clock < since + fade[0]);
        opening || self.help.leaving.as_ref().is_some_and(|(_, at)| clock < at + fade[1])
    }

    /// A tooltip waits for a frame at or after its delay to draw, or fades in or out.
    #[inline]
    pub(super) fn tooltip_waiting(&self, clock: f64, open_delay: f64, fade: [f64; 2]) -> bool {
        matches!(self.help.state, Tip::Waiting { since, .. } if clock + 1e-6 < since + open_delay)
            || self.tooltip_fading(clock, fade)
    }

    /// Steps the tooltip's state; returns a wake delay when a wait starts.
    fn step_help(&mut self, event: Event, delays: Delays, clock: f64, fade_out: bool) -> Option<std::time::Duration> {
        let mut wake = None;
        for effect in step(&mut self.help.state, event, delays) {
            match effect {
                Effect::WakeAt(due) => wake = Some(std::time::Duration::from_secs_f64((due - clock).max(0.0))),
                Effect::FadeOut => {
                    self.help.tip = None;
                    self.help.leaving = self.help.drawn.take().filter(|_| fade_out).map(|d| (d, clock));
                }
            }
        }
        wake
    }
}

impl RuntimeCore {
    /// The look tooltips draw in now: the view's, or the system's for this window.
    fn help_look(&self) -> super::HelpLook {
        self.commit.defaults.help.unwrap_or_else(|| super::HelpLook::system(self.backdrop_dark().unwrap_or(self.env.system_dark)))
    }

    /// The host shows the system's own tooltips for [`Runtime::help_regions`], so Rune draws none.
    #[cfg(target_os = "macos")]
    #[inline]
    pub(crate) fn set_native_help(&mut self, on: bool) { self.pointer.set_native_help(on); }

    /// Each visible help element's rect (window points, x0 y0 x1 y1) and text, in paint order,
    /// for a host that shows system tooltips.
    #[inline]
    pub fn help_regions(&self) -> &[([f32; 4], String)] { self.pointer.help_regions() }

    /// The tooltip Rune shows now: its text and box (window points, x y w h).
    #[inline]
    pub fn help_tooltip(&self) -> Option<(&str, [f32; 4])> { self.pointer.help_tooltip() }

    /// A tooltip waits for a frame at or after its delay to draw, or fades in or out.
    pub(in crate::runtime) fn tooltip_waiting(&self) -> bool {
        let look = self.help_look();
        self.pointer.tooltip_waiting(self.clock, look.delay.unwrap_or(self.help_delay), look.fade)
    }

    /// The look's delays, the runtime's `help_delay` where it sets none.
    fn help_delays(&self) -> Delays {
        let look = self.help_look();
        Delays { open: look.delay.unwrap_or(self.help_delay), skip: look.skip }
    }

    /// During `build`: the line boxes of node `i`'s links that have help, for border box
    /// `rect` and clipped to `clip` (points, x0 y0 x1 y1), after the first `seq` hits.
    pub(in crate::runtime) fn push_link_help_hits(&mut self, i: usize, rect: [f32; 4], clip: [f32; 4], seq: usize) {
        let Some(t) = self.tree.nodes[i].text.as_ref() else { return };
        if !t.cold.links.iter().any(|l| l.help.is_some()) { return; }
        let id = self.tree.nodes[i].id;
        for (k, r) in self.link_boxes(i, rect) {
            if self.tree.nodes[i].text.as_ref().and_then(|t| t.cold.links.get(k)).is_none_or(|l| l.help.is_none()) { continue; }
            let c = [r[0].max(clip[0]), r[1].max(clip[1]), r[2].min(clip[2]), r[3].min(clip[3])];
            self.hit.push_help_hit(id, c, seq, Some(k));
        }
    }

    /// The help text of node `i`, or of its link `link`.
    fn help_text(&self, i: usize, link: Option<usize>) -> Option<&String> {
        match link {
            Some(k) => self.tree.nodes[i].text.as_ref()?.cold.links.get(k)?.help.as_ref(),
            None => self.tree.nodes[i].help.as_ref(),
        }
    }

    #[inline]
    pub(in crate::runtime) fn clear_help_hits(&mut self) { self.hit.clear_help_hits(); }

    /// A scroll or key: hide the tooltip until the pointer leaves its element.
    pub(in crate::runtime) fn dismiss_help(&mut self) {
        if self.pointer.help.state.target().is_some() {
            let look = self.help_look();
            let delays = Delays { open: look.delay.unwrap_or(self.help_delay), skip: look.skip };
            let fade_out = look.fade[1] > 0.0 && !self.reduced_motion;
            if let Some(delay) = self.pointer.step_help(Event::Dismiss, delays, self.clock, fade_out) {
                self.schedule_wakes([delay]);
            }
        }
    }

    /// The help element under (x, y): the topmost one, unless something interactive drawn over
    /// it (not inside it) is there too.
    fn help_at(&self, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
        self.help_under(x, y).filter(|u| u.2).map(|(h, link, _)| (h, link))
    }

    /// The topmost help element under (x, y), and whether the pointer reaches it: not behind a
    /// modal, and nothing interactive drawn over it (not inside it) is there too.
    fn help_under(&self, x: f32, y: f32) -> Option<(usize, Option<usize>, bool)> {
        let (id, seq, link) = self.hit.top_help(|id, r| self.hit_contains(id, r, x, y))?;
        let h = *self.tree.node_index.get(&id)?;
        if self.subtree_exiting(h) { return None; }
        let over = self.hit.top_hit_index(|id, r| self.hit_contains(id, r, x, y));
        let covered = over.is_some_and(|(k, top)| {
            k >= seq && self.tree.node_index.get(&top).is_some_and(|&t| !std::iter::successors(Some(t), |&p| self.tree.nodes[p].parent).any(|p| p == h))
        });
        Some((h, link, self.allowed(h) && !covered))
    }

    /// Whether moving the pointer to (x, y) changes the help element under it, so a host that
    /// skips frames for unchanged pixels still starts or ends a tooltip's wait.
    pub(in crate::runtime) fn help_target_changes(&self, x: f32, y: f32) -> bool {
        let target = self.pointer.help.state.target();
        if self.pointer.native_help(self.commit.defaults.help.is_some()) || !self.hit.has_help_hits() && target.is_none() { return false; }
        let at = self.help_at(x, y).map(|(i, k)| (self.tree.nodes[i].path.as_str(), k));
        at != target.map(|t| (t.path.as_str(), t.link))
    }

    /// What the pointer is over for the tooltip, and the node of a help element it reaches.
    fn help_over(&self) -> (Over, Option<usize>) {
        let target = |i: usize, link| Target { path: self.tree.nodes[i].path.clone(), link };
        // A tooltip held open shows first (a Copy button's "Copied").
        // Noted as the frame's help hits were pushed (`HitState::help_held`): no walk, and no
        // lookup at all when nothing is held open, as almost always.
        if let Some(i) = self.hit.help_held.and_then(|id| self.tree.node_index.get(&id).copied()) { return (Over::Forced(target(i, None)), Some(i)); }
        let (over, under) = self.pointer_help_over();
        // With the pointer on no help element, the one the keyboard focused shows its own.
        if matches!(over, Over::Nothing | Over::Away) && self.help_look().on_focus && self.kbd.focus.keyboard()
            && let Some(i) = self.focused_index()
            && self.tree.nodes[i].help.is_some() && self.allowed(i) && !self.subtree_exiting(i)
        {
            return (Over::Focused(target(i, None)), Some(i));
        }
        (over, under)
    }

    /// What the pointer is over for the tooltip.
    fn pointer_help_over(&self) -> (Over, Option<usize>) {
        // A menu is up or about to open and has the pointer: the element it came from keeps
        // its tooltip hidden.
        if !self.host.popups.is_empty() { return (self.pointer.help.state.target().cloned().map_or(Over::Away, Over::Covered), None); }
        let Some((x, y)) = self.pointer.pos.filter(|&(x, y)| self.in_viewport(x, y)) else { return (Over::Away, None) };
        let target = |i: usize, link| Target { path: self.tree.nodes[i].path.clone(), link };
        // While moving layout holds hover, the tooltip keeps its element, and nothing sliding
        // under the pointer starts a wait.
        if self.held_hover().is_some() {
            let kept = self.pointer.help.state.target().and_then(|t| Some((self.hover_live(&t.path)?, t.link)));
            return kept.map_or((Over::Nothing, None), |(i, k)| (Over::Help(target(i, k)), Some(i)));
        }
        let Some((i, link, reaches)) = self.help_under(x, y) else { return (Over::Nothing, None) };
        let at = target(i, link);
        // A press holds the pointer: a drag across other elements starts no tooltip.
        if self.pointer.captured() && self.pointer.help.state.target() != Some(&at) {
            return (Over::Nothing, None);
        }
        if reaches { (Over::Help(at), Some(i)) } else { (Over::Covered(at), None) }
    }

    /// The parts of help element `h`'s rect `r` that nothing interactive drawn over it (not
    /// inside it), from hit `seq` on, covers.
    fn unoccluded(&self, h: usize, r: [f32; 4], seq: usize) -> Vec<[f32; 4]> {
        let mut parts = vec![self.hit_screen_rect(self.tree.nodes[h].id, r)];
        for &(id, o) in self.hit.hits_since(seq) {
            let o = self.hit_screen_rect(id, o);
            if !parts.iter().any(|p| overlaps(*p, o)) { continue; }
            let Some(&t) = self.tree.node_index.get(&id) else { continue };
            if std::iter::successors(Some(t), |&p| self.tree.nodes[p].parent).any(|p| p == h) { continue; }
            parts = parts.into_iter().flat_map(|p| subtract(p, o)).collect();
        }
        parts
    }

    /// After `build`'s hits: track the rest, show or hide the tooltip and draw it.
    pub(in crate::runtime) fn update_help(&mut self) {
        self.pointer.help.regions.clear();
        if self.pointer.native_help(self.commit.defaults.help.is_some()) {
            // Nothing behind a modal, and only where nothing else is drawn over the element.
            let mut regions = Vec::new();
            for &(id, r, seq, link) in self.hit.help_hits() {
                let Some(&i) = self.tree.node_index.get(&id) else { continue };
                let Some(text) = self.help_text(i, link) else { continue };
                if self.subtree_exiting(i) || !self.allowed(i) { continue; }
                let words = crate::el::help_words(text);
                regions.extend(self.unoccluded(i, r, seq).into_iter().map(|p| (p, words.clone().into_owned())));
            }
            self.pointer.help.regions = regions;
            self.pointer.help.tip = None;
            self.pointer.help.state = Tip::Idle;
            self.pointer.help.drawn = None;
            self.pointer.help.leaving = None;
            return;
        }
        let (over, under) = self.help_over();
        let look = self.help_look();
        let delays = self.help_delays();
        let fade_out = look.fade[1] > 0.0 && !self.reduced_motion;
        if let Some(delay) = self.pointer.step_help(Event::Frame { over, now: self.clock }, delays, self.clock, fade_out) {
            self.schedule_wakes([delay]);
        }
        self.pointer.help.tip = None;
        let motion = !self.reduced_motion;
        // The closed tooltip fades out first, under the next one.
        if let Some((d, at)) = self.pointer.help.leaving.take() {
            let t = if motion && look.fade[1] > 0.0 { ((self.clock - at) / look.fade[1]) as f32 } else { 1.0 };
            if t < 1.0 {
                let e = rune_motion::Easing::EASE_IN.eval(t);
                let scale = d.scale + (look.fade_scale - d.scale) * e;
                self.draw_tooltip(&Drawn { alpha: d.alpha * (1.0 - e), scale, ..d.clone() });
                self.pointer.help.leaving = Some((d, at));
            }
        }
        self.pointer.help.drawn = None;
        if let Tip::Open { target, since } = &self.pointer.help.state {
            let (link, since) = (target.link, *since);
            let node = under.or_else(|| self.hover_live(&target.path));
            if let Some(i) = node && let Some(text) = self.help_text(i, link).cloned() {
                let e = self.drawn_box(i);
                let anchor = link.and_then(|k| self.link_boxes(i, e).into_iter().filter(|(j, _)| *j == k).map(|(_, r)| r)
                    .reduce(|u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
                let box_rect = anchor.unwrap_or(e);
                if box_rect[2] <= box_rect[0] && box_rect[3] <= box_rect[1] {
                    let path = self.tree.nodes[i].path.clone();
                    if self.paint.unresolved_anchor_warnings.insert((path.clone(), "<tooltip>".to_owned())) {
                        eprintln!("rune: tooltip on {path:?} skipped because its anchor rect is empty (never drawn at origin)");
                    }
                } else {
                    let side = link.is_none().then(|| self.tree.nodes[i].cold.get().and_then(|c| c.help_side)).flatten();
                    let t = if motion && look.fade[0] > 0.0 { ((self.clock - since) / look.fade[0]) as f32 } else { 1.0 };
                    let e_in = rune_motion::Easing::EASE_OUT.eval(t.clamp(0.0, 1.0));
                    let d = Drawn { text, anchor: box_rect, side, alpha: e_in, scale: look.fade_scale + (1.0 - look.fade_scale) * e_in };
                    let [x, y, w, h] = self.draw_tooltip(&d);
                    self.pointer.help.tip = Some((crate::el::help_words(&d.text).into_owned(), [x, y, w, h]));
                    self.pointer.help.drawn = Some(d);
                }
            }
        }
        if self.pointer.tooltip_fading(self.clock, look.fade) { self.schedule_wakes([std::time::Duration::ZERO]); }
    }

    /// Draws `d`'s text (and its keycaps) in a tooltip in the current look under its box
    /// (above it when there's no room below), or on the side it asks for (the opposite one when
    /// there's no room there), kept inside the window. Returns the tooltip's box at full size
    /// (window points, x y w h); it draws at `d.alpha`, scaled by `d.scale` about the middle
    /// of the edge that faces its box.
    fn draw_tooltip(&mut self, d: &Drawn) -> [f32; 4] {
        let e = d.anchor;
        debug_assert!(e[2] > e[0] || e[3] > e[1], "draw_tooltip called with empty anchor rect {e:?}");
        let s = self.env.scale;
        let look = self.help_look();
        let key = look.key;
        let [pad_x, pad_y] = look.padding;
        let (vw, vh) = self.viewport;
        let margin = look.margin;
        let wrap = look.max_w.unwrap_or(vw - 2.0 * margin - 2.0 * pad_x).max(1.0);
        // Each row (`El::help_row`) is its words, then its keycaps.
        let mut rows = Vec::new();
        for row in d.text.split(crate::el::HELP_ROW) {
            let mut parts = row.split(crate::el::HELP_KEY);
            let words = parts.next().unwrap_or_default();
            let caps: Vec<_> = parts.map(|k| {
                let k = if key.upper { k.to_uppercase() } else { k.to_owned() };
                let l = self.text.layout_rich(&k, &[], key.font, s, None);
                let w = ((l.full_width() / s).ceil() + 2.0 * key.pad_x).max(key.height);
                (l, w)
            }).collect();
            let keys_w = if caps.is_empty() { 0.0 } else {
                caps.iter().map(|(_, w)| w).sum::<f32>() + key.gap * (caps.len() - 1) as f32 + if words.is_empty() { 0.0 } else { key.lead }
            };
            let layout = self.text.layout_rich(words, &[], look.font, s, Some((wrap - keys_w).max(1.0) * s));
            let (tw, th) = if words.is_empty() { (0.0, 0.0) } else { ((layout.full_width() / s).ceil(), (layout.height() / s).ceil()) };
            let ch = if caps.is_empty() { th } else { th.max(key.height) };
            rows.push(TipRow { words: !words.is_empty(), layout, tw, th, ch, caps, keys_w });
        }
        let gaps = look.row_gap * rows.len().saturating_sub(1) as f32;
        let w = rows.iter().map(|r| r.tw + r.keys_w).fold(0.0, f32::max) + 2.0 * pad_x;
        let h = rows.iter().map(|r| r.ch).sum::<f32>() + gaps + 2.0 * pad_y;
        let [below, over] = look.gap;
        let (x, y) = match d.side {
            // Beside it, centred down it: right of it unless only the left has room, and the
            // other way round (`placement="right"` / `"left"`).
            Some(side @ (crate::Side::Left | crate::Side::Right)) => {
                let (left, right) = (e[0] - over - w, e[2] + over);
                let (fits_left, fits_right) = (left >= margin, right + w <= vw - margin);
                let on_left = if side == crate::Side::Left { fits_left || !fits_right } else { fits_left && !fits_right };
                let x = if on_left { left } else { right };
                (x, (e[1] + e[3] - h) * 0.5)
            }
            side => {
                let mut y = e[3] + below;
                if side == Some(crate::Side::Top) && e[1] - over - h >= margin {
                    y = e[1] - over - h;
                } else if y + h > vh - margin && e[1] - below - h >= margin {
                    y = e[1] - below - h;
                }
                ((e[0] + e[2] - w) * 0.5, y)
            }
        };
        let x = x.clamp(margin, (vw - w - margin).max(margin));
        let y = y.clamp(margin, (vh - h - margin).max(margin));
        let full = [x * s, y * s, w * s, h * s].map(|v| v.round());
        // Scales about the middle of the edge facing the element (Kobalte's transform origin).
        let (k, alpha) = (d.scale, d.alpha);
        let pivot = if x >= e[2] {
            [full[0], ((e[1] + e[3]) * 0.5 * s).clamp(full[1], full[1] + full[3])]
        } else if x + w <= e[0] {
            [full[0] + full[2], ((e[1] + e[3]) * 0.5 * s).clamp(full[1], full[1] + full[3])]
        } else {
            [((e[0] + e[2]) * 0.5 * s).clamp(full[0], full[0] + full[2]), if y >= e[3] { full[1] } else { full[1] + full[3] }]
        };
        let at_k = |p: [f32; 2]| [pivot[0] + (p[0] - pivot[0]) * k, pivot[1] + (p[1] - pivot[1]) * k];
        let rect_k = |r: [f32; 4]| { let [x, y] = at_k([r[0], r[1]]); [x, y, r[2] * k, r[3] * k] };
        let r = rect_k(full);
        let srgb = self.paint.srgb_blend;
        let radii = [look.radius * s * k; 4];
        let cast = super::shadow_paint::Caster {
            rect: r, radii, shape: radii, k_px: s, scale: s, opacity: alpha, srgb, clip: NO_CLIP, fade: [0.0; 4], hole: None,
            opaque: look.fill.a >= 1.0,
        };
        for sh in look.shadows.iter().flatten().filter(|sh| sh.color.a > 0.0) {
            super::shadow_paint::push_shadow(&mut self.instances, sh, sh.color, &cast);
        }
        let (ring_w, ring) = look.ring.map_or((0.0, Color::TRANSPARENT), |(rw, c)| ((rw * s).max(1.0), c));
        self.instances.push(Instance {
            rect: r,
            radii,
            fill: premul(look.fill, alpha, srgb),
            border: premul(ring, alpha, srgb),
            params: [ring_w, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: crate::render::pack_fade([0.0; 4], NO_CLIP, NO_CLIP),
        });
        let mut top = full[1] + pad_y * s;
        // In a right-to-left app a row reads from the right: its words, then its keycaps to their
        // left, as the mirrored tooltip of a browser in Arabic or Hebrew does.
        let rtl = crate::labels::rtl();
        for row in &rows {
            let mut at = full[0] + pad_x * s;
            let words_at = if rtl { full[0] + full[2] - pad_x * s - row.tw * s } else { at };
            if row.words {
                // Right-to-left words align to the right of the width they broke at: back into the box.
                let shift = if row.layout.is_rtl() { (wrap - row.keys_w).max(1.0) * s - row.layout.full_width() } else { 0.0 };
                self.draw_tip_text(&row.layout, at_k([words_at - shift, top + ((row.ch - row.th) * 0.5 * s).round()]), k, look.ink, alpha);
                if !rtl { at += (row.tw + key.lead) * s; }
            }
            // Right to left, the keycaps end the lead short of the words (`keys_w` holds it).
            if rtl { at = words_at - row.keys_w * s; }
            for (l, cw) in &row.caps {
                let cap = [at, top + ((row.ch - key.height) * 0.5 * s).round(), cw * s, key.height * s].map(|v| v.round());
                self.instances.push(Instance {
                    rect: rect_k(cap),
                    radii: [key.radius * s * k; 4],
                    fill: premul(key.fill, alpha, srgb),
                    border: [0.0; 4],
                    params: [0.0, KIND_RECT, 0.0, 1.0],
                    uv: [0.0; 4],
                    clip: NO_CLIP,
                    fade: crate::render::pack_fade([0.0; 4], NO_CLIP, NO_CLIP),
                });
                let lw = l.full_width();
                let origin = [cap[0] + ((cap[2] - lw) * 0.5).round(), cap[1] + ((cap[3] - l.height()) * 0.5).round()];
                self.draw_tip_text(l, at_k(origin), k, key.ink, alpha);
                at += (cw + key.gap) * s;
            }
            top += (row.ch + look.row_gap) * s;
        }
        [x, y, w, h]
    }

    /// Draws a tooltip's shaped text with its top left at `origin` (device pixels), scaled by
    /// `k` and at `alpha`.
    fn draw_tip_text(&mut self, layout: &parley::Layout<crate::text::Ink>, origin: [f32; 2], k: f32, ink: Color, alpha: f32) {
        let srgb = self.paint.srgb_blend;
        self.paint.glyph_scratch.clear();
        self.text.set_glyph_inks(ink, &[]);
        self.text.glyphs_for_layout_scaled(layout, origin, k, false, &mut self.paint.glyph_scratch);
        for q in &self.paint.glyph_scratch {
            self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, premul(ink, alpha, srgb), NO_CLIP, [0.0; 4]));
        }
    }
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;
