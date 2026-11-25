//! Hidden semantic DOM mirror for the browser shell: landmarks, headings, text runs with inline
//! links, links, images and buttons positioned over the canvas so assistive tech, the keyboard
//! and browser Find-in-page see the scene. In a page (`App::web_document`) the rects are in
//! document coordinates and the mirror carries the page's scroll, selection and anchors.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use super::*;
use crate::a11y::roles;

pub(crate) mod patch;
pub(crate) mod schedule;

/// Per-frame scroll/commit snapshot used to detect whether scroll or virtual-window movement
/// happened on the current frame rather than comparing against the last flushed mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameStamp {
    commits: u64,
    non_virtual_commits: u64,
    scroll_digest: u64,
}

#[derive(Default)]
pub(crate) struct A11yMirror {
    commits: u64,
    non_virtual_commits: u64,
    live_text_gen: u64,
    viewport: (u32, u32),
    zoom: u32,
    scroll_digest: u64,
    /// Scroll/commit stamp observed on the previous frame (`web_a11y_frame_event`).
    last_frame: Option<FrameStamp>,
    /// Whether motion ran at the last build, and when (clock seconds) it was.
    moving: bool,
    built_at: f64,
    built: bool,
    root_dir: Option<&'static str>,
    root_lang: Option<parley::fontique::Language>,
    json: String,
    patch: patch::PatchState,
    patch_json: String,
    /// The focus proxy last sent (`web_a11y_focus`), and the commit, live-text generation and focus it was read at.
    focus_at: (u64, u64, String, Option<usize>),
    focus_json: String,
}


/// While motion runs (a sheet sliding in, a panel opening), how often the mirror follows what's
/// drawn. Its boxes are the drawn ones, so a build at the commit alone keeps a sliding element
/// where it started: off screen for a sheet, under a reader's focus ring.
const MOVING_EVERY: f64 = 0.1;

#[derive(Debug, PartialEq)]
pub(crate) struct A11yItem {
    pub kind: &'static str,
    pub path: String,
    /// `[x, y, w, h]` in CSS pixels: viewport points, or document points in a page.
    pub rect: [f32; 4],
    pub font_size: f32,
    pub level: Option<u32>,
    pub expanded: Option<bool>,
    pub text: String,
    pub runs: Vec<A11yRun>,
    /// The HTML element a container stands for (`main`, `nav`, `section`, `ul`, ...).
    pub tag: Option<&'static str>,
    /// The ARIA role it carries (a table's parts: `table`, `row`, `cell`, ...).
    pub role: Option<&'static str>,
    /// The index of the container item it sits in.
    pub parent: Option<usize>,
    /// The element id an anchor link scrolls to (`#key`).
    pub id: Option<String>,
    /// A link's URL.
    pub href: Option<String>,
    /// Text's font, so the browser's Find highlight (and, in a page, its selection) lands on the
    /// drawn glyphs.
    pub font: Option<A11yFont>,
    /// Text the browser selects and copies, in a page: `El::selectable` text, and text outside
    /// anything that takes the pointer (`Runtime::page_text_selects`).
    pub selectable: bool,
    /// What describes it beyond its name (`El::description`, `El::described_by`'s words):
    /// `aria-description`.
    pub description: String,
    /// A control's explicit value (`El::a11y_value`): a combobox's selected option or range text.
    pub value: String,
    /// A control's states: `aria-pressed` on a toggle button, `aria-selected`, `aria-checked`,
    /// and its place in its set (`El::position_in_set`) on a control role (`role`).
    pub pressed: Option<&'static str>,
    pub selected: Option<bool>,
    pub checked: Option<&'static str>,
    pub set: Option<(usize, usize)>,
    /// Effective live-region politeness (`aria-live`: `"polite"`, `"assertive"` or `"off"`).
    pub live: Option<&'static str>,
    /// Numeric range (`[min, max, value]` -> `aria-valuemin`, `aria-valuemax`, `aria-valuenow`).
    pub range: Option<[f64; 3]>,
    /// Human-readable value (`El::a11y_value` -> `aria-valuetext`).
    pub value_text: String,
    /// Disabled state (`El::disabled` -> `aria-disabled="true"`).
    pub disabled: bool,
    /// Busy state (`El::busy` -> `aria-busy="true"`).
    pub busy: bool,
    /// App-mode scroll viewport offset `[tx, ty]` in CSS pixels (`translate3d(-tx, -ty, 0)`).
    pub scroll: Option<[f32; 2]>,
    /// Extra WAI-ARIA states and relations (`aria-haspopup`, `aria-orientation`,
    /// `aria-autocomplete`, `aria-multiselectable`, `aria-required`, `aria-controls`, `aria-owns`).
    pub aria: MirrorAria,
    /// Explicit HTML `dir` (`"rtl"` or `"ltr"`) when this item's resolved `Flow` flips from its
    /// mirror parent's; `None` when inherited.
    pub dir: Option<&'static str>,
}

/// Extra WAI-ARIA states and relations mirrored onto a web DOM item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MirrorAria {
    pub multiselectable: bool,
    pub required: bool,
    pub has_popup: Option<&'static str>,
    pub orientation: Option<&'static str>,
    pub auto_complete: Option<&'static str>,
    pub controls: Option<String>,
    pub owns: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct A11yFont {
    /// `"sans"`, `"mono"` or a family name.
    pub family: String,
    pub weight: f32,
    pub line_height: f32,
    pub tracking: f32,
    pub italic: bool,
    /// Logical alignment: `"start"`, `"center"` or `"end"`.
    pub align: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct A11yRun {
    pub text: String,
    pub href: Option<String>,
    pub path: Option<String>,
    pub link: Option<usize>,
    pub description: Option<String>,
}

impl A11yRun {
    fn plain(text: String) -> Self {
        A11yRun { text, href: None, path: None, link: None, description: None }
    }
}

impl A11yItem {
    fn new(kind: &'static str, path: &str, rect: [f32; 4]) -> Self {
        A11yItem { kind, path: path.to_owned(), rect, font_size: 0.0, level: None, expanded: None, text: String::new(),
            runs: Vec::new(), tag: None, role: None, parent: None, id: None, href: None, font: None, selectable: false, description: String::new(), value: String::new(), pressed: None, selected: None, checked: None, set: None,
            live: None, range: None, value_text: String::new(), disabled: false, busy: false, scroll: None, aria: MirrorAria::default(), dir: None }
    }

    fn to_json(&self) -> Value {
        Value::Object(patch::item_json_fields(
            self.kind,
            &self.path,
            self.rect,
            self.font_size,
            self.level,
            self.expanded,
            &self.text,
            &self.runs,
            self.tag,
            self.role,
            self.parent,
            self.id.as_deref(),
            self.href.as_deref(),
            self.font.as_ref(),
            self.selectable,
            &self.description,
            &self.value,
            self.pressed,
            self.selected,
            self.checked,
            self.set,
            self.live,
            self.range,
            &self.value_text,
            self.disabled,
            self.busy,
            self.scroll,
            &self.aria,
            self.dir,
        ))
    }
}

/// An element id from a key, for anchors: letters, digits, `-` and `_`, starting with a letter.
pub(crate) fn anchor_id(key: Option<&str>) -> Option<String> {
    let key = key?;
    let ok = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| key.to_owned())
}

/// Strips control characters and CSS/HTML syntax characters from a font family name so it
/// stays inside a CSS double-quoted string and cannot break out of `<style>` or `cssText`.
pub(crate) fn sanitize_font_family(name: &str) -> String {
    name.chars()
        .filter(|&c| !c.is_control() && !matches!(c, '"' | '\'' | '\\' | ';' | '{' | '}' | '<' | '>'))
        .collect::<String>()
        .trim()
        .to_owned()
}

pub(crate) fn family_name(family: crate::tok::Family) -> String {
    match family {
        crate::tok::Family::Sans => "sans".into(),
        crate::tok::Family::Mono | crate::tok::Family::SystemMono => "mono".into(),
        crate::tok::Family::Named(name) => {
            let clean = sanitize_font_family(name);
            if clean.is_empty() { "sans".into() } else { clean }
        }
    }
}

pub(crate) fn align_name(align: crate::el::Align) -> &'static str {
    match align {
        crate::el::Align::Center => "center",
        crate::el::Align::End => "end",
        crate::el::Align::Start => "start",
    }
}

fn flow_name(flow: crate::el::Flow) -> &'static str {
    match flow {
        crate::el::Flow::Ltr => "ltr",
        crate::el::Flow::Rtl => "rtl",
    }
}

/// Node `n` takes the pointer: a press, drag, gesture or pointer handler, a control, or a field.
/// Hover alone doesn't count; the page's mirror forwards hover to the canvas.
pub(crate) fn takes_pointer(n: &Node) -> bool {
    let h = &n.handlers;
    h.on_press.is_some() || h.on_double_press.is_some() || h.on_secondary_press.is_some() || h.on_middle_press.is_some()
        || h.on_pointer_move.is_some() || h.on_drag.is_some() || h.drag_out.is_some() || h.on_drag_start.is_some()
        || h.gestures.as_ref().is_some_and(|g| g.takes_pointer_gestures())
        || n.input.is_some() || matches!(n.window_drag, crate::el::WindowDrag::Drag | crate::el::WindowDrag::Resize(_))
        || n.a11y.more.as_ref().is_some_and(|m| m.href.is_some())
        || roles::takes_pointer(n.role)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckedAttr { Pressed, Checked, Selected }

/// Which ARIA state attribute carries `El::checked` for `aria_role` (`None` is an HTML `<button>`).
fn checked_attr(aria_role: Option<&str>) -> Option<CheckedAttr> {
    match aria_role {
        None | Some("button") => Some(CheckedAttr::Pressed),
        Some("checkbox" | "radio" | "switch" | "menuitem" | "menuitemcheckbox" | "menuitemradio" | "treeitem") => Some(CheckedAttr::Checked),
        Some("tab" | "option") => Some(CheckedAttr::Selected),
        _ => None,
    }
}

fn checked_str(c: crate::el::Checked) -> &'static str {
    match c { crate::el::Checked::True => "true", crate::el::Checked::False => "false", crate::el::Checked::Mixed => "mixed" }
}

fn live_str(live: crate::LiveRegion) -> &'static str {
    match live {
        crate::LiveRegion::Off => "off",
        crate::LiveRegion::Polite => "polite",
        crate::LiveRegion::Assertive => "assertive",
    }
}

fn effective_live(n: &Node) -> Option<&'static str> {
    n.a11y.live.or_else(|| roles::default_live(n.role)).map(live_str)
}

fn finite_range(n: &Node) -> Option<[f64; 3]> {
    let r = n.a11y.more.as_deref()?.range?;
    (r.min.is_finite() && r.max.is_finite() && r.value.is_finite()).then_some([r.min, r.max, r.value])
}

pub(crate) fn round1(v: f32) -> f32 { (v * 10.0).round() / 10.0 }

pub(crate) fn clean_slice(spec: &crate::el::TextSpec, a: usize, b: usize) -> String {
    let a = a.min(spec.content.len());
    let b = b.min(spec.content.len()).max(a);
    if spec.cold.uncopied.is_empty() {
        return spec.content[a..b].to_owned();
    }
    let mut out = String::with_capacity(b - a);
    let mut cur = a;
    for &(ua, ub) in &spec.cold.uncopied {
        let (ua, ub) = (ua as usize, ub as usize);
        if ub <= cur || ua >= b { continue; }
        let stop = ua.max(cur).min(b);
        if cur < stop { out.push_str(&spec.content[cur..stop]); }
        cur = ub.min(b);
    }
    if cur < b { out.push_str(&spec.content[cur..b]); }
    out
}

/// Inline link help as an accessible description: plain words, omitted when empty or an exact
/// duplicate of the link's own label.
fn link_description(help: Option<&str>, label: &str) -> Option<String> {
    let words = crate::el::help_words(help?);
    let trimmed = words.trim();
    (!trimmed.is_empty() && trimmed != label.trim()).then(|| trimmed.to_owned())
}

fn text_runs(path: &str, spec: &crate::el::TextSpec) -> Vec<A11yRun> {
    if spec.cold.links.is_empty() {
        let t = clean_slice(spec, 0, spec.content.len());
        return if t.is_empty() { Vec::new() } else { vec![A11yRun::plain(t)] };
    }
    let mut out: Vec<A11yRun> = Vec::new();
    let mut at = 0usize;
    let push_plain = |out: &mut Vec<A11yRun>, text: String| {
        if text.is_empty() { return; }
        if let Some(last) = out.last_mut().filter(|r| r.href.is_none()) {
            last.text.push_str(&text);
        } else {
            out.push(A11yRun::plain(text));
        }
    };
    for (idx, link) in spec.cold.links.iter().enumerate() {
        let (la, lb) = ((link.start as usize).min(spec.content.len()), (link.end as usize).min(spec.content.len()));
        if la > at {
            push_plain(&mut out, clean_slice(spec, at, la));
        }
        if lb > la {
            let label = clean_slice(spec, la, lb);
            if !label.is_empty() {
                if let Ok(href) = crate::open_url::check_href(&link.id) {
                    let description = link_description(link.help.as_deref(), &label);
                    out.push(A11yRun {
                        text: label,
                        href: Some(href),
                        path: Some(path.to_owned()),
                        link: Some(idx),
                        description,
                    });
                } else {
                    push_plain(&mut out, label);
                }
            }
        }
        at = at.max(lb);
    }
    if at < spec.content.len() {
        push_plain(&mut out, clean_slice(spec, at, spec.content.len()));
    }
    out
}

impl RuntimeCore {
    #[cfg(test)]
    pub(super) fn scroll_digest_for_test(&self) -> u64 { self.scroll_digest() }

    fn scroll_digest(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // A page hands horizontal scrolling to the DOM, whose container scrolls its own children:
        // its offset changes nothing in the mirror, and hashing it would rewrite the whole page's
        // items on every frame of a horizontal flick.
        let mut entries: Vec<_> = self.scroll.scrolls.iter()
            .map(|(id, s)| (
                id.0,
                s.offset.to_bits(),
                s.drawn.y().to_bits(),
                if self.host.page.is_some() { 0 } else { s.x.offset.to_bits() },
                if self.host.page.is_some() { 0 } else { s.drawn_x.to_bits() },
            ))
            .collect();
        entries.sort_unstable_by_key(|e| e.0);
        entries.hash(&mut h);
        h.finish()
    }

    pub(crate) fn subtree_end(&self, root: usize) -> usize {
        let mut j = root + 1;
        while j < self.tree.nodes.len() {
            if std::iter::successors(self.tree.nodes[j].parent, |&p| self.tree.nodes[p].parent).any(|p| p == root) {
                j += 1;
            } else {
                break;
            }
        }
        j
    }

    /// Every live node's contiguous subtree end, in one pass. Exiting nodes sit after the live
    /// tree with `parent` pointing back into it, so they must not stretch a live ancestor's span
    /// past its later live siblings.
    fn subtree_ends(&self) -> Vec<usize> {
        let mut ends: Vec<usize> = (1..=self.tree.nodes.len()).collect();
        let mut open: Vec<usize> = Vec::new();
        for (i, n) in self.tree.nodes.iter().enumerate() {
            if self.is_exiting(n.id) || !self.scene.contains(n.id) {
                for done in open.drain(..) { ends[done] = i; }
                continue;
            }
            while open.last().is_some_and(|&a| Some(a) != n.parent) {
                let done = open.pop().unwrap();
                ends[done] = i;
            }
            // A descendant of an exiting or unmounted node has no live parent on `open`; leave it
            // unopened so it cannot pull later nodes into a span.
            if n.parent.is_none() || open.last() == n.parent.as_ref() {
                open.push(i);
            }
        }
        for done in open { ends[done] = self.tree.nodes.len(); }
        ends
    }

    /// In a page, which nodes' text the browser selects without `El::selectable`, as a web page's
    /// text selects: all of it but text inside something that takes the pointer (mirrored text
    /// takes the pointer from the canvas, so it would swallow that control's press) or inside
    /// `El::unselectable`. Empty outside a page. A parent always comes before its children.
    pub(crate) fn page_text_selects(&self) -> Vec<bool> {
        if self.host.page.is_none() { return Vec::new(); }
        let mut out = vec![false; self.tree.nodes.len()];
        for (i, n) in self.tree.nodes.iter().enumerate() {
            let inherited = n.parent.is_none_or(|p| out[p]);
            out[i] = inherited && !n.a11y.unselectable && !takes_pointer(n);
        }
        out
    }

    pub(crate) fn descendant_label(&self, root: usize, end: usize) -> String {
        self.descendant_label_with_mask(root, end, None)
    }

    fn descendant_label_with_mask(&self, root: usize, end: usize, mask: Option<&crate::a11y::node::A11yModalMask>) -> String {
        let mut parts = Vec::new();
        let mut j = root;
        while j < end {
            let n = &self.tree.nodes[j];
            if n.is_secure() {
                j = self.subtree_end(j).min(end);
                continue;
            }
            let modal_ok = match mask {
                Some(m) => !n.inert && m.visible(j),
                None => self.allowed(j),
            };
            if !modal_ok || self.subtree_exiting(j) || n.vis.opacity <= 0.01
                || (j > root && (n.a11y.hidden || n.handlers.on_press.is_some() || n.role == "button" || roles::web_control_role(n.role).is_some()))
            {
                j = if j > root { self.subtree_end(j).min(end) } else { j + 1 };
                continue;
            }
            if let Some(lbl) = n.label.as_deref().filter(|s| !s.trim().is_empty()) {
                parts.push(lbl.trim().to_owned());
                j += 1;
                continue;
            }
            if let Some(spec) = n.text.as_deref().filter(|t| !t.redacted && n.input.is_none()) {
                let s = clean_slice(spec, 0, spec.content.len());
                if !s.trim().is_empty() { parts.push(s.trim().to_owned()); }
            }
            j += 1;
        }
        parts.join(" ")
    }

    /// Collect semantic DOM items for the current laid-out scene.
    pub fn a11y_json(&self) -> String {
        let values: Vec<Value> = self.web_a11y_items().iter().map(A11yItem::to_json).collect();
        serde_json::to_string(&values).unwrap_or_else(|_| "[]".into())
    }

    /// Node `i`'s drawn box in the mirror's coordinates (CSS pixels): the band's, plus where the
    /// band starts in the document, and `dx` points right for content inside a horizontal scroller
    /// the browser scrolls itself, scaled by UI zoom (`Cx::zoom`). Both are added and scaled in
    /// `f64` and rounded once, so a fractional device-pixel scroll offset can't round the same
    /// document position two ways between frames.
    fn mirror_rect(&self, i: usize, shift: [f64; 2]) -> [f32; 4] {
        let b = self.drawn_box(i);
        let z = f64::from(self.zoom());
        let (dx, dy) = (shift[0], shift[1] + f64::from(self.page_offset()));
        let (left, right) = ((f64::from(b[0]) + dx) * z, (f64::from(b[2]) + dx) * z);
        let (top, bottom) = ((f64::from(b[1]) + dy) * z, (f64::from(b[3]) + dy) * z);
        [round1(left as f32), round1(top as f32), round1((right - left).max(0.0) as f32), round1((bottom - top).max(0.0) as f32)]
    }

    /// Resolved flow of the scene root (`Flow::Ltr` when empty).
    pub(crate) fn web_a11y_root_flow(&self) -> crate::el::Flow {
        self.tree.nodes.first().map_or(crate::el::Flow::Ltr, |n| n.flow)
    }

    /// HTML `dir` attribute value for `[data-rune-a11y]` (`"ltr"` or `"rtl"`).
    pub(crate) fn web_a11y_root_dir(&self) -> &'static str {
        flow_name(self.web_a11y_root_flow())
    }

    /// Active BCP 47 language for `[data-rune-a11y]`'s `lang` attribute, when set.
    pub(crate) fn web_a11y_root_lang(&self) -> Option<parley::fontique::Language> {
        self.text.current_language()
    }

    pub(crate) fn web_a11y_items(&self) -> Vec<A11yItem> {
        let zoom = self.zoom();
        let root_flow = self.web_a11y_root_flow();
        let mut out: Vec<A11yItem> = Vec::new();
        let mut item_flows: Vec<crate::el::Flow> = Vec::new();
        let mut last_flow_parent: Option<usize> = None;
        let mut inline: Option<Inline> = None;
        let ends = self.subtree_ends();
        let selects = self.page_text_selects();
        let modal_mask = self.a11y_modal_mask();
        // Open containers: (end of their node subtree, their item index).
        let mut open: Vec<(usize, usize)> = Vec::new();
        // Open scrollers and their drawn scroll shift [sx, sy] in view points: added back so
        // content mirrors in scroll-content coordinates while the container carries the offset.
        let mut scrollers: Vec<(usize, [f64; 2])> = Vec::new();
        // End of an emitted control whose subtree is walked for nested interactive descendants, so
        // its own non-interactive content isn't duplicated beside the control.
        let mut control_end: usize = 0;
        let mut i = 0;
        while i < self.tree.nodes.len() {
            while open.last().is_some_and(|&(end, _)| i >= end) { open.pop(); }
            while scrollers.last().is_some_and(|&(end, _)| i >= end) { scrollers.pop(); }
            let parent = open.last().map(|&(_, item)| item);
            let n = &self.tree.nodes[i];
            let end = ends[i];
            if let Some(mask) = &modal_mask
                && !mask.included[i]
                && !mask.outer_modal[i]
            {
                // Non-modal ancestors holding an open modal or kept popup trigger are entered
                // silently; everything else outside the modal is omitted.
                if !n.inert && !self.subtree_exiting(i) && n.vis.opacity > 0.01 && !n.a11y.hidden && mask.ancestor[i] {
                    i += 1;
                } else {
                    i = end;
                }
                last_flow_parent = None;
                continue;
            }
            if n.inert || self.subtree_exiting(i) || n.vis.opacity <= 0.01 || n.a11y.hidden
                || matches!(n.role, "none" | "presentation")
            {
                i = end;
                last_flow_parent = None;
                continue;
            }
            if n.is_secure() {
                i = end;
                last_flow_parent = None;
                continue;
            }
            let busy = n.a11y.more.as_deref().is_some_and(|m| m.busy);
            // A page hands every horizontal viewport to the DOM: iOS scrolls it natively, at the
            // compositor's rate, with axis locking, momentum and text selection.
            let parent_flow = parent.map_or(root_flow, |p| item_flows[p]);
            let node_flow = n.flow;
            let item_dir = (node_flow != parent_flow).then(|| flow_name(node_flow));
            if self.host.page.is_some() && n.scroll_axes == crate::el::ScrollAxes::X {
                let rect = self.mirror_rect(i, [0.0, 0.0]);
                let shift = self.scroll.scrolls.get(&n.id).map_or(0.0, |st| st.x.shift());
                let mut item = A11yItem::new("scroll_x", &n.path, rect);
                item.tag = Some("div");
                item.busy = busy;
                item.parent = parent;
                item.dir = item_dir;
                out.push(item);
                item_flows.push(node_flow);
                open.push((end, out.len() - 1));
                scrollers.push((end, [shift, 0.0]));
                last_flow_parent = None;
                i += 1;
                continue;
            }
            let shift = scrollers.iter().fold([0.0f64; 2], |acc, &(_, off)| [acc[0] + off[0], acc[1] + off[1]]);
            let rect = self.mirror_rect(i, shift);
            let last_item_flow = item_flows.last().copied();
            let mut push = |out: &mut Vec<A11yItem>, mut item: A11yItem| {
                item.parent = parent;
                item.dir = item_dir;
                out.push(item);
                item_flows.push(node_flow);
            };

            let in_control = i < control_end;
            if let Some(inp) = n.input.as_deref() {
                // Mirror editable comboboxes (`text_input(...).role("combobox")`) alongside
                // non-editable comboboxes so `aria-autocomplete`, `aria-expanded`, `aria-haspopup`,
                // and `aria-controls` stay readable both closed and while a popup is open.
                if !in_control && n.role == "combobox" && rect[2] > 0.0 && rect[3] > 0.0 {
                    let label = self.mirror_name(i).unwrap_or_else(|| inp.placeholder.clone());
                    let value = self.mirror_value(i).map(str::to_owned).unwrap_or_else(|| inp.edit.text.clone());
                    if !label.is_empty() || !value.is_empty() {
                        let mut item = A11yItem::new("button", &n.path, rect);
                        item.role = Some("combobox");
                        item.level = n.a11y.level;
                        item.expanded = n.a11y.expanded;
                        item.text = label;
                        item.description = self.mirror_description(i);
                        item.disabled = n.a11y.disabled;
                        item.busy = busy;
                        item.value = value;
                        item.aria = self.mirror_aria(i);
                        push(&mut out, item);
                    }
                }
                i = end;
                last_flow_parent = None;
                continue;
            }
            let live = effective_live(n);
            let live_role = roles::default_live(n.role).and_then(|_| roles::container_aria(n.role));
            let text_aria = (n.text.is_some() && end == i + 1).then(|| roles::container_aria(n.role)).flatten();
            let raw_href = n.a11y.more.as_ref().and_then(|m| m.href.as_deref());
            // A labelled `group` (a game's surface naming its keys, a set of controls) is a div
            // with the role and its label; an unlabelled one adds nothing a reader would say unless
            // it is a live region container.
            let labelled_group = n.role == "group" && self.mirror_name(i).is_some();
            let live_group = n.role == "group" && n.text.is_none() && n.handlers.on_press.is_none()
                && raw_href.is_none() && live.is_some();
            let container_tag = (n.text.is_none() || (live_role.is_none() && text_aria.is_none()))
                .then(|| roles::container_tag(n.role))
                .flatten()
                .or((labelled_group || live_group).then_some("div"));
            let app_scroll = self.host.page.is_none() && n.scroll.is_some();
            if !in_control && (container_tag.is_some() || app_scroll) {
                let mut item = A11yItem::new("group", &n.path, rect);
                item.tag = Some(container_tag.unwrap_or("div"));
                item.role = roles::container_aria(n.role).or(labelled_group.then_some("group"));
                item.live = live;
                item.level = n.a11y.level;
                item.expanded = n.a11y.expanded;
                item.disabled = n.a11y.disabled;
                item.busy = busy;
                item.id = anchor_id(n.key.as_deref());
                item.text = self.mirror_name(i).unwrap_or_default();
                item.description = self.mirror_description(i);
                item.aria = self.mirror_aria(i);
                if app_scroll {
                    let [sx, sy] = self.scroll.scrolls.get(&n.id).map_or([0.0, 0.0], |st| {
                        self.content_to_window(i, [st.drawn_x, st.drawn.y()])
                    });
                    item.scroll = Some([
                        patch::clean_zero(round1((f64::from(sx) * f64::from(zoom)) as f32)),
                        patch::clean_zero(round1((f64::from(sy) * f64::from(zoom)) as f32)),
                    ]);
                    scrollers.push((end, [f64::from(sx), f64::from(sy)]));
                }
                push(&mut out, item);
                open.push((end, out.len() - 1));
                last_flow_parent = None;
                i += 1;
                continue;
            }
            if let Some(href) = raw_href.and_then(|u| crate::open_url::check_href(u).ok()) {
                let label = self.mirror_name(i).unwrap_or_else(|| self.descendant_label_with_mask(i, end, modal_mask.as_ref()));
                if !label.is_empty() && rect[2] > 0.0 && rect[3] > 0.0 {
                    let mut item = A11yItem::new("link", &n.path, rect);
                    item.text = label;
                    item.href = Some(href);
                    item.id = anchor_id(n.key.as_deref());
                    item.disabled = n.a11y.disabled;
                    item.busy = busy;
                    item.aria = self.mirror_aria(i);
                    push(&mut out, item);
                }
                i = end;
                last_flow_parent = None;
                continue;
            }
            if !in_control && matches!(n.role, "image" | "img") && let Some(label) = self.mirror_name(i) {
                let mut item = A11yItem::new("image", &n.path, rect);
                item.text = label;
                item.busy = busy;
                push(&mut out, item);
                i = end;
                last_flow_parent = None;
                continue;
            }
            if !in_control && n.role == "progressbar" && let Some(label) = self.mirror_name(i) {
                let mut item = A11yItem::new("progressbar", &n.path, rect);
                item.role = Some("progressbar");
                item.live = live;
                item.text = label;
                item.description = self.mirror_description(i);
                item.range = finite_range(n);
                item.value_text = self.mirror_value(i).unwrap_or_default().to_owned();
                item.disabled = n.a11y.disabled;
                item.busy = busy;
                item.aria = self.mirror_aria(i);
                push(&mut out, item);
                i = end;
                last_flow_parent = None;
                continue;
            }
            let control_role = roles::web_control_role(n.role);
            let delegated_control = control_role.is_some()
                && !n.a11y.disabled
                && self.a11y_role(i).is_some_and(roles::composite_item)
                && self.a11y_action_ancestor(i).is_some();
            let range_or_combo = matches!(control_role, Some("combobox" | "slider" | "spinbutton"));
            let stateful_control = control_role.is_some()
                && (n.a11y.disabled
                    || self.takes_focus(n)
                    || n.a11y.checked.is_some()
                    || n.focus.selected
                    || n.a11y.expanded.is_some());
            let button_like = raw_href.is_none()
                && n.handlers.on_a11y_action.is_none()
                && (n.role == "button" || range_or_combo || stateful_control || n.handlers.on_press.is_some() || delegated_control)
                && !matches!(n.window_drag, crate::el::WindowDrag::Drag | crate::el::WindowDrag::Resize(_));
            // Only collapse a pressable container into one button when it doesn't wrap an entire
            // multi-block section with independent buttons inside, unless it has an explicit
            // control role (such as a closable tab or treeitem with row actions, whose child
            // controls mirror as siblings).
            let nested_press = (i + 1..end).any(|j| {
                self.tree.nodes[j].handlers.on_press.is_some()
                    || self.tree.nodes[j].role == "button"
                    || roles::web_control_role(self.tree.nodes[j].role).is_some()
            });
            if button_like && (!nested_press || control_role.is_some()) && rect[2] > 0.0 && rect[3] > 0.0 {
                let label = self.mirror_name(i).unwrap_or_else(|| self.descendant_label_with_mask(i, end, modal_mask.as_ref()));
                let value = self.mirror_value(i).unwrap_or_default().to_owned();
                if !label.is_empty() || !value.is_empty() {
                    let mut item = A11yItem::new("button", &n.path, rect);
                    item.level = n.a11y.level;
                    item.expanded = n.a11y.expanded;
                    item.text = label;
                    item.description = self.mirror_description(i);
                    item.disabled = n.a11y.disabled;
                    item.busy = busy;
                    if matches!(control_role, Some("slider" | "spinbutton")) {
                        item.range = finite_range(n);
                        item.value_text = value;
                    } else {
                        item.value = value;
                    }
                    item.aria = self.mirror_aria(i);
                    if let Some(role) = control_role {
                        item.role = Some(role);
                        if self.a11y_role(i).is_some_and(roles::selectable_item) { item.selected = Some(n.focus.selected); }
                        item.set = n.a11y.more.as_ref().and_then(|m| m.set);
                    }
                    let checked = n.a11y.checked
                        .or_else(|| (n.role == "radio").then_some(if n.focus.selected { crate::el::Checked::True } else { crate::el::Checked::False }));
                    if let Some(c) = checked {
                        match checked_attr(item.role) {
                            Some(CheckedAttr::Pressed) => item.pressed = Some(checked_str(c)),
                            Some(CheckedAttr::Checked) => item.checked = Some(checked_str(c)),
                            Some(CheckedAttr::Selected) => item.selected = Some(c == crate::el::Checked::True),
                            None => {}
                        }
                    }
                    push(&mut out, item);
                    if nested_press {
                        control_end = control_end.max(end);
                        i += 1;
                    } else {
                        i = end;
                    }
                    last_flow_parent = None;
                    continue;
                }
            }
            let is_live_text = live_role.is_some() || live.is_some();
            if !in_control && let Some(spec) = n.text.as_deref().filter(|t| !t.redacted && (is_live_text || (rect[2] > 0.0 && rect[3] > 0.0))) {
                let runs = text_runs(&n.path, spec);
                let selectable = spec.selectable || selects.get(i).copied().unwrap_or(false);
                if !runs.is_empty() || is_live_text {
                    let is_heading = n.role == "heading";
                    let plain_text = !is_heading && !is_live_text && text_aria.is_none() && !busy;
                    // Flowed word-by-word paragraphs (`pr_chips::flow`) put sibling words (or 1-word
                    // wrapper rows) inside one flex-wrap line row: merge consecutive inline text runs
                    // sharing that flow container so screen readers and Find-in-page see whole sentences.
                    let flow_group = n.parent.and_then(|p| {
                        if self.tree.nodes[p].key.as_deref().is_none() && self.tree.nodes[p].parent.is_some()
                            && self.tree.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("flow-"))
                        {
                            self.tree.nodes[p].parent
                        } else if self.tree.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("flow-")) {
                            Some(p)
                        } else {
                            None
                        }
                    });
                    if plain_text && flow_group.is_some() && flow_group == last_flow_parent
                        && last_item_flow == Some(node_flow)
                        && let Some(prev) = out.last_mut().filter(|it| it.kind == "text" && it.role.is_none() && it.live.is_none() && !it.busy)
                    {
                        merge_flow(prev, rect, runs);
                        i += 1;
                        continue;
                    }
                    let mut text_rect = inset_rect(rect, n.inset.map(|v| v * zoom));
                    if spec.cold.align_y != Align::Start && let Some(laid) = n.text_layout.as_ref() {
                        let th = text_height(spec, laid.view(), self.env.scale) * zoom / self.env.scale;
                        let ty = text_y(spec, text_rect[1], text_rect[1] + text_rect[3], th);
                        text_rect[1] = round1(ty);
                        text_rect[3] = round1(th.min(text_rect[3]).max(0.0));
                    }
                    if plain_text && flow_group.is_none()
                        && let Some(prev) = inline.filter(|l| l.item + 1 == out.len() && l.parent == n.parent)
                        && let Some(gap) = prev.gap(n.target, spec.style, selectable, node_flow)
                        && let Some(item) = out.last_mut().filter(|it| it.kind == "text" && it.role.is_none() && it.live.is_none() && !it.busy)
                    {
                        join_inline(item, text_rect, runs, gap > spec.style.size * 0.15);
                        inline = Some(Inline { right: n.target[2], ..prev });
                        i += 1;
                        continue;
                    }
                    let (plain, rich_runs) = if runs.is_empty() {
                        (String::new(), Vec::new())
                    } else if runs.len() == 1 && runs[0].href.is_none() && flow_group.is_none() {
                        (runs.into_iter().next().unwrap().text, Vec::new())
                    } else {
                        (String::new(), runs)
                    };
                    let mut item = A11yItem::new(if is_heading { "heading" } else { "text" }, &n.path, rect);
                    item.font_size = round1(spec.style.size * zoom);
                    item.level = is_heading.then(|| n.a11y.level.unwrap_or(2).clamp(1, 6));
                    item.role = if is_heading { None } else { live_role.or(text_aria) };
                    item.live = live;
                    item.busy = busy;
                    item.text = plain;
                    item.runs = rich_runs;
                    item.id = if is_heading { anchor_id(n.key.as_deref()) } else { None };
                    // Laid out in the drawn text's font, so the browser's Find highlight (and, in a
                    // page, its selection) lands on the glyphs.
                    item.rect = text_rect;
                    let style = spec.style;
                    item.font = Some(A11yFont {
                        family: family_name(style.family),
                        weight: style.weight,
                        line_height: round1((if style.line_height > 0.0 { style.line_height } else { style.size * 1.25 }) * zoom),
                        tracking: style.tracking * zoom,
                        italic: style.italic,
                        align: align_name(spec.align),
                    });
                    if self.host.page.is_some() {
                        item.selectable = selectable;
                    }
                    push(&mut out, item);
                    last_flow_parent = if plain_text { flow_group } else { None };
                    inline = (plain_text && flow_group.is_none()).then(|| Inline {
                        parent: n.parent,
                        item: out.len() - 1,
                        right: n.target[2],
                        top: n.target[1],
                        bottom: n.target[3],
                        style: spec.style,
                        selectable,
                        flow: node_flow,
                    });
                }
            } else if !in_control && n.role == "heading" && let Some(lbl) = self.mirror_name(i) {
                let mut item = A11yItem::new("heading", &n.path, rect);
                item.font_size = round1(16.0 * zoom);
                item.level = Some(n.a11y.level.unwrap_or(2).clamp(1, 6));
                item.live = live;
                item.busy = busy;
                item.text = lbl;
                item.id = anchor_id(n.key.as_deref());
                push(&mut out, item);
                last_flow_parent = None;
            }
            i += 1;
        }
        out
    }

    /// Node `i`'s name in the mirror, as the native tree names it: its label, else the words of
    /// the element `El::labelled_by` names.
    fn mirror_name(&self, i: usize) -> Option<String> {
        let n = &self.tree.nodes[i];
        n.label.as_deref().filter(|s| !s.trim().is_empty()).map(str::to_owned).or_else(|| {
            let key = n.a11y.more.as_deref()?.labelled_by.as_deref()?;
            Some(self.a11y_words(self.a11y_keyed(i, key)?)).filter(|w| !w.trim().is_empty())
        })
    }

    /// Node `i`'s description in the mirror: `El::description`, else the words of the element
    /// `El::described_by` names.
    fn mirror_description(&self, i: usize) -> String {
        let Some(more) = self.tree.nodes[i].a11y.more.as_deref() else { return String::new() };
        more.description.as_deref().map(str::to_owned)
            .or_else(|| more.described_by.as_deref().and_then(|k| self.a11y_keyed(i, k)).map(|j| self.a11y_words(j)))
            .filter(|w| !w.trim().is_empty())
            .unwrap_or_default()
    }

    /// Node `i`'s explicit value (`El::a11y_value`) for the web mirror and focus proxy, omitted
    /// for secure nodes (or descendants of secure containers) and empty values.
    fn mirror_value(&self, i: usize) -> Option<&str> {
        if std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent).any(|j| self.tree.nodes[j].is_secure()) {
            return None;
        }
        self.tree.nodes[i].a11y.more.as_ref().and_then(|m| m.value.as_deref()).filter(|s| !s.is_empty())
    }

    fn mirror_related_id(&self, i: usize, key: Option<&str>) -> Option<String> {
        let j = self.a11y_related_target(i, key?)?;
        anchor_id(self.tree.nodes[j].key.as_deref())
    }

    fn mirror_aria(&self, i: usize) -> MirrorAria {
        let n = &self.tree.nodes[i];
        let mut aria = MirrorAria::default();
        if let Some(m) = n.a11y.more.as_deref() {
            aria.multiselectable = m.multiselectable;
            aria.required = m.required;
            aria.has_popup = m.has_popup.map(crate::HasPopup::as_aria);
            aria.orientation = m.orientation.map(crate::Orientation::as_aria);
            aria.auto_complete = m.auto_complete.map(crate::AutoComplete::as_aria);
            aria.controls = self.mirror_related_id(i, m.controls.as_deref());
            aria.owns = self.mirror_related_id(i, m.owns.as_deref());
        }
        aria
    }

    /// The page's horizontal scroll viewports: their paths and how far each is scrolled right.
    /// The browser's mirror holds the containers, so the DOM keeps an offset Rune scrolls too.
    pub(crate) fn page_scroll_x(&self) -> Vec<(String, f32)> {
        if self.host.page.is_none() { return Vec::new(); }
        self.tree.nodes.iter()
            .filter(|n| n.scroll_axes == crate::el::ScrollAxes::X)
            .filter_map(|n| self.scroll.scrolls.get(&n.id).map(|st| (n.path.clone(), st.x.offset as f32)))
            .collect()
    }

    #[inline]
    fn mirror_viewport(&self) -> (u32, u32) {
        (self.viewport.0.to_bits(), if self.host.page.is_some() { 0 } else { self.viewport.1.to_bits() })
    }

    /// Cheap O(1) stamp check: true when the mirror has not been built yet or commits, live text,
    /// viewport, zoom, scroll offsets, or motion moved since the last build.
    pub(crate) fn web_a11y_needs_sync(&self, mirror: &A11yMirror) -> bool {
        if !mirror.built { return true; }
        let vp = self.mirror_viewport();
        let zoom = self.zoom().to_bits();
        let sd = self.scroll_digest();
        let moving = self.animating;
        let follow = moving != mirror.moving || (moving && self.clock - mirror.built_at >= MOVING_EVERY);
        let root_dir = self.web_a11y_root_dir();
        let root_lang = self.web_a11y_root_lang();
        mirror.commits != self.commit.commits
            || mirror.live_text_gen != self.frame.live_text_gen
            || mirror.viewport != vp
            || mirror.zoom != zoom
            || mirror.scroll_digest != sd
            || mirror.root_dir != Some(root_dir)
            || mirror.root_lang != root_lang
            || follow
    }

    /// Whether any scroll viewport has ongoing scroll motion right now (active touch scroll,
    /// pending direct scroll, momentum coast, glide, or wheel ease). Unlike [`Self::scrolling`],
    /// this does not stay true from the one-step `hand.scrolling` latch after a scroll finishes
    /// and no later frame steps it.
    pub(crate) fn web_a11y_scroll_active(&self) -> bool {
        self.pointer.touch_scroll().is_some()
            || self.scroll.scrolls.values().any(|st| {
                st.direct()
                    || st.coast().abs() > 1.0
                    || st.glide().is_some()
                    || st.mode.wheel().is_some()
                    || st.x.coast().abs() > 1.0
                    || st.x.carried()
            })
    }

    /// Builds the [`schedule::ScheduleEvent::Frame`] input for the web host and tests alike.
    ///
    /// Reports whether scroll motion is active or scroll / `CommitWhy::VIRTUAL` movement occurred
    /// on *this* frame (compared against the previous frame's stamp), not how stale the unflushed
    /// mirror is.
    pub(crate) fn web_a11y_frame_event(
        &self,
        mirror: &mut A11yMirror,
        now: crate::time::Instant,
        document: bool,
    ) -> schedule::ScheduleEvent {
        let dirty = self.web_a11y_needs_sync(mirror);
        let cur = FrameStamp {
            commits: self.commit.commits,
            non_virtual_commits: self.commit.non_virtual_commits,
            scroll_digest: self.scroll_digest(),
        };
        let prev = mirror.last_frame.replace(cur);
        let frame_scroll_moved = prev.is_some_and(|p| {
            p.scroll_digest != cur.scroll_digest
                || (p.commits != cur.commits && p.non_virtual_commits == cur.non_virtual_commits)
        });
        schedule::ScheduleEvent::Frame {
            now,
            document,
            dirty,
            scrolling: self.web_a11y_scroll_active() || frame_scroll_moved,
            non_virtual_commit: cur.non_virtual_commits != mirror.non_virtual_commits,
        }
    }

    /// Builds the [`schedule::ScheduleEvent::SettleDue`] input for the web host and tests alike.
    pub(crate) fn web_a11y_settle_event(
        &self,
        attempt: crate::machine::Attempt,
        now: crate::time::Instant,
    ) -> schedule::ScheduleEvent {
        schedule::ScheduleEvent::SettleDue {
            attempt,
            now,
            scrolling: self.web_a11y_scroll_active(),
        }
    }

    /// Builds the [`schedule::ScheduleEvent::IdleDue`] input for the web host and tests alike.
    pub(crate) fn web_a11y_idle_event(
        &self,
        attempt: crate::machine::Attempt,
        now: crate::time::Instant,
    ) -> schedule::ScheduleEvent {
        schedule::ScheduleEvent::IdleDue {
            attempt,
            now,
            scrolling: self.web_a11y_scroll_active(),
        }
    }

    /// Builds and diffs the mirror against what was previously sent, returning a compact
    /// incremental patch JSON (`{ "p": 1, "d"?, "u"?, "o"? }`) or `None` when nothing changed.
    pub(crate) fn web_a11y_patch<'a>(&self, mirror: &'a mut A11yMirror) -> Option<&'a str> {
        if !self.web_a11y_needs_sync(mirror) {
            return None;
        }
        let sd = self.scroll_digest();
        mirror.moving = self.animating;
        mirror.built_at = self.clock;
        mirror.commits = self.commit.commits;
        mirror.non_virtual_commits = self.commit.non_virtual_commits;
        mirror.live_text_gen = self.frame.live_text_gen;
        mirror.viewport = self.mirror_viewport();
        mirror.zoom = self.zoom().to_bits();
        mirror.scroll_digest = sd;
        mirror.last_frame = Some(FrameStamp {
            commits: self.commit.commits,
            non_virtual_commits: self.commit.non_virtual_commits,
            scroll_digest: sd,
        });
        let root_dir = self.web_a11y_root_dir();
        let root_lang = self.web_a11y_root_lang();
        mirror.root_dir = Some(root_dir);
        mirror.root_lang = root_lang;
        mirror.built = true;
        let items = self.web_a11y_items();
        let lang_str = root_lang.map(|l| l.to_string());
        let patch = mirror.patch.diff_with_root(&items, root_dir, lang_str.as_deref())?;
        mirror.patch_json = serde_json::to_string(&patch).unwrap_or_else(|_| r#"{"p":1}"#.into());
        Some(&mirror.patch_json)
    }

    /// Returns updated full-snapshot JSON only when layout commits, viewport, zoom or scroll
    /// positions changed.
    pub(crate) fn web_a11y<'a>(&self, mirror: &'a mut A11yMirror) -> Option<&'a str> {
        let vp = self.mirror_viewport();
        let zoom = self.zoom().to_bits();
        let sd = self.scroll_digest();
        // Motion moves drawn boxes without a commit: follow it while it runs, and once more where it stops.
        let moving = self.animating;
        let follow = moving != mirror.moving || (moving && self.clock - mirror.built_at >= MOVING_EVERY);
        if mirror.commits == self.commit.commits && mirror.live_text_gen == self.frame.live_text_gen && mirror.viewport == vp && mirror.zoom == zoom && mirror.scroll_digest == sd && !follow && !mirror.json.is_empty() {
            return None;
        }
        mirror.moving = moving;
        mirror.built_at = self.clock;
        mirror.commits = self.commit.commits;
        mirror.non_virtual_commits = self.commit.non_virtual_commits;
        mirror.live_text_gen = self.frame.live_text_gen;
        mirror.viewport = vp;
        mirror.zoom = zoom;
        mirror.scroll_digest = sd;
        mirror.built = true;
        let values: Vec<Value> = self.web_a11y_items().iter().map(A11yItem::to_json).collect();
        let next = serde_json::to_string(&values).unwrap_or_else(|_| "[]".into());
        if mirror.json == next {
            return None;
        }
        mirror.json = next;
        Some(&mirror.json)
    }

    /// An app's focus, for the canvas's focus proxy (`focusA11y` in `bridge.js`): the page keeps
    /// the browser's focus on the canvas (keys go there) or its text field, so without this a
    /// reader heard nothing as Tab, arrows or a request moved Rune's focus. The focused
    /// element's ARIA role, name, description and states; empty when nothing has focus. `None`
    /// when unchanged since the last call.
    pub(crate) fn web_a11y_focus<'a>(&self, mirror: &'a mut A11yMirror) -> Option<&'a str> {
        let at = (self.commit.commits, self.frame.live_text_gen, self.kbd.focus.path().unwrap_or_default().to_owned(), self.link_focus());
        if mirror.focus_at == at { return None; }
        mirror.focus_at = at;
        // A field naming a highlighted row (`El::active_descendant`, the palette's): the row is
        // what a reader is told about, as the native trees hand it focus.
        let next = self.focused_index().map(|i| match self.a11y_active_descendant(i) {
            Some(row) => {
                let mut item = self.web_focus_item(row);
                if let Some(o) = item.as_object_mut() {
                    o.insert("f".into(), json!(self.mirror_name(i).unwrap_or_default()));
                    if let Some(ac) = self.mirror_aria(i).auto_complete {
                        o.insert("fac".into(), json!(ac));
                    }
                }
                item.to_string()
            }
            None => self.web_focus_item(i).to_string(),
        }).unwrap_or_default();
        if mirror.focus_json == next { return None; }
        mirror.focus_json = next;
        Some(&mirror.focus_json)
    }

    fn web_focus_item(&self, i: usize) -> Value {
        let n = &self.tree.nodes[i];
        if self.focused_index() == Some(i) && !n.is_secure()
            && let Some(k) = self.link_focus()
            && let Some(spec) = n.text.as_deref().filter(|t| !t.redacted)
            && let Some(link) = spec.cold.links.get(k)
        {
            let role = roles::map(link.role, &roles::RoleCtx {
                pressable: true, focusable: true, input: false, multiline: false, secure: false, scrolls: false, modal: false,
                default_button: false, labelled: true, keyed: true, has_text: true, image: false,
            }).map_or("link", roles::focus_role);
            let name = link.label.as_deref().filter(|s| !s.trim().is_empty()).map(str::to_owned)
                .unwrap_or_else(|| clean_slice(spec, link.start as usize, link.end as usize));
            let mut o = serde_json::Map::new();
            o.insert("o".into(), json!(role));
            o.insert("t".into(), json!(name));
            if let Some(d) = link_description(link.help.as_deref(), &name) {
                o.insert("d".into(), json!(d));
            }
            return Value::Object(o);
        }
        let role = self.a11y_role(i);
        let focus_role = role.map_or("group", roles::focus_role);
        let mut o = serde_json::Map::new();
        o.insert("o".into(), json!(focus_role));
        let name = self.mirror_name(i).unwrap_or_else(|| self.descendant_label(i, self.subtree_end(i)));
        o.insert("t".into(), json!(name));
        let description = self.mirror_description(i);
        if !description.is_empty() { o.insert("d".into(), json!(description)); }
        let mut selected = role.is_some_and(roles::selectable_item).then_some(n.focus.selected);
        let checked = n.a11y.checked.or_else(|| (role == Some(accesskit::Role::RadioButton)).then_some(if n.focus.selected { crate::el::Checked::True } else { crate::el::Checked::False }));
        if let Some(c) = checked {
            match checked_attr(Some(focus_role)) {
                Some(CheckedAttr::Pressed) => { o.insert("pr".into(), json!(checked_str(c))); }
                Some(CheckedAttr::Checked) => { o.insert("c".into(), json!(checked_str(c))); }
                Some(CheckedAttr::Selected) => { selected = Some(c == crate::el::Checked::True); }
                None => {}
            }
        }
        if let Some(s) = selected { o.insert("s".into(), json!(s)); }
        if let Some(open) = n.a11y.expanded { o.insert("x".into(), json!(open)); }
        if let Some((pos, size)) = n.a11y.more.as_ref().and_then(|m| m.set) { o.insert("n".into(), json!([pos, size])); }
        if let Some(level) = n.a11y.level { o.insert("l".into(), json!(level)); }
        if n.a11y.disabled || n.inert { o.insert("dis".into(), json!(true)); }
        if n.a11y.more.as_deref().is_some_and(|m| m.busy) { o.insert("b".into(), json!(true)); }
        let in_secure = std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent)
            .any(|j| self.tree.nodes[j].is_secure());
        if !in_secure && let Some(rg) = finite_range(n) { o.insert("rg".into(), json!(rg)); }
        if let Some(value) = self.mirror_value(i) { o.insert("v".into(), json!(value)); }
        let aria = self.mirror_aria(i);
        if aria.multiselectable { o.insert("ms".into(), json!(true)); }
        if aria.required { o.insert("rq".into(), json!(true)); }
        if let Some(hp) = aria.has_popup { o.insert("hp".into(), json!(hp)); }
        if let Some(or) = aria.orientation { o.insert("or".into(), json!(or)); }
        if let Some(ac) = aria.auto_complete { o.insert("ac".into(), json!(ac)); }
        if let Some(ct) = aria.controls.as_deref() { o.insert("ct".into(), json!(ct)); }
        if let Some(ow) = aria.owns.as_deref() { o.insert("ow".into(), json!(ow)); }
        Value::Object(o)
    }

    /// Dispatches assistive-tech activation of a mirrored button or inline link.
    pub(crate) fn web_a11y_action(&mut self, event: &serde_json::Value) -> bool {
        let Some(path) = event["path"].as_str() else { return false };
        match event["kind"].as_str() {
            Some("a11y_press") => {
                let Some(i) = self.live(path) else { return false };
                if self.tree.nodes[i].a11y.disabled || !self.allowed(i) { return false; }
                if self.tree.nodes[i].handlers.on_press.is_some() {
                    self.frame.queue.push(Action::Press(path.to_owned(), crate::PointerButton::Primary, false, None));
                    true
                } else if self.tree.nodes[i].input.is_some() {
                    self.queue_a11y(path.to_owned(), crate::a11y::actions::A11yAct::Focus(None));
                    true
                } else {
                    self.queue_a11y_delegate(i, accesskit::Action::Click)
                }
            }
            Some("a11y_link") => {
                let Some(k) = event["link"].as_u64().and_then(|n| usize::try_from(n).ok()) else { return false };
                let Some(i) = self.live(path) else { return false };
                if self.tree.nodes[i].a11y.disabled || !self.allowed(i) || self.tree.nodes[i].text.as_ref().is_none_or(|t| k >= t.cold.links.len()) { return false; }
                self.frame.queue.push(Action::Link(path.to_owned(), k, crate::PointerButton::Primary));
                true
            }
            _ => false,
        }
    }
}

/// Joins one more flowed word run into the paragraph item before it.
fn merge_flow(prev: &mut A11yItem, rect: [f32; 4], runs: Vec<A11yRun>) {
    prev.rect = union(prev.rect, rect);
    let needs_space = prev.runs.last().is_some_and(|r| !r.text.ends_with(char::is_whitespace))
        && runs.first().is_some_and(|r| !r.text.starts_with(char::is_whitespace) && !r.text.starts_with([')', ']', '.', ',', ';', ':']));
    if needs_space {
        if let Some(last) = prev.runs.last_mut().filter(|r| r.href.is_none()) {
            last.text.push(' ');
        } else {
            prev.runs.push(A11yRun::plain(" ".into()));
        }
    }
    if runs.len() == 1 && runs[0].href.is_none()
        && let Some(last) = prev.runs.last_mut().filter(|r| r.href.is_none())
    {
        last.text.push_str(&runs[0].text);
    } else {
        prev.runs.extend(runs);
    }
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
    let (x1, y1) = ((a[0] + a[2]).max(b[0] + b[2]), (a[1] + a[3]).max(b[1] + b[3]));
    [x0, y0, round1(x1 - x0), round1(y1 - y0)]
}

fn inset_rect(rect: [f32; 4], inset: [f32; 4]) -> [f32; 4] {
    [round1(rect[0] + inset[0]), round1(rect[1] + inset[1]),
        round1((rect[2] - inset[0] - inset[2]).max(0.0)), round1((rect[3] - inset[1] - inset[3]).max(0.0))]
}

/// The last text item and where its node's laid-out box ends, so a sibling text that carries on
/// the same line joins it: a code line drawn one text per token mirrors as one line, which reads,
/// finds and copies as the line rather than token by token.
#[derive(Clone, Copy)]
struct Inline {
    parent: Option<usize>,
    item: usize,
    right: f32,
    top: f32,
    bottom: f32,
    style: crate::tok::TextStyle,
    selectable: bool,
    flow: crate::el::Flow,
}

impl Inline {
    /// The gap in points from the last text to `target`, when `target` carries on its line in the
    /// same face; a larger gap is a separate column, not the same run of text.
    fn gap(&self, target: [f32; 4], style: crate::tok::TextStyle, selectable: bool, flow: crate::el::Flow) -> Option<f32> {
        let a = self.style;
        let same_face = (a.family, a.size, a.weight, a.tracking) == (style.family, style.size, style.weight, style.tracking)
            && self.selectable == selectable && self.flow == flow;
        let height = (self.bottom - self.top).min(target[3] - target[1]);
        let overlap = self.bottom.min(target[3]) - self.top.max(target[1]);
        let gap = target[0] - self.right;
        (same_face && height > 0.0 && overlap >= height * 0.5 && (-0.5..=style.size * 0.6).contains(&gap)).then_some(gap)
    }
}

/// Appends a sibling's text to the line item before it, with a space when a visible gap stands
/// for one.
fn join_inline(prev: &mut A11yItem, rect: [f32; 4], mut runs: Vec<A11yRun>, gap: bool) {
    prev.rect = union(prev.rect, rect);
    let ends_blank = if prev.runs.is_empty() { prev.text.ends_with(char::is_whitespace) } else { prev.runs.last().is_some_and(|r| r.text.ends_with(char::is_whitespace)) };
    if gap && !ends_blank && runs.first().is_some_and(|r| !r.text.starts_with(char::is_whitespace)) {
        runs.insert(0, A11yRun::plain(" ".into()));
    }
    if prev.runs.is_empty() && runs.iter().all(|r| r.href.is_none()) {
        for r in runs { prev.text.push_str(&r.text); }
        return;
    }
    if prev.runs.is_empty() {
        prev.runs.push(A11yRun::plain(std::mem::take(&mut prev.text)));
    }
    for r in runs {
        match prev.runs.last_mut() {
            Some(last) if last.href.is_none() && r.href.is_none() => last.text.push_str(&r.text),
            _ => prev.runs.push(r),
        }
    }
}

#[cfg(test)]
#[path = "a11y_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "a11y/mirror_tests.rs"]
mod mirror_tests;
