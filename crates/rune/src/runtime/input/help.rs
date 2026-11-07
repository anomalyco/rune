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
use super::*;

#[derive(Default)]
pub(super) struct HelpState {
    /// This frame's help elements: (node, visible rect in points, `hits.len()` before it, the
    /// link of its text when the help is that link's).
    hits: Vec<(NodeId, [f32; 4], usize, Option<usize>)>,
    /// The help element (and link) under the pointer and when (clock seconds) the pointer
    /// reached it.
    target: Option<(String, Option<usize>, f64)>,
    /// A press, scroll or key hid its tooltip until the pointer leaves it.
    dismissed: bool,
    /// The deadline already asked for, so each rest schedules one wake.
    scheduled: Option<f64>,
    /// The tooltip showing this frame: its text and box in points.
    tip: Option<(String, [f32; 4])>,
    /// The host shows system tooltips from `help_regions`.
    native: bool,
    regions: Vec<([f32; 4], String)>,
    /// When (clock seconds) a shown tooltip last hid because the pointer left its element,
    /// for the look's skip delay.
    hid_at: Option<f64>,
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

impl<S: 'static> Runtime<S> {
    /// The host shows AppKit's tooltips: on macOS, while the view keeps the system look.
    fn native_help(&self) -> bool { self.help.native && self.defaults.help.is_none() }

    /// The look tooltips draw in now: the view's, or the system's for this window.
    fn help_look(&self) -> super::HelpLook {
        self.defaults.help.unwrap_or_else(|| super::HelpLook::system(self.backdrop_dark().unwrap_or(self.env.system_dark)))
    }

    /// The host shows the system's own tooltips for [`Runtime::help_regions`], so Rune draws none.
    #[cfg(target_os = "macos")]
    pub(crate) fn set_native_help(&mut self, on: bool) { self.help.native = on; }

    /// Each visible help element's rect (window points, x0 y0 x1 y1) and text, in paint order,
    /// for a host that shows system tooltips.
    pub fn help_regions(&self) -> &[([f32; 4], String)] { &self.help.regions }

    /// The tooltip Rune shows now: its text and box (window points, x y w h).
    pub fn help_tooltip(&self) -> Option<(&str, [f32; 4])> {
        self.help.tip.as_ref().map(|(t, r)| (t.as_str(), *r))
    }

    /// A tooltip waits for a frame at or after its delay to draw.
    pub(super) fn tooltip_waiting(&self) -> bool {
        self.help.scheduled.is_some_and(|due| self.clock + 1e-6 < due)
    }

    /// During `build`: node `i` has help and draws visible over `rect` (points, x0 y0 x1 y1),
    /// after the first `seq` hits.
    pub(super) fn push_help_hit(&mut self, i: usize, rect: [f32; 4], seq: usize) {
        if rect[2] > rect[0] && rect[3] > rect[1] {
            self.help.hits.push((self.nodes[i].id, rect, seq, None));
        }
    }

    /// During `build`: the line boxes of node `i`'s links that have help, for border box
    /// `rect` and clipped to `clip` (points, x0 y0 x1 y1), after the first `seq` hits.
    pub(super) fn push_link_help_hits(&mut self, i: usize, rect: [f32; 4], clip: [f32; 4], seq: usize) {
        let Some(t) = self.nodes[i].text.as_ref() else { return };
        if !t.links.iter().any(|l| l.help.is_some()) { return; }
        let id = self.nodes[i].id;
        for (k, r) in self.link_boxes(i, rect) {
            if self.nodes[i].text.as_ref().and_then(|t| t.links.get(k)).is_none_or(|l| l.help.is_none()) { continue; }
            let c = [r[0].max(clip[0]), r[1].max(clip[1]), r[2].min(clip[2]), r[3].min(clip[3])];
            if c[2] > c[0] && c[3] > c[1] { self.help.hits.push((id, c, seq, Some(k))); }
        }
    }

    /// The help text of node `i`, or of its link `link`.
    fn help_text(&self, i: usize, link: Option<usize>) -> Option<&String> {
        match link {
            Some(k) => self.nodes[i].text.as_ref()?.links.get(k)?.help.as_ref(),
            None => self.nodes[i].help.as_ref(),
        }
    }

    pub(super) fn clear_help_hits(&mut self) { self.help.hits.clear(); }

    /// A press, scroll or key: hide the tooltip until the pointer leaves its element.
    pub(super) fn dismiss_help(&mut self) {
        if self.help.target.is_some() {
            self.help.dismissed = true;
            self.help.tip = None;
        }
    }

    /// The help element under (x, y): the topmost one, unless something interactive drawn over
    /// it (not inside it) is there too.
    fn help_at(&self, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
        let &(id, _, seq, link) = self.help.hits.iter().rev().find(|&&(id, r, _, _)| self.hit_contains(id, r, x, y))?;
        let h = *self.node_index.get(&id)?;
        if self.subtree_exiting(h) || !self.allowed(h) { return None; }
        let over = self.hits.iter().enumerate().rev().find(|&(_, &(id, r))| self.hit_contains(id, r, x, y));
        if let Some((k, (top, _))) = over
            && k > seq
            && let Some(&t) = self.node_index.get(top)
            && !std::iter::successors(Some(t), |&p| self.nodes[p].parent).any(|p| p == h)
        {
            return None;
        }
        Some((h, link))
    }

    /// Whether moving the pointer to (x, y) changes the help element under it, so a host that
    /// skips frames for unchanged pixels still starts or ends a tooltip's wait.
    pub(super) fn help_target_changes(&self, x: f32, y: f32) -> bool {
        if self.native_help() || self.help.hits.is_empty() && self.help.target.is_none() { return false; }
        let at = self.help_at(x, y).map(|(i, k)| (self.nodes[i].path.as_str(), k));
        at != self.help.target.as_ref().map(|(p, k, _)| (p.as_str(), *k))
    }

    /// The parts of help element `h`'s rect `r` that nothing interactive drawn over it (not
    /// inside it), from hit `seq` on, covers.
    fn unoccluded(&self, h: usize, r: [f32; 4], seq: usize) -> Vec<[f32; 4]> {
        let mut parts = vec![self.hit_screen_rect(self.nodes[h].id, r)];
        for &(id, o) in &self.hits[seq.min(self.hits.len())..] {
            let o = self.hit_screen_rect(id, o);
            if !parts.iter().any(|p| overlaps(*p, o)) { continue; }
            let Some(&t) = self.node_index.get(&id) else { continue };
            if std::iter::successors(Some(t), |&p| self.nodes[p].parent).any(|p| p == h) { continue; }
            parts = parts.into_iter().flat_map(|p| subtract(p, o)).collect();
        }
        parts
    }

    /// After `build`'s hits: track the rest, show or hide the tooltip and draw it.
    pub(super) fn update_help(&mut self) {
        self.help.regions.clear();
        if self.native_help() {
            // Nothing behind a modal, and only where nothing else is drawn over the element.
            let mut regions = Vec::new();
            for &(id, r, seq, link) in &self.help.hits {
                let Some(&i) = self.node_index.get(&id) else { continue };
                let Some(text) = self.help_text(i, link) else { continue };
                if self.subtree_exiting(i) || !self.allowed(i) { continue; }
                let words = crate::el::help_words(text);
                regions.extend(self.unoccluded(i, r, seq).into_iter().map(|p| (p, words.clone().into_owned())));
            }
            self.help.regions = regions;
            self.help.tip = None;
            return;
        }
        let pointer = self.pointer.filter(|&(x, y)| self.in_viewport(x, y) && self.capture.is_none() && self.secondary_capture.is_none());
        // While moving layout holds hover, the tooltip keeps its element, and nothing sliding
        // under the pointer starts a wait.
        let under = match self.held_hover() {
            Some(_) => self.help.target.as_ref().and_then(|(p, k, _)| Some((self.hover_live(p)?, *k))).filter(|_| pointer.is_some()),
            None => pointer.and_then(|(x, y)| self.help_at(x, y)),
        };
        let at = under.map(|(i, k)| (self.nodes[i].path.as_str(), k));
        let look = self.help_look();
        let delay = look.delay.unwrap_or(self.help_delay);
        if at != self.help.target.as_ref().map(|(p, k, _)| (p.as_str(), *k)) {
            if self.help.tip.is_some() {
                self.help.hid_at = Some(self.clock);
            }
            // Just after a tooltip hid, the next element's shows at once.
            let warm = look.skip > 0.0 && self.help.hid_at.is_some_and(|t| self.clock - t <= look.skip);
            let since = if warm { self.clock - delay } else { self.clock };
            self.help.target = under.map(|(i, k)| (self.nodes[i].path.clone(), k, since));
            self.help.dismissed = false;
            self.help.scheduled = None;
        }
        self.help.tip = None;
        let (Some((i, link)), Some((_, _, since))) = (under, self.help.target.clone()) else { return };
        if self.help.dismissed { return; }
        let due = since + delay;
        if self.clock + 1e-6 < due {
            if self.help.scheduled != Some(due) {
                self.help.scheduled = Some(due);
                self.schedule_wakes([std::time::Duration::from_secs_f64(due - self.clock)]);
            }
            return;
        }
        let Some(text) = self.help_text(i, link).cloned() else { return };
        let e = self.drawn_box(i);
        let anchor = link.and_then(|k| self.link_boxes(i, e).into_iter().filter(|(j, _)| *j == k).map(|(_, r)| r)
            .reduce(|u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
        let above = link.is_none() && self.nodes[i].cold.get().is_some_and(|c| c.help_above);
        self.draw_tooltip(anchor.unwrap_or(e), text, above);
    }

    /// Draws `text` (and its keycaps) in a tooltip in the current look under box `e` (window
    /// points, x0 y0 x1 y1; above it when there's no room below), or `above` it (under it when
    /// there's no room above), kept inside the window.
    fn draw_tooltip(&mut self, e: [f32; 4], text: String, above: bool) {
        let s = self.env.scale;
        let look = self.help_look();
        let key = look.key;
        let mut parts = text.split(crate::el::HELP_KEY);
        let words = parts.next().unwrap_or_default();
        let keys: Vec<String> = parts.map(|k| if key.upper { k.to_uppercase() } else { k.to_owned() }).collect();
        let caps: Vec<_> = keys.iter().map(|k| {
            let l = self.text.layout_rich(k, &[], key.font, s, None);
            let w = ((l.full_width() / s).ceil() + 2.0 * key.pad_x).max(key.height);
            (l, w)
        }).collect();
        let keys_w = if caps.is_empty() { 0.0 } else {
            caps.iter().map(|(_, w)| w).sum::<f32>() + key.gap * (caps.len() - 1) as f32 + if words.is_empty() { 0.0 } else { key.lead }
        };
        let [pad_x, pad_y] = look.padding;
        let (vw, vh) = self.viewport;
        let margin = look.margin;
        let max_w = look.max_w.unwrap_or(vw - 2.0 * margin - 2.0 * pad_x).max(1.0) - keys_w;
        let layout = self.text.layout_rich(words, &[], look.font, s, Some(max_w.max(1.0) * s));
        let (tw, th) = if words.is_empty() { (0.0, 0.0) } else { ((layout.full_width() / s).ceil(), (layout.height() / s).ceil()) };
        let ch = if caps.is_empty() { th } else { th.max(key.height) };
        let (w, h) = (tw + keys_w + 2.0 * pad_x, ch + 2.0 * pad_y);
        let [below, over] = look.gap;
        let mut y = e[3] + below;
        if above && e[1] - over - h >= margin {
            y = e[1] - over - h;
        } else if y + h > vh - margin && e[1] - below - h >= margin {
            y = e[1] - below - h;
        }
        let x = ((e[0] + e[2] - w) * 0.5).clamp(margin, (vw - w - margin).max(margin));
        let y = y.clamp(margin, (vh - h - margin).max(margin));
        let r = [x * s, y * s, w * s, h * s].map(|v| v.round());
        let srgb = self.srgb_blend;
        let radii = [look.radius * s; 4];
        let cast = super::shadow_paint::Caster {
            rect: r, radii, shape: radii, k_px: s, scale: s, opacity: 1.0, srgb, clip: NO_CLIP, fade: [0.0; 4], hole: None,
            opaque: look.fill.a >= 1.0,
        };
        for sh in look.shadows.iter().flatten().filter(|sh| sh.color.a > 0.0) {
            super::shadow_paint::push_shadow(&mut self.instances, sh, sh.color, &cast);
        }
        let (ring_w, ring) = look.ring.map_or((0.0, Color::TRANSPARENT), |(rw, c)| ((rw * s).max(1.0), c));
        self.instances.push(Instance {
            rect: r,
            radii,
            fill: premul(look.fill, 1.0, srgb),
            border: premul(ring, 1.0, srgb),
            params: [ring_w, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: crate::render::pack_fade([0.0; 4], NO_CLIP, NO_CLIP),
        });
        let top = r[1] + pad_y * s;
        let mut at = r[0] + pad_x * s;
        if !words.is_empty() {
            self.draw_tip_text(&layout, [at, top + ((ch - th) * 0.5 * s).round()], look.ink);
            at += (tw + key.lead) * s;
        }
        for (l, cw) in &caps {
            let cap = [at, top + ((ch - key.height) * 0.5 * s).round(), cw * s, key.height * s].map(|v| v.round());
            self.instances.push(Instance {
                rect: cap,
                radii: [key.radius * s; 4],
                fill: premul(key.fill, 1.0, srgb),
                border: [0.0; 4],
                params: [0.0, KIND_RECT, 0.0, 1.0],
                uv: [0.0; 4],
                clip: NO_CLIP,
                fade: crate::render::pack_fade([0.0; 4], NO_CLIP, NO_CLIP),
            });
            let lw = l.full_width();
            let origin = [cap[0] + ((cap[2] - lw) * 0.5).round(), cap[1] + ((cap[3] - l.height()) * 0.5).round()];
            self.draw_tip_text(l, origin, key.ink);
            at += (cw + key.gap) * s;
        }
        self.help.tip = Some((crate::el::help_words(&text).into_owned(), [x, y, w, h]));
    }

    /// Draws a tooltip's shaped text with its top left at `origin` (device pixels).
    fn draw_tip_text(&mut self, layout: &parley::Layout<crate::text::Ink>, origin: [f32; 2], ink: Color) {
        let srgb = self.srgb_blend;
        self.glyph_scratch.clear();
        self.text.set_glyph_inks(ink, &[]);
        self.text.glyphs_for_layout_scaled(layout, origin, 1.0, false, &mut self.glyph_scratch);
        for q in &self.glyph_scratch {
            self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, premul(ink, 1.0, srgb), NO_CLIP, [0.0; 4]));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Cx, El, Env, el, row};
    use super::{Runtime, subtract};

    #[derive(Default)]
    struct S { modal: bool, cover: bool }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = el().w_full().h_full()
            .child(el().key("copy").size(40.0, 20.0).pin(Some(20.0), Some(20.0), None, None).help("Copy"))
            .child(el().key("low").size(40.0, 20.0).pin(Some(20.0), Some(200.0), None, None).help("Low"));
        if s.cover {
            // A popover drawn over the right half of "copy".
            root = root.child(el().key("pop").size(100.0, 60.0).pin(Some(40.0), Some(10.0), None, None).on_press(|_: &mut S| {}));
        }
        if s.modal {
            root = root.child(row().key("sheet").size(80.0, 40.0).pin(Some(200.0), Some(200.0), None, None)
                .modal(false, |_: &mut S| {})
                .child(el().key("inside").size(30.0, 20.0).help("Inside")));
        }
        root
    }

    fn runtime(s: S, native: bool) -> Runtime<S> {
        let mut rt = Runtime::new(s, view, (300.0, 300.0), Env::new(true, 2.0));
        rt.help.native = native;
        rt.frame(0.0);
        rt
    }

    fn texts(rt: &Runtime<S>) -> Vec<&str> { rt.help_regions().iter().map(|(_, t)| t.as_str()).collect() }

    #[test]
    fn native_regions_skip_what_a_modal_or_popover_covers() {
        let rt = runtime(S::default(), true);
        assert_eq!(texts(&rt), ["Copy", "Low"]);
        let rt = runtime(S { modal: true, ..S::default() }, true);
        assert_eq!(texts(&rt), ["Inside"], "behind a modal, nothing registers");
        let rt = runtime(S { cover: true, ..S::default() }, true);
        let copy: Vec<[f32; 4]> = rt.help_regions().iter().filter(|(_, t)| t == "Copy").map(|(r, _)| *r).collect();
        assert_eq!(copy, [[20.0, 20.0, 40.0, 40.0]], "only the uncovered half of copy");
    }

    #[test]
    fn subtract_leaves_what_is_outside() {
        assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [20.0, 0.0, 30.0, 10.0]), [[0.0, 0.0, 10.0, 10.0]]);
        assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [-1.0, -1.0, 11.0, 11.0]), Vec::<[f32; 4]>::new());
        assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [4.0, 4.0, 6.0, 6.0]).len(), 4);
    }

    #[test]
    fn reaching_help_wakes_a_host_that_skips_unchanged_frames() {
        // An overlay draws only when observe_pointer says something changed; help that has no
        // hover paint of its own must still start its wait.
        let mut rt = runtime(S::default(), false);
        assert!(!rt.observe_pointer(150.0, 150.0), "nothing there");
        assert!(rt.observe_pointer(30.0, 30.0), "onto help");
        rt.frame(1.0 / 60.0);
        assert!(!rt.observe_pointer(31.0, 30.0), "still on it");
        rt.frame(rt.help_delay);
        assert!(rt.help_tooltip().is_some());
        assert!(rt.observe_pointer(150.0, 150.0), "off it hides the tooltip");
    }

    fn linked(k: &f32, _: &mut Cx<f32>) -> El {
        let label = crate::text("MMMM MMMM")
            .font(crate::TextStyle::new(16.0, 400.0))
            .link(crate::Link::new(0..4, "a"))
            .link(crate::Link::new(5..9, "b").help("Goes to b"))
            .on_link(|_: &mut f32, _, _| {});
        el().w_full().h_full().child(el().pin(Some(10.0), Some(10.0), None, None).scale(*k).child(label))
    }

    /// Rests on (x, y) past the delay and returns the tooltip shown.
    fn rest(rt: &mut Runtime<f32>, x: f32, y: f32) -> Option<(String, [f32; 4])> {
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        rt.help_tooltip().map(|(t, r)| (t.to_owned(), r))
    }

    #[test]
    fn a_link_shows_its_own_help_under_its_glyphs() {
        for k in [1.0, 1.6] {
            let mut rt = Runtime::new(k, linked, (400.0, 300.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let i = rt.nodes.iter().position(|n| n.text.is_some()).unwrap();
            let boxes = rt.link_boxes(i, rt.drawn_box(i));
            let b = boxes.iter().find(|(j, _)| *j == 1).unwrap().1;
            let a = boxes.iter().find(|(j, _)| *j == 0).unwrap().1;
            assert!(rest(&mut rt, (a[0] + a[2]) * 0.5, (a[1] + a[3]) * 0.5).is_none(), "zoom {k}: a link without help shows none");
            let (text, r) = rest(&mut rt, (b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5).expect("tip on the link");
            assert_eq!(text, "Goes to b");
            assert!(r[1] >= b[3] && r[0] < b[2] && r[0] + r[2] > b[0], "zoom {k}: tip {r:?} sits under the link {b:?}");
            assert!(rest(&mut rt, 300.0, 250.0).is_none(), "leaving hides it");
        }
    }

    #[test]
    fn native_regions_carry_a_links_help() {
        let mut rt = Runtime::new(1.0, linked, (400.0, 300.0), Env::new(true, 2.0));
        rt.help.native = true;
        rt.frame(0.0);
        let texts: Vec<&str> = rt.help_regions().iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, ["Goes to b"]);
    }
}
