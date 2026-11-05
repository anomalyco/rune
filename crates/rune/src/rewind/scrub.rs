//! The scrubber: a frozen clip's frames over the window, one at a time, with the elements that
//! moved outlined and labelled, a track along the bottom that lights up where things moved, and
//! Send for the selected stretch. Opening it slows the window to a stop, then sets the frozen
//! window back on a dark stage with the track beneath it; closing reverses that.

use std::ops::Range;
use std::sync::Arc;

use crate::time::Instant;

use crate::edit::{Key, KeyEvent};
use crate::tok::{self, type_};
use crate::{Color, El, Tx, col, el, row, spacer, text, text_input};

use super::bundle::Bundle;
use super::clip::{Change, Clip, Shot};
use super::Rewind;

/// The bar along the window's foot, in points.
pub(crate) const BAR_H: f32 = 92.0;
pub(crate) const TRACK_H: f32 = 34.0;
pub(crate) const PAD: f32 = 12.0;
/// Frames of an element's path shown either side of the playhead when nothing is selected.
const PATH_SPAN: usize = 90;
/// Narrower than this, the bar drops its key hints.
const HINT_MIN_W: f32 = 760.0;
/// The margin round the shrunk window and the bar under it.
const GAP: f32 = 16.0;
/// Seconds the window takes to slow to a stop.
const SLOW_S: f64 = 0.55;
/// Seconds after opening that the window steps back and the bar rises.
const SHRINK_S: f64 = 0.2;
/// Seconds the window takes to come back up to speed after closing.
const RESUME_S: f64 = 0.35;
/// Seconds the stage keeps its backdrop and corners while the window springs back.
const SETTLE_S: f64 = 0.6;

pub(crate) fn moved_color() -> Color { crate::oklch(0.86, 0.15, 85.0) }
fn backdrop() -> Color { crate::oklch(0.11, 0.0, 0.0) }
pub(crate) fn jump_color() -> Color { crate::oklch(0.68, 0.2, 25.0) }

/// The scrubber's state; keep one in the app's state.
#[derive(Default)]
pub struct Scrub {
    rewind: Option<Rewind>,
    clip: Option<Arc<Clip>>,
    frame: usize,
    anchor: Option<usize>,
    picked: Option<u32>,
    note: crate::TextEdit,
    /// When it opened, for the slow-down.
    opened: Option<Instant>,
    /// When it closed, for the speed-up.
    closed: Option<Instant>,
    /// The app's speed the view last showed ([`Scrub::poll`]).
    shown: Option<f32>,
}

/// What a press of a key did ([`Scrub::key`]).
#[derive(Debug)]
#[non_exhaustive]
pub enum Keyed {
    /// Not the scrubber's key.
    Ignored,
    Handled,
    /// Escape: the scrubber closed and recording resumed.
    Closed,
    /// Return: send the selected stretch.
    Send(Sent),
}

/// A stretch of frames to hand on, with the note typed for it.
#[derive(Clone)]
pub struct Sent {
    clip: Arc<Clip>,
    frames: Range<usize>,
    note: String,
}

impl std::fmt::Debug for Sent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sent").field("frames", &self.frames).field("note", &self.note).finish()
    }
}

impl Sent {
    pub fn clip(&self) -> &Arc<Clip> { &self.clip }
    pub fn frames(&self) -> Range<usize> { self.frames.clone() }
    pub fn note(&self) -> &str { &self.note }
    /// [`Clip::write_bundle`] for this stretch and note.
    pub fn write(&self, dir: &std::path::Path) -> std::io::Result<Bundle> {
        self.clip.write_bundle(dir, self.frames.clone(), &self.note)
    }
}

impl Scrub {
    /// Freezes `rewind` and shows its clip at the last frame anything moved in.
    pub fn open(&mut self, rewind: &Rewind) {
        self.show(rewind.freeze());
        self.rewind = Some(rewind.clone());
    }

    /// Shows `clip` without a recorder behind it (a story, a saved clip).
    pub fn show(&mut self, clip: Arc<Clip>) {
        let last = clip.len().saturating_sub(1);
        self.frame = if clip.activity(last) > 0.0 { last } else { clip.next_active(last, -1).unwrap_or(last) };
        self.clip = Some(clip);
        self.anchor = None;
        self.picked = None;
        self.note = crate::TextEdit::default();
        self.opened = Some(Instant::now());
        self.closed = None;
    }

    /// Hides the scrubber. Recording resumes once the window is back in place ([`Scrub::poll`]),
    /// so the step back isn't in it.
    pub fn close(&mut self) {
        if self.clip.take().is_some() { self.closed = Some(Instant::now()); }
        self.opened = None;
    }

    /// How fast the app runs under [`Scrub::stage`]: easing from 1 to 0 as it opens (the
    /// recording already stopped, so the slow-down isn't in it) and back up as it closes.
    pub fn app_speed(&self) -> f32 {
        let since = |t: Option<Instant>| t.map(|t| t.elapsed().as_secs_f64());
        if self.clip.is_some() {
            let p = since(self.opened).map_or(1.0, |t| (t / SLOW_S).min(1.0));
            return ((1.0 - p) * (1.0 - p)) as f32;
        }
        let p = since(self.closed).map_or(1.0, |t| (t / RESUME_S).min(1.0));
        (p * p) as f32
    }

    /// Whether the window has stepped back onto the stage yet.
    fn shrunk(&self) -> bool {
        self.clip.is_some() && self.opened.is_none_or(|t| t.elapsed().as_secs_f64() >= SHRINK_S)
    }

    /// Whether the window is still settling back after closing.
    fn settling(&self) -> bool {
        self.clip.is_none() && self.closed.is_some_and(|t| t.elapsed().as_secs_f64() < SETTLE_S)
    }

    /// Call from the app's poll: while the scrubber opens or closes it asks for a frame each
    /// display refresh, it resumes recording once closed and settled, and it's true whenever
    /// the view should be built again.
    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        if self.clip.is_none() && !self.settling() && let Some(rewind) = self.rewind.take() { rewind.resume(); }
        let now = (self.app_speed(), self.shrunk(), self.settling());
        let moving = !matches!(now, (0.0, true, _) | (1.0, false, false));
        if moving { tx.wake_after(std::time::Duration::ZERO); }
        let key = now.0 + if now.1 { 2.0 } else { 0.0 } + if now.2 { 4.0 } else { 0.0 };
        let changed = self.shown.unwrap_or(1.0) != key;
        self.shown = Some(key);
        changed
    }

    /// Jumps the opening or closing motion to its end: for Reduce Motion, and tests.
    pub fn settle(&mut self) {
        let long_ago = Instant::now().checked_sub(std::time::Duration::from_secs(5));
        if self.opened.is_some() { self.opened = long_ago; }
        if self.closed.is_some() { self.closed = long_ago; }
    }

    pub fn is_open(&self) -> bool { self.clip.is_some() }

    pub fn clip(&self) -> Option<&Arc<Clip>> { self.clip.as_ref() }

    /// The frame showing.
    pub fn frame(&self) -> usize { self.frame }

    /// The selected frames: from the anchor ⇧ set to the playhead, or the playhead alone.
    pub fn selection(&self) -> Range<usize> {
        let a = self.anchor.unwrap_or(self.frame);
        a.min(self.frame)..a.max(self.frame) + 1
    }

    /// Shows frame `k`; with `extend`, selects from where the selection started to it.
    pub fn seek(&mut self, k: usize, extend: bool) {
        let Some(clip) = &self.clip else { return };
        let k = k.min(clip.len().saturating_sub(1));
        if extend { self.anchor.get_or_insert(self.frame); } else { self.anchor = None; }
        self.frame = k;
    }

    /// Shows element `id`'s path over time (None hides it).
    pub fn pick(&mut self, id: Option<u32>) { self.picked = id; }

    /// The note typed so far.
    pub fn note(&self) -> &str { &self.note.text }

    pub fn set_note(&mut self, note: &str) {
        self.note = crate::TextEdit::default();
        self.note.insert(note);
    }

    /// What Send would hand on now.
    pub fn sent(&self) -> Option<Sent> {
        Some(Sent { clip: self.clip.clone()?, frames: self.selection(), note: self.note.text.clone() })
    }

    /// ←/→ step a frame (⇧ selects, ⌥ jumps to the next frame anything moved in), Home/End go
    /// to the ends, Return sends and Escape closes. Route the app's unhandled keys here while
    /// [`Scrub::is_open`].
    pub fn key(&mut self, ev: &KeyEvent) -> Keyed {
        let Some(clip) = self.clip.clone() else { return Keyed::Ignored };
        let extend = ev.mods.shift;
        let step = |dir: isize| if ev.mods.alt { clip.next_active(self.frame, dir) } else { self.frame.checked_add_signed(dir) };
        match ev.key {
            Key::Left => { if let Some(k) = step(-1) { self.seek(k, extend); } }
            Key::Right => { if let Some(k) = step(1) { self.seek(k, extend); } }
            Key::Home => self.seek(0, extend),
            Key::End => self.seek(clip.len(), extend),
            Key::Escape => { self.close(); return Keyed::Closed; }
            Key::Enter => return self.sent().map_or(Keyed::Handled, Keyed::Send),
            _ => return Keyed::Ignored,
        }
        Keyed::Handled
    }

    /// The app's view on the scrubber's stage. Wrap the app this way whether or not the
    /// scrubber shows, so nothing in it changes path when it opens. Open, the app slows to a
    /// stop ([`Scrub::app_speed`]; its own clock, not the window's, so the scrubber still
    /// animates), steps back onto a dark stage with the elements that moved outlined over it,
    /// and the track rises beneath. `lens` finds this state in the app's; Send (the button, or
    /// Return in the note) calls `on_send`. Call [`Scrub::poll`] from the app's poll.
    pub fn stage<S: 'static>(&self, app: El, lens: fn(&mut S) -> &mut Scrub, on_send: fn(&mut S, Sent, &mut Tx)) -> El {
        let root = el().key("rewind-root").w_full().h_full();
        let mut stage = el().key("rewind-stage").w_full().h_full().center()
            .child(app.time_scale(self.app_speed()));
        let Some(clip) = &self.clip else {
            if !self.settling() { return root.child(stage) }
            return root.fill(backdrop()).child(stage.clip_rounded(12.0).shadow(18.0, 48.0, Color::BLACK.alpha(0.55)));
        };
        let (vw, vh) = clip.viewport();
        let [_, _, w, h] = crate::memo::layout_with("rewind-root", |r| r.map(|v| (v * 2.0).round() / 2.0)).unwrap_or([0.0, 0.0, vw, vh]);
        if self.shrunk() {
            let (k, oy) = fit(w, h);
            stage = stage.scale(k).origin(0.5, oy).clip_rounded(12.0 / k)
                .shadow(18.0, 48.0, Color::BLACK.alpha(0.55))
                .child(self.marks(clip, lens).transition(crate::Transition::opacity()).enter_delay((SLOW_S - SHRINK_S) as f32 * 0.6));
        }
        let mut root = root.fill(backdrop()).child(stage);
        if self.shrunk() {
            root = root.child(self.bar(clip, w, &clip.changes(self.frame), lens, on_send)
                .transition(crate::Transition::offset(0.0, BAR_H + GAP).combined(crate::Transition::opacity())));
        }
        root
    }

    /// The outlines over the frozen window: what moved this frame, where everything else was.
    fn marks<S: 'static>(&self, clip: &Clip, lens: fn(&mut S) -> &mut Scrub) -> El {
        let (vw, vh) = clip.viewport();
        let [ox, oy, _, _] = crate::memo::layout_with("rewind", |r| r.map(|v| (v * 2.0).round() / 2.0)).unwrap_or([0.0, 0.0, vw, vh]);
        let changes = clip.changes(self.frame);
        // The live view under it is frozen at the moment it opened; dimmed, the recorded
        // frame reads over it.
        let layer = el().key("rewind").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .child(el().key("rewind-dim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(Color::BLACK.alpha(0.4))
                .on_press(move |s: &mut S| lens(s).picked = None));
        // Recorded frames: they land where they were, never springing between steps.
        let mut marks = el().key("rewind-marks").absolute(-ox, -oy).size(vw, vh).motion(crate::Spring::NONE);
        for (id, shot) in clip.frame(self.frame) {
            if changes.iter().any(|c| c.id == id) { continue; }
            let r = shot.rect;
            marks = marks.child(el().key(format!("rewind-el-{id}")).absolute(r[0], r[1]).size(r[2].max(1.0), r[3].max(1.0))
                .border(0.5, Color::WHITE.alpha(0.08 + 0.14 * shot.opacity.clamp(0.0, 1.0))));
        }
        if let Some(id) = self.picked { marks = marks.children(self.path_view(clip, id)); }
        for c in changes.iter().rev() { marks = marks.children(outline(c, lens)); }
        if let Some(id) = self.picked.filter(|id| !changes.iter().any(|c| c.id == *id))
            && let Some((_, s)) = clip.frame(self.frame).into_iter().find(|(i, _)| *i == id)
        {
            marks = marks.child(el().key("rewind-picked").absolute(s.rect[0], s.rect[1]).size(s.rect[2].max(1.0), s.rect[3].max(1.0)).border(1.0, Color::WHITE.alpha(0.7)));
        }
        layer.child(marks)
    }

    fn path_view(&self, clip: &Clip, id: u32) -> Vec<El> {
        let sel = self.selection();
        let range = if sel.len() > 1 { sel } else { self.frame.saturating_sub(PATH_SPAN)..(self.frame + PATH_SPAN + 1).min(clip.len()) };
        let path = clip.path(id, range);
        let mut last: Option<Shot> = None;
        path.into_iter().filter_map(|(k, s)| {
            let moved = last.is_none_or(|l| l != s);
            last = Some(s);
            if !moved && k != self.frame { return None; }
            let (cx, cy) = (s.rect[0] + s.rect[2] / 2.0, s.rect[1] + s.rect[3] / 2.0);
            let r = if k == self.frame { 3.5 } else { 1.5 };
            let c = if clip.has_jump(k) { jump_color() } else { Color::WHITE.alpha(if k == self.frame { 1.0 } else { 0.55 }) };
            Some(el().key(format!("rewind-path-{k}")).absolute(cx - r, cy - r).size(r * 2.0, r * 2.0).radius(r).fill(c))
        }).collect()
    }

    fn bar<S: 'static>(&self, clip: &Clip, w: f32, changes: &[Change], lens: fn(&mut S) -> &mut Scrub, on_send: fn(&mut S, Sent, &mut Tx)) -> El {
        let t = tok::theme(true);
        let sel = self.selection();
        let moved = changes.iter().filter(|c| c.before.is_some() && c.after.is_some()).count();
        let mut facts = vec![format!("{:+.2} s", clip.time(self.frame)), format!("frame {} of {}", self.frame + 1, clip.len())];
        if moved > 0 { facts.push(format!("{moved} moved")); }
        if sel.len() > 1 { facts.push(format!("{} frames selected", sel.len())); }
        let track_w = (w - GAP * 2.0 - PAD * 2.0).max(1.0);
        let send = row().key("rewind-send").h(24.0).px(10.0).center_y().radius(tok::radius::SM).fill(t.card)
            .border(0.5, t.card_border).hover(t.hover).press(t.press)
            .role("button").label("Send these frames")
            .child(text("Send").font(type_::LABEL).color(t.text))
            .on_press_tx(move |s: &mut S, tx: &mut Tx| if let Some(sent) = lens(s).sent() { on_send(s, sent, tx); });
        let note = text_input(&self.note, type_::BODY).key("rewind-note").single_line().grow(1.0)
            .placeholder("What's wrong here?", t.text_secondary)
            .on_edit(move |s: &mut S| &mut lens(s).note)
            .on_submit(move |s: &mut S, tx: &mut Tx| if let Some(sent) = lens(s).sent() { on_send(s, sent, tx); });
        let mut controls = row().w_full().h(24.0).gap(12.0).center_y()
            .child(text(facts.join("  ·  ")).font(type_::MONO_CAPTION.mono()).color(t.text_secondary))
            .child(note);
        if w >= HINT_MIN_W {
            controls = controls.child(text("←→ step  ⇧ select  ⌥ next move  esc resume").font(type_::LABEL).color(t.text_secondary));
        }
        col().key("rewind-bar").pin(Some(GAP), None, Some(GAP), Some(GAP)).h(BAR_H).px(PAD).pt(8.0).gap(8.0)
            .radius(12.0).fill(t.bg.alpha(0.96)).border(0.5, t.separator)
            .child(super::track::view(clip, self.frame, sel, track_w, lens))
            .child(controls.child(send))
            .child(spacer())
    }
}

/// How far the window steps back so the bar fits under it, and where it scales from (a fraction
/// of its height) so it keeps a margin above.
fn fit(w: f32, h: f32) -> (f32, f32) {
    let k = ((h - BAR_H - GAP * 3.0) / h).min((w - GAP * 2.0) / w).clamp(0.5, 1.0);
    let oy = if k < 1.0 { (GAP / (h * (1.0 - k))).clamp(0.0, 1.0) } else { 0.0 };
    (k, oy)
}

/// An element that changed this frame: its old place dim, its new place outlined, and a label.
fn outline<S: 'static>(c: &Change, lens: fn(&mut S) -> &mut Scrub) -> Vec<El> {
    let Some(now) = c.after.or(c.before) else { return Vec::new() };
    let color = if c.jump { jump_color() } else { moved_color() };
    let id = c.id;
    let mut out = Vec::new();
    if let (Some(b), Some(_)) = (c.before, c.after) {
        out.push(el().key(format!("rewind-was-{id}")).absolute(b.rect[0], b.rect[1]).size(b.rect[2].max(1.0), b.rect[3].max(1.0)).border(1.0, color.alpha(0.35)));
    }
    let r = now.rect;
    out.push(el().key(format!("rewind-box-{id}")).absolute(r[0], r[1]).size(r[2].max(1.0), r[3].max(1.0))
        .border(if c.after.is_some() { 1.5 } else { 1.0 }, color.alpha(if c.after.is_some() { 1.0 } else { 0.5 }))
        .role("button").label(format!("Show {}'s path", c.name))
        .on_press(move |s: &mut S| { let scrub = lens(s); scrub.picked = if scrub.picked == Some(id) { None } else { Some(id) }; }));
    let above = r[1] >= 20.0;
    out.push(row().key(format!("rewind-label-{id}")).absolute(r[0], if above { r[1] - 19.0 } else { r[1] + r[3] + 2.0 }).h(17.0).px(5.0).center_y()
        .radius(3.0).fill(color).child(text(c.label()).font(type_::LABEL).color(Color::BLACK)));
    out
}
