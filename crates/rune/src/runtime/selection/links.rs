//! Pressable spans in text (`El::link`). Each link's line boxes come from the shaped layout,
//! so only its glyphs take the pointer. The text element holds focus for its links and
//! `LinkState::focus` says which one; links are reported to tools as their own elements.
use super::*;
use crate::text::LinkPaint;

/// The focused text element's focused link. A press's link is the press's
/// (`capture::Held::link`, `capture::Secondary::Held`).
#[derive(Default)]
pub(in crate::runtime) struct LinkState {
    focus: Option<usize>,
}

/// Where node `n`'s text starts inside its border box `rect` (points, x0 y0 x1 y1), its text
/// drawn `k` times its size.
pub(crate) fn origin(n: &Node, spec: &TextSpec, flow: TextFlow<'_>, rect: [f32; 4], s: f32, k: f32) -> [f32; 2] {
    let x = text_x(spec, rect[0] + n.inset[0] * k, rect[2] - n.inset[2] * k, text_width(flow.full_width() * k) / s, flow.flow());
    [x, rect[1] + n.inset[1] * k]
}

/// Link bytes `a..b` of node `n`'s text as bytes of its drawn layout, which truncation may
/// have cut (empty when the cut hides the link).
fn drawn(n: &Node, a: u32, b: u32) -> (u32, u32) {
    let (a, b) = n.elided.map_or((a as usize, b as usize), |e| e.mark(a as usize, b as usize, true));
    (a as u32, b as u32)
}

/// The byte length of node `n`'s drawn text.
fn layout_len(n: &Node, spec: &TextSpec) -> usize {
    n.elided.map_or(spec.content.len(), |e| e.drawn_len(spec.content.len()))
}

/// Line boxes of bytes `start..end` in layout device px, with each box's line index.
fn boxes(layout: &Layout<Ink>, len: usize, start: u32, end: u32) -> Vec<(parley::BoundingBox, usize)> {
    let (a, b) = ((start as usize).min(len), (end as usize).min(len));
    crate::text::range_boxes(layout, a, b, false)
}

/// The link paints to draw this frame, by byte range, with the resting span's `bg_pad`.
pub(in crate::runtime) struct LinkFx(Vec<(u32, u32, LinkPaint, f32, bool)>);

impl LinkFx {
    pub(in crate::runtime) fn is_empty(&self) -> bool { self.0.is_empty() }

    pub(in crate::runtime) fn glyph_color(&self, byte: u32) -> Option<Color> {
        self.0.iter().find(|(a, b, p, _, _)| byte >= *a && byte < *b && p.color.is_some()).and_then(|(_, _, p, _, _)| p.color)
    }

    pub(in crate::runtime) fn decos_piece(&self, piece: &crate::text::Piece<'_>, len: usize, s: f32, origin: [f32; 2], out: &mut Vec<PaintedDeco>) {
        if self.is_empty() { return; }
        if piece.is_whole() { return self.decos(piece.layout, len, s, origin, out); }
        let clipped: Vec<_> = self.0.iter().filter_map(|&(a, b, p, pad, rings)| {
            let (sa, sb) = piece.clip((a as usize).min(len), (b as usize).min(len))?;
            Some((sa as u32, sb as u32, p, pad, rings))
        }).collect();
        if !clipped.is_empty() { LinkFx(clipped).decos(piece.layout, piece.shaped(piece.end), s, origin, out); }
    }

    /// Backgrounds and underlines (device px, layout top-left at `origin`).
    pub(in crate::runtime) fn decos(&self, layout: &Layout<Ink>, len: usize, s: f32, origin: [f32; 2], out: &mut Vec<PaintedDeco>) {
        for &(a, b, p, bg_pad, has_rings) in &self.0 {
            for (bb, line) in boxes(layout, len, a, b) {
                let (x0, w) = (origin[0] + bb.x0 as f32, (bb.x1 - bb.x0) as f32);
                if p.bg.is_some() || p.ring.is_some() {
                    let pad = bg_pad * s;
                    let (mut y0, mut y1) = ((origin[1] + bb.y0 as f32).round(), (origin[1] + bb.y1 as f32).round());
                    if let Some(l) = layout.get(line) {
                        let base = origin[1] + l.metrics().baseline;
                        let (mut asc, mut desc) = (0.0f32, 0.0f32);
                        for run in l.runs() {
                            let r = run.text_range();
                            if r.start < b as usize && (a as usize) < r.end {
                                let fm = run.font_metrics();
                                (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                            }
                        }
                        if asc > 0.0 || desc > 0.0 {
                            y0 = (base - asc - 1.0 * s).round();
                            y1 = (base + desc + 1.0 * s).round();
                        }
                    }
                    let rect = [x0 - pad, y0, w + 2.0 * pad, y1 - y0];
                    let ring = p.ring.map(|(c, rw)| (c, rw * s));
                    // A ring can split an otherwise uniform link background. Recolor those
                    // pieces in place, but don't mistake an isolated inline-code background
                    // for the whole link's hover background.
                    let contained = |d: &&PaintedDeco| {
                        d.radius > 0.0 && d.rect[0] >= rect[0] - 1.5
                            && d.rect[0] + d.rect[2] <= rect[0] + rect[2] + 1.5
                            && d.rect[1] < y1 && y0 < d.rect[1] + d.rect[3]
                    };
                    let split = has_rings && out.iter().filter(contained).any(|d| d.ring.is_some())
                        && out.iter().filter(contained).any(|d| (d.rect[0] - rect[0]).abs() < 1.5)
                        && out.iter().filter(contained).any(|d| (d.rect[0] + d.rect[2] - rect[0] - rect[2]).abs() < 1.5);
                    let mut matched = false;
                    for d in out.iter_mut().filter(|d| {
                        d.radius > 0.0
                            && d.rect[0] >= rect[0] - 1.5
                            && d.rect[0] + d.rect[2] <= rect[0] + rect[2] + 1.5
                            && (split || ((d.rect[0] - rect[0]).abs() < 1.5 && (d.rect[2] - rect[2]).abs() < 1.5))
                            && d.rect[1] < y1
                            && y0 < d.rect[1] + d.rect[3]
                    }) {
                        matched = true;
                        if let Some(c) = p.bg {
                            d.color = c;
                        }
                        if ring.is_some() {
                            d.ring = ring;
                        }
                    }
                    if !matched {
                        out.push(PaintedDeco { deco: Deco { rect, color: p.bg.unwrap_or(Color::TRANSPARENT), radius: 4.0 * s }, ring, byte: b.saturating_sub(1) });
                    }
                }
                if let (Some(c), Some(l)) = (p.underline, layout.get(line)) {
                    let y = (origin[1] + l.metrics().baseline + 2.0 * s).round();
                    let (x0, x1) = crate::text::underline_edges(&l, origin[0], bb.x0 as f32, bb.x1 as f32);
                    let w = x1 - x0;
                    if let Some(d) = out.iter_mut().find(|d| {
                        d.radius == 0.0
                            && (d.rect[0] - x0).abs() < 1.5
                            && (d.rect[2] - w).abs() < 1.5
                            && (d.rect[1] - y).abs() < 1.5
                    }) {
                        d.color = c;
                    } else {
                        out.push(PaintedDeco { deco: Deco { rect: [x0, y, w, s.max(1.0)], color: c, radius: 0.0 }, ring: None, byte: 0 });
                    }
                }
            }
        }
    }
}

pub(in crate::runtime) fn link_count(n: &Node) -> usize {
    n.text.as_ref().map_or(0, |t| t.cold.links.len())
}

impl caret::TextInputState {
    /// Tab inside the focused text's links: moves to its next (or previous) link and returns
    /// true, or false at its last (first) link so Tab leaves it.
    #[inline]
    pub(in crate::runtime) fn link_tab(&mut self, n: &Node, backward: bool) -> bool {
        let count = link_count(n);
        let Some(k) = self.links.focus.filter(|_| count > 0) else { return false };
        let next = if backward { k.checked_sub(1) } else { Some(k + 1).filter(|&j| j < count) };
        if next.is_some() { self.links.focus = next; }
        next.is_some()
    }

    /// Tab focused node `n`: start at its first link, or its last going backward.
    #[inline]
    pub(in crate::runtime) fn enter_links(&mut self, n: &Node, backward: bool) {
        let count = link_count(n);
        self.links.focus = (count > 0).then(|| if backward { count - 1 } else { 0 });
    }

    #[inline]
    pub(in crate::runtime) fn blur_links(&mut self) {
        self.links.focus = None;
    }

    /// The focused text's focused link, if any (it may not be in range for that text).
    #[inline]
    pub(in crate::runtime) fn link_focus(&self) -> Option<usize> { self.links.focus }

    /// Focuses link `k` of the text about to take focus, as Tab into it does.
    #[inline]
    pub(in crate::runtime) fn set_link_focus(&mut self, k: usize) { self.links.focus = Some(k); }

    /// Whether focused node `n` has a link focused, which takes Return.
    #[inline]
    pub(in crate::runtime) fn link_focused(&self, n: &Node) -> bool {
        self.links.focus.is_some_and(|k| k < link_count(n))
    }

    /// The id of focused node `n`'s focused link, which tools list as its own element.
    pub(in crate::runtime) fn focused_link_id(&self, n: &Node) -> Option<String> {
        let k = self.links.focus?;
        n.text.as_ref()?.cold.links.get(k).map(|l| l.id.clone())
    }
}

impl RuntimeCore {
    /// Node `i`'s border box as drawn this frame (points, x0 y0 x1 y1).
    pub(crate) fn drawn_box(&self, i: usize) -> [f32; 4] {
        self.paint.drawn.get(i).filter(|d| d[2] > 0.0 || d[3] > 0.0)
            .map_or(self.tree.nodes[i].target, |d| [d[0], d[1], d[0] + d[2], d[1] + d[3]])
    }

    /// Line boxes in window points `(x0, y0, x1, y1)` for each `(start, end)` byte span in `spans`
    /// of node `n`, inside border box `rect` drawn at scale `z`.
    fn span_boxes(&self, n: &Node, rect: [f32; 4], z: f32, spans: impl IntoIterator<Item = (u32, u32)>) -> Vec<(usize, [f32; 4])> {
        let (Some(spec), Some(laid)) = (&n.text, &n.text_layout) else { return Vec::new() };
        let flow = laid.view();
        let s = self.env.scale;
        let [ox, oy] = origin(n, spec, flow, rect, s, z);
        let px = s / z.max(1e-4);
        let len = layout_len(n, spec);
        let mut out = Vec::new();
        for (k, (start, end)) in spans.into_iter().enumerate() {
            let (a, b) = drawn(n, start, end);
            for (bb, _) in flow.range_boxes((a as usize).min(len), (b as usize).min(len), false) {
                out.push((k, [ox + bb.x0 as f32 / px, oy + bb.y0 as f32 / px, ox + bb.x1 as f32 / px, oy + bb.y1 as f32 / px]));
            }
        }
        out
    }

    /// Node `i`'s link line boxes in window points (x0, y0, x1, y1) for border box `rect`.
    pub(crate) fn link_boxes(&self, i: usize, rect: [f32; 4]) -> Vec<(usize, [f32; 4])> {
        self.link_boxes_in(&self.tree.nodes, i, rect, self.paint.drawn_scale.get(i).copied().unwrap_or(1.0))
    }

    fn link_boxes_in(&self, nodes: &[Node], i: usize, rect: [f32; 4], z: f32) -> Vec<(usize, [f32; 4])> {
        let n = &nodes[i];
        let Some(spec) = n.text.as_deref() else { return Vec::new() };
        if spec.cold.links.is_empty() { return Vec::new(); }
        self.span_boxes(n, rect, z, spec.cold.links.iter().map(|l| (l.start, l.end)))
    }

    /// The link of node `i` under window point (x, y).
    pub(in crate::runtime) fn link_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        if self.tree.nodes[i].text.as_ref().is_none_or(|t| t.cold.links.is_empty()) { return None; }
        let (rect, x, y) = self.unturned_box_and_point(i, x, y);
        self.link_boxes(i, rect).into_iter()
            .find(|(_, r)| contains(*r, x, y)).map(|(k, _)| k)
    }

    /// The id of the `Mark::Pill` in text field `i` under window point `(x, y)`, if any.
    pub(in crate::runtime) fn input_pill_at(&self, i: usize, x: f32, y: f32) -> Option<String> {
        let n = &self.tree.nodes[i];
        let input = n.input.as_deref()?;
        if input.edit.is_secure() || input.edit.preedit().is_some() { return None; }
        let pills = input.edit.display_pills();
        if pills.is_empty() { return None; }
        let (rect, ux, uy) = self.unturned_box_and_point(i, x, y);
        let z = self.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
        let scroll_x = if input.single_line { self.edit.input_scroll.get(&n.path).copied().unwrap_or(0.0) * z } else { 0.0 };
        self.span_boxes(n, rect, z, pills.iter().map(|&(s, e, _)| (s, e)))
            .into_iter()
            .find(|(_, r)| contains([r[0] - scroll_x, r[1], r[2] - scroll_x, r[3]], ux, uy))
            .map(|(k, _)| pills[k].2.to_owned())
    }

    /// Hit rects for node `i`: only its links' boxes, clipped like `clipped`. False when it
    /// has no links, or is selectable (it takes its whole box).
    pub(in crate::runtime) fn push_link_hits(&mut self, i: usize, rect: [f32; 4], clipped: [f32; 4]) -> bool {
        if self.tree.nodes[i].text.as_ref().is_none_or(|t| t.cold.links.is_empty() || t.selectable) { return false; }
        let id = self.tree.nodes[i].id;
        for (_, r) in self.link_boxes(i, rect) {
            let c = [r[0].max(clipped[0]), r[1].max(clipped[1]), r[2].min(clipped[2]), r[3].min(clipped[3])];
            if c[2] > c[0] && c[3] > c[1] { self.hit.push_hit(id, c); }
        }
        true
    }

    /// Which of node `i`'s links are hovered, pressed and keyboard-focused.
    pub(in crate::runtime) fn link_states(&self, i: usize) -> (Option<usize>, Option<usize>, Option<usize>) {
        let path = &self.tree.nodes[i].path;
        let hovered = self.pointer.pos.filter(|_| self.pointer.hovered.as_deref() == Some(path))
            .and_then(|(x, y)| self.link_at(i, x, y));
        let pressed = self.pointer.press_held().filter(|h| h.path == *path && h.inside()).and_then(|h| h.link).filter(|&k| hovered == Some(k));
        let focused = self.edit.link_focus().filter(|_| self.kbd.focus.keyboard() && self.kbd.focus.path() == Some(path));
        (hovered, pressed, focused)
    }

    /// The paints node `i`'s links show this frame.
    pub(in crate::runtime) fn link_fx(&self, i: usize) -> LinkFx {
        let Some(spec) = self.tree.nodes[i].text.as_ref().filter(|t| !t.cold.links.is_empty()) else { return LinkFx(Vec::new()) };
        let (hovered, pressed, focused) = self.link_states(i);
        let mut fx = Vec::new();
        for (k, l) in spec.cold.links.iter().enumerate() {
            let paint = if pressed == Some(k) {
                l.pressed.unwrap_or(l.hover)
            } else if hovered == Some(k) {
                l.hover
            } else if focused == Some(k) {
                let p = l.focused.unwrap_or(l.hover);
                if p.is_empty() {
                    let ink = spec.spans.iter().filter(|sp| sp.start <= l.start && sp.end > l.start).find_map(|sp| sp.color);
                    LinkPaint::new().underline(ink.unwrap_or(spec.color))
                } else { p }
            } else { continue };
            let (a, b) = drawn(&self.tree.nodes[i], l.start, l.end);
            let pad = spec.spans.iter().find(|sp| sp.start < l.end && sp.end > l.start && sp.bg.is_some()).map_or(2.0, |sp| sp.bg_pad);
            fx.push((a, b, paint, pad, !spec.cold.rings.is_empty()));
        }
        LinkFx(fx)
    }

    /// Pointer up on text `path` of a press with `button` that began on its link `k`: ending on
    /// the same link activates it.
    pub(in crate::runtime) fn link_up(&mut self, path: &str, k: usize, x: f32, y: f32, button: crate::PointerButton) {
        let Some(i) = self.tree.nodes.iter().position(|n| n.path == path) else { return };
        if self.link_at(i, x, y) == Some(k) {
            self.frame.queue.push(Action::Link(path.to_owned(), k, button));
        }
    }

    /// Runs the `on_link` handler of node `path` for link `k`.
    pub(in crate::runtime) fn press_link(&mut self, app: &mut dyn AppDriver, path: &str, k: usize, tx: &mut Tx) -> bool {
        let Some((f, id)) = self.node(path).and_then(|n| {
            let t = n.text.as_ref()?;
            Some((t.cold.on_link.clone()?, t.cold.links.get(k)?.id.clone()))
        }) else { return false };
        log_event!(self, "link {path} {id}");
        (f.0)(app.states(), &id, tx);
        true
    }

    /// Node `i`'s link under (x, y), if its text takes secondary presses on links.
    pub(in crate::runtime) fn secondary_link_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        self.tree.nodes[i].text.as_ref()?.cold.on_link_secondary.as_ref()?;
        self.link_at(i, x, y)
    }

    /// Runs the `on_link_secondary` handler of node `path` for link `k`.
    pub(in crate::runtime) fn secondary_press_link(&mut self, app: &mut dyn AppDriver, path: &str, k: usize, point: PointerEvent, tx: &mut Tx) -> bool {
        let Some((f, id)) = self.node(path).and_then(|n| {
            let t = n.text.as_ref()?;
            Some((t.cold.on_link_secondary.clone()?, t.cold.links.get(k)?.id.clone()))
        }) else { return false };
        log_event!(self, "link secondary {path} {id}");
        (f.0)(app.states(), &id, point.window(), tx);
        true
    }

    /// Centre of the first drawn line box of the link whose id is `id`, in points.
    pub(in crate::runtime) fn find_link(&self, id: &str) -> Option<(f32, f32)> {
        (0..self.tree.nodes.len()).filter(|&i| !self.subtree_exiting(i)).find_map(|i| {
            let k = self.tree.nodes[i].text.as_ref()?.cold.links.iter().position(|l| l.id == id)?;
            let (_, r) = self.link_boxes(i, self.drawn_box(i)).into_iter().find(|(j, _)| *j == k)?;
            Some(((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5))
        })
    }

    /// The focused text's focused link, if any (it may not be in range for that text).
    #[inline]
    pub(crate) fn link_focus(&self) -> Option<usize> { self.edit.link_focus() }

    /// Focuses link `k` of the text about to take focus, as Tab into it does.
    #[inline]
    pub(crate) fn set_link_focus(&mut self, k: usize) { self.edit.set_link_focus(k); }

    /// The Menu key on focused node `i`'s focused link: its `on_link_secondary`, at the
    /// bottom-left of its first line box.
    pub(in crate::runtime) fn focused_link_menu(&mut self, app: &mut dyn AppDriver, i: usize, tx: &mut Tx) -> bool {
        let Some(k) = self.edit.link_focus().filter(|&k| k < link_count(&self.tree.nodes[i])) else { return false };
        if self.tree.nodes[i].text.as_ref().is_none_or(|t| t.cold.on_link_secondary.is_none()) { return false; }
        let Some((_, r)) = self.link_boxes(i, self.drawn_box(i)).into_iter().find(|(j, _)| *j == k) else { return false };
        let path = self.tree.nodes[i].path.clone();
        let point = self.pointer_event(i, r[0], r[3]);
        self.secondary_press_link(app, &path, k, point, tx)
    }

    /// Enter or Space on a focused link presses it.
    pub(in crate::runtime) fn link_key(&mut self, app: &mut dyn AppDriver, i: usize, tx: &mut Tx) -> bool {
        let Some(k) = self.edit.link_focus().filter(|&k| k < link_count(&self.tree.nodes[i])) else { return false };
        let path = self.tree.nodes[i].path.clone();
        self.press_link(app, &path, k, tx);
        true
    }

    /// Reports each link as its own element after its text, which hands it its focus.
    pub(in crate::runtime) fn link_elements(&self, elements: &mut Vec<ElementInfo>) {
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let Some(spec) = n.text.as_ref().filter(|t| !t.cold.links.is_empty()) else { continue };
            let Some(at) = elements.iter().position(|e| e.path == n.path) else { continue };
            let text = elements[at].clone();
            elements[at].focusable = false;
            elements[at].focused = false;
            elements[at].focus_visible = false;
            let (hovered, pressed, _) = self.link_states(i);
            let focused = self.edit.link_focus().filter(|_| self.kbd.focus.path() == Some(&n.path));
            let boxes = self.link_boxes(i, self.drawn_box(i));
            let targets = self.link_boxes_in(&self.tree.nodes, i, n.target, 1.0);
            let union = |k: usize, bs: &[(usize, [f32; 4])]| bs.iter().filter(|(j, _)| *j == k).fold(None, |u: Option<[f32; 4]>, (_, r)| {
                Some(u.map_or(*r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]))
            });
            for (k, l) in spec.cold.links.iter().enumerate() {
                let Some(r) = union(k, &boxes) else { continue };
                let t = union(k, &targets).unwrap_or(r);
                let v = text.visible;
                let (x0, y0) = (r[0].max(v[0]), r[1].max(v[1]));
                let (x1, y1) = (r[2].min(v[0] + v[2]), r[3].min(v[1] + v[3]));
                let words = (!spec.redacted).then(|| spec.source_slice(l.start, l.end).map(str::to_owned)).flatten();
                elements.push(ElementInfo {
                    stuck: 0.0,
                    path: format!("{}#link-{k}", n.path),
                    key: Some(l.id.clone()),
                    role: l.role,
                    label: l.label.clone().or_else(|| words.clone()),
                    text: words,
                    rect: [r[0], r[1], r[2] - r[0], r[3] - r[1]],
                    target: [t[0], t[1], t[2] - t[0], t[3] - t[1]],
                    visible: [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)],
                    focusable: text.focusable,
                    focused: focused == Some(k),
                    focus_visible: self.kbd.focus.keyboard() && focused == Some(k),
                    hover: if hovered == Some(k) { 1.0 } else { 0.0 },
                    press: if pressed == Some(k) { 1.0 } else { 0.0 },
                    parent: Some(i),
                    caret: None,
                    selected: None,
                    level: None,
                    expanded: None,
                    item_selected: false,
                    help: l.help.clone(),
                    ..text.clone()
                });
            }
        }
    }

    /// Node `i`'s highlight line boxes in window points (x0, y0, x1, y1) for border box `rect`.
    pub(in crate::runtime) fn highlight_boxes(&self, i: usize, rect: [f32; 4]) -> Vec<(usize, [f32; 4])> {
        self.highlight_boxes_in(&self.tree.nodes, i, rect, self.paint.drawn_scale.get(i).copied().unwrap_or(1.0))
    }

    pub(in crate::runtime) fn highlight_boxes_in(&self, nodes: &[Node], i: usize, rect: [f32; 4], z: f32) -> Vec<(usize, [f32; 4])> {
        let n = &nodes[i];
        let Some(spec) = n.text.as_deref() else { return Vec::new() };
        if spec.cold.highlights.is_empty() || n.is_secure() { return Vec::new(); }
        self.span_boxes(n, rect, z, spec.cold.highlights.iter().map(|h| (h.start, h.end)))
    }

    /// Unscrolled target rect `[x0, y0, x1, y1]` (in window points) and first node index for
    /// the highlight keyed `key` (or scoped `path/key`).
    pub(in crate::runtime) fn highlight_target_in(&self, nodes: &[Node], scope: Option<&str>, key: &str) -> Option<(usize, [f32; 4])> {
        self.highlight_target_with(nodes, self.key_index_for(nodes), scope, key)
    }

    /// [`Self::highlight_target_in`], looking only at the nodes `index` lists for `key`.
    pub(in crate::runtime) fn highlight_target_with(&self, nodes: &[Node], index: Option<&super::key_index::KeyIndex>, scope: Option<&str>, key: &str) -> Option<(usize, [f32; 4])> {
        let mut first_node = None;
        let mut union: Option<[f32; 4]> = None;
        let mut visit = |i: usize| {
            let n = &nodes[i];
            let Some(spec) = n.text.as_ref().filter(|t| !t.cold.highlights.is_empty()) else { return };
            if !n.parent.is_none_or(|p| super::in_scope(&nodes[p].path, scope)) {
                return;
            }
            let matches: Vec<_> = spec.cold.highlights.iter().map(|h| h.key.as_deref().is_some_and(|hkey| {
                crate::flight::selects_under(key, &n.path, hkey)
            })).collect();
            if matches.iter().any(|m| *m) {
                for (k, r) in self.highlight_boxes_in(nodes, i, n.target, 1.0) {
                    if matches[k] {
                        first_node.get_or_insert(i);
                        union = Some(union.map_or(r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
                    }
                }
            }
        };
        match index {
            Some(index) => index.marked(key).into_iter().for_each(&mut visit),
            None => (0..nodes.len()).for_each(&mut visit),
        }
        Some((first_node?, union?))
    }

    /// Centre of the first drawn line box of the highlight keyed `key`, in points.
    pub(in crate::runtime) fn find_highlight(&self, key: &str) -> Option<(f32, f32)> {
        (0..self.tree.nodes.len()).filter(|&i| !self.subtree_exiting(i)).find_map(|i| {
            let n = &self.tree.nodes[i];
            let k = n.text.as_ref()?.cold.highlights.iter().position(|h| {
                h.key.as_deref().is_some_and(|hk| crate::flight::selects_under(key, &n.path, hk))
            })?;
            let (_, r) = self.highlight_boxes(i, self.drawn_box(i)).into_iter().find(|(j, _)| *j == k)?;
            Some(((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5))
        })
    }

    /// Reports each text highlight as its own `"mark"` element after its text.
    pub(in crate::runtime) fn highlight_elements(&self, elements: &mut Vec<ElementInfo>) {
        if !self.tree.nodes.iter().any(|n| n.text.as_ref().is_some_and(|t| !t.cold.highlights.is_empty())) { return; }
        let text_elements: std::collections::HashMap<_, _> = elements.iter().enumerate().map(|(i, e)| (e.path.clone(), i)).collect();
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let Some(spec) = n.text.as_ref().filter(|t| !t.cold.highlights.is_empty()) else { continue };
            let Some(&at) = text_elements.get(&n.path) else { continue };
            let text = elements[at].clone();
            let boxes = self.highlight_boxes(i, self.drawn_box(i));
            let targets = self.highlight_boxes_in(&self.tree.nodes, i, n.target, 1.0);
            let unions = |bs: &[(usize, [f32; 4])]| {
                let mut out = vec![None; spec.cold.highlights.len()];
                for &(k, r) in bs {
                    out[k] = Some(out[k].map_or(r, |u: [f32; 4]| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
                }
                out
            };
            let (boxes, targets) = (unions(&boxes), unions(&targets));
            for (k, h) in spec.cold.highlights.iter().enumerate() {
                let Some(r) = boxes[k] else { continue };
                let t = targets[k].unwrap_or(r);
                let v = text.visible;
                let (x0, y0) = (r[0].max(v[0]), r[1].max(v[1]));
                let (x1, y1) = (r[2].min(v[0] + v[2]), r[3].min(v[1] + v[3]));
                let words = spec.source_slice(h.start, h.end).map(str::to_owned);
                elements.push(ElementInfo {
                    stuck: 0.0,
                    path: format!("{}#mark-{k}", n.path),
                    key: h.key.clone(),
                    role: "mark",
                    label: words.clone(),
                    text: words,
                    rect: [r[0], r[1], r[2] - r[0], r[3] - r[1]],
                    target: [t[0], t[1], t[2] - t[0], t[3] - t[1]],
                    visible: [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)],
                    fill: h.bg,
                    text_color: h.fg.or(text.text_color),
                    radius: 4.0,
                    focusable: false,
                    focused: false,
                    focus_visible: false,
                    hover: 0.0,
                    press: 0.0,
                    parent: Some(i),
                    caret: None,
                    selected: None,
                    level: None,
                    expanded: None,
                    item_selected: false,
                    ..text.clone()
                });
            }
        }
    }
}
