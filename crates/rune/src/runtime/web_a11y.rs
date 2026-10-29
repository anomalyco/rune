//! Hidden semantic DOM mirror for the browser shell: headings, text runs with inline links, and
//! buttons positioned over the canvas so assistive tech and browser Find-in-page see the scene.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use super::*;

#[derive(Default)]
pub(crate) struct A11yMirror {
    commits: u64,
    viewport: (u32, u32),
    scroll_digest: u64,
    json: String,
}

#[derive(Debug, PartialEq)]
pub(crate) struct A11yItem {
    pub kind: &'static str,
    pub path: String,
    /// `[x, y, w, h]` in viewport points (CSS pixels).
    pub rect: [f32; 4],
    pub font_size: f32,
    pub level: Option<u32>,
    pub expanded: Option<bool>,
    pub text: String,
    pub runs: Vec<A11yRun>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct A11yRun {
    pub text: String,
    pub href: Option<String>,
    pub path: Option<String>,
    pub link: Option<usize>,
}

impl A11yItem {
    fn to_json(&self) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("k".into(), json!(self.kind));
        obj.insert("p".into(), json!(self.path));
        obj.insert("r".into(), json!(self.rect));
        if self.font_size != 0.0 { obj.insert("s".into(), json!(self.font_size)); }
        if let Some(l) = self.level { obj.insert("l".into(), json!(l)); }
        if let Some(x) = self.expanded { obj.insert("x".into(), json!(x)); }
        if !self.text.is_empty() { obj.insert("t".into(), json!(self.text)); }
        if !self.runs.is_empty() {
            let runs: Vec<Value> = self.runs.iter().map(|r| {
                let mut ro = serde_json::Map::new();
                ro.insert("t".into(), json!(r.text));
                if let Some(h) = &r.href { ro.insert("h".into(), json!(h)); }
                if let Some(p) = &r.path { ro.insert("p".into(), json!(p)); }
                if let Some(i) = r.link { ro.insert("i".into(), json!(i)); }
                Value::Object(ro)
            }).collect();
            obj.insert("u".into(), Value::Array(runs));
        }
        Value::Object(obj)
    }
}

fn round1(v: f32) -> f32 { (v * 10.0).round() / 10.0 }

fn rect4(b: [f32; 4]) -> [f32; 4] {
    [round1(b[0]), round1(b[1]), round1((b[2] - b[0]).max(0.0)), round1((b[3] - b[1]).max(0.0))]
}

fn clean_slice(spec: &crate::el::TextSpec, a: usize, b: usize) -> String {
    let a = a.min(spec.content.len());
    let b = b.min(spec.content.len()).max(a);
    if spec.uncopied.is_empty() {
        return spec.content[a..b].to_owned();
    }
    let mut out = String::with_capacity(b - a);
    let mut cur = a;
    for &(ua, ub) in &spec.uncopied {
        let (ua, ub) = (ua as usize, ub as usize);
        if ub <= cur || ua >= b { continue; }
        let stop = ua.max(cur).min(b);
        if cur < stop { out.push_str(&spec.content[cur..stop]); }
        cur = ub.min(b);
    }
    if cur < b { out.push_str(&spec.content[cur..b]); }
    out
}

fn text_runs(path: &str, spec: &crate::el::TextSpec) -> Vec<A11yRun> {
    if spec.links.is_empty() {
        let t = clean_slice(spec, 0, spec.content.len());
        return if t.is_empty() { Vec::new() } else { vec![A11yRun { text: t, href: None, path: None, link: None }] };
    }
    let mut out = Vec::new();
    let mut at = 0usize;
    for (idx, link) in spec.links.iter().enumerate() {
        let (la, lb) = ((link.start as usize).min(spec.content.len()), (link.end as usize).min(spec.content.len()));
        if la > at {
            let before = clean_slice(spec, at, la);
            if !before.is_empty() {
                out.push(A11yRun { text: before, href: None, path: None, link: None });
            }
        }
        if lb > la {
            let label = clean_slice(spec, la, lb);
            if !label.is_empty() {
                out.push(A11yRun {
                    text: label,
                    href: Some(link.id.clone()),
                    path: Some(path.to_owned()),
                    link: Some(idx),
                });
            }
        }
        at = at.max(lb);
    }
    if at < spec.content.len() {
        let tail = clean_slice(spec, at, spec.content.len());
        if !tail.is_empty() {
            out.push(A11yRun { text: tail, href: None, path: None, link: None });
        }
    }
    out
}

impl<S: 'static> Runtime<S> {
    fn scroll_digest(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut entries: Vec<_> = self.scrolls.iter().map(|(id, s)| (id.0, s.offset.to_bits(), s.x.offset.to_bits())).collect();
        entries.sort_unstable_by_key(|e| e.0);
        entries.hash(&mut h);
        h.finish()
    }

    fn subtree_end(&self, root: usize) -> usize {
        let mut j = root + 1;
        while j < self.nodes.len() {
            if std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == root) {
                j += 1;
            } else {
                break;
            }
        }
        j
    }

    fn descendant_label(&self, root: usize, end: usize) -> String {
        let mut parts = Vec::new();
        for j in root..end {
            if !self.allowed(j) || self.subtree_exiting(j) || self.nodes[j].vis.opacity <= 0.01 { continue; }
            if let Some(lbl) = self.nodes[j].label.as_deref().filter(|s| !s.trim().is_empty()) {
                parts.push(lbl.trim().to_owned());
                continue;
            }
            if let Some(spec) = self.nodes[j].text.as_deref().filter(|t| !t.redacted && self.nodes[j].input.is_none()) {
                let s = clean_slice(spec, 0, spec.content.len());
                if !s.trim().is_empty() { parts.push(s.trim().to_owned()); }
            }
        }
        parts.join(" ")
    }

    /// Collect semantic DOM items for the current laid-out scene.
    pub fn a11y_json(&self) -> String {
        let values: Vec<Value> = self.web_a11y_items().iter().map(A11yItem::to_json).collect();
        serde_json::to_string(&values).unwrap_or_else(|_| "[]".into())
    }

    pub(crate) fn web_a11y_items(&self) -> Vec<A11yItem> {
        let mut out: Vec<A11yItem> = Vec::new();
        let mut last_flow_parent: Option<usize> = None;
        let mut i = 0;
        while i < self.nodes.len() {
            let n = &self.nodes[i];
            let end = self.subtree_end(i);
            if !self.allowed(i) || self.subtree_exiting(i) || n.vis.opacity <= 0.01
                || matches!(n.role, "none" | "presentation")
            {
                i = end;
                last_flow_parent = None;
                continue;
            }
            if n.input.is_some() {
                i = end;
                last_flow_parent = None;
                continue;
            }
            let b = self.drawn_box(i);
            let rect = rect4(b);
            let button_like = (n.role == "button" || n.on_press.is_some()) && !n.window_drag;
            // Only collapse a pressable container into one button when it doesn't wrap an entire
            // multi-block section with independent buttons inside.
            let nested_press = (i + 1..end).any(|j| self.nodes[j].on_press.is_some() || self.nodes[j].role == "button");
            if button_like && !nested_press && rect[2] > 0.0 && rect[3] > 0.0 {
                let label = n.label.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| self.descendant_label(i, end));
                if !label.is_empty() {
                    out.push(A11yItem {
                        kind: "button",
                        path: n.path.clone(),
                        rect,
                        font_size: 0.0,
                        level: None,
                        expanded: n.a11y.expanded,
                        text: label,
                        runs: Vec::new(),
                    });
                    i = end;
                    last_flow_parent = None;
                    continue;
                }
            }
            if let Some(spec) = n.text.as_deref().filter(|t| !t.redacted && rect[2] > 0.0 && rect[3] > 0.0) {
                let runs = text_runs(&n.path, spec);
                if !runs.is_empty() {
                    let is_heading = n.role == "heading";
                    // Flowed word-by-word paragraphs (`pr_chips::flow`) put sibling words (or 1-word
                    // wrapper rows) inside one flex-wrap line row: merge consecutive inline text runs
                    // sharing that flow container so screen readers and Find-in-page see whole sentences.
                    let flow_group = n.parent.and_then(|p| {
                        if self.nodes[p].key.as_deref().is_none() && self.nodes[p].parent.is_some()
                            && self.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("flow-"))
                        {
                            self.nodes[p].parent
                        } else if self.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("flow-")) {
                            Some(p)
                        } else {
                            None
                        }
                    });
                    if !is_heading && flow_group.is_some() && flow_group == last_flow_parent
                        && let Some(prev) = out.last_mut().filter(|it| it.kind == "text")
                    {
                        let px1 = prev.rect[0] + prev.rect[2];
                        let py1 = prev.rect[1] + prev.rect[3];
                        let nx0 = prev.rect[0].min(rect[0]);
                        let ny0 = prev.rect[1].min(rect[1]);
                        let nx1 = px1.max(rect[0] + rect[2]);
                        let ny1 = py1.max(rect[1] + rect[3]);
                        prev.rect = [nx0, ny0, round1(nx1 - nx0), round1(ny1 - ny0)];
                        let needs_space = prev.runs.last().is_some_and(|r| !r.text.ends_with(char::is_whitespace))
                            && runs.first().is_some_and(|r| !r.text.starts_with(char::is_whitespace) && !r.text.starts_with([')', ']', '.', ',', ';', ':']));
                        if needs_space {
                            if let Some(last) = prev.runs.last_mut().filter(|r| r.href.is_none()) {
                                last.text.push(' ');
                            } else {
                                prev.runs.push(A11yRun { text: " ".into(), href: None, path: None, link: None });
                            }
                        }
                        if runs.len() == 1 && runs[0].href.is_none()
                            && let Some(last) = prev.runs.last_mut().filter(|r| r.href.is_none())
                        {
                            last.text.push_str(&runs[0].text);
                        } else {
                            prev.runs.extend(runs);
                        }
                        i += 1;
                        continue;
                    }
                    let (plain, rich_runs) = if runs.len() == 1 && runs[0].href.is_none() && flow_group.is_none() {
                        (runs.into_iter().next().unwrap().text, Vec::new())
                    } else {
                        (String::new(), runs)
                    };
                    out.push(A11yItem {
                        kind: if is_heading { "heading" } else { "text" },
                        path: n.path.clone(),
                        rect,
                        font_size: round1(spec.style.size),
                        level: is_heading.then(|| n.a11y.level.unwrap_or(2).clamp(1, 6)),
                        expanded: None,
                        text: plain,
                        runs: rich_runs,
                    });
                    last_flow_parent = if is_heading { None } else { flow_group };
                }
            } else if n.role == "heading" && let Some(lbl) = n.label.clone().filter(|s| !s.trim().is_empty()) {
                out.push(A11yItem {
                    kind: "heading",
                    path: n.path.clone(),
                    rect,
                    font_size: 16.0,
                    level: Some(n.a11y.level.unwrap_or(2).clamp(1, 6)),
                    expanded: None,
                    text: lbl,
                    runs: Vec::new(),
                });
                last_flow_parent = None;
            }
            i += 1;
        }
        out
    }

    /// Returns updated JSON only when layout commits, viewport or scroll positions changed.
    pub(crate) fn web_a11y<'a>(&self, mirror: &'a mut A11yMirror) -> Option<&'a str> {
        let vp = (self.viewport.0.to_bits(), self.viewport.1.to_bits());
        let sd = self.scroll_digest();
        if mirror.commits == self.commits && mirror.viewport == vp && mirror.scroll_digest == sd && !mirror.json.is_empty() {
            return None;
        }
        mirror.commits = self.commits;
        mirror.viewport = vp;
        mirror.scroll_digest = sd;
        let values: Vec<Value> = self.web_a11y_items().iter().map(A11yItem::to_json).collect();
        let next = serde_json::to_string(&values).unwrap_or_else(|_| "[]".into());
        if mirror.json == next {
            return None;
        }
        mirror.json = next;
        Some(&mirror.json)
    }

    /// Dispatches assistive-tech activation of a mirrored button or inline link.
    pub(crate) fn web_a11y_action(&mut self, event: &serde_json::Value) -> bool {
        let Some(path) = event["path"].as_str() else { return false };
        match event["kind"].as_str() {
            Some("a11y_press") => {
                let Some(i) = self.live(path) else { return false };
                if self.nodes[i].on_press.is_none() { return false; }
                self.queue.push(Action::Press(path.to_owned(), crate::PointerButton::Primary));
                true
            }
            Some("a11y_link") => {
                let Some(k) = event["link"].as_u64().and_then(|n| usize::try_from(n).ok()) else { return false };
                let Some(i) = self.live(path) else { return false };
                if self.nodes[i].text.as_ref().is_none_or(|t| k >= t.links.len()) { return false; }
                self.queue.push(Action::Link(path.to_owned(), k));
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mirrors_headings_inline_links_and_buttons_and_skips_idle_frames() {
        fn view(open: &(bool, usize), _: &mut Cx<(bool, usize)>) -> El {
            let mut body = crate::rich("Read spec for details.", vec![]).font(crate::tok::type_::BODY)
                .link(crate::Link::new(5..9, "https://example.com/spec"))
                .on_link(|s: &mut (bool, usize), _, _| s.1 += 1);
            body = body.selectable(true);
            let toggle = crate::row().key("fold").role("button").label(if open.0 { "Hide details" } else { "Show details" })
                .expanded(open.0)
                .on_press(|s: &mut (bool, usize)| s.0 = !s.0)
                .child(crate::text("Details"));
            crate::col().w_full().gap(8.0)
                .child(crate::text("Release notes").role("heading").level(1).font(crate::tok::type_::TITLE))
                .child(body)
                .child(toggle)
        }
        let mut rt = Runtime::new((false, 0usize), view, (480.0, 320.0), Env::default());
        rt.frame(0.0);
        let mut mirror = A11yMirror::default();
        let first = rt.web_a11y(&mut mirror).expect("initial tree").to_owned();
        assert!(rt.web_a11y(&mut mirror).is_none(), "unchanged frame emits nothing");

        let items = rt.web_a11y_items();
        assert_eq!(items.len(), 3);
        assert_eq!((items[0].kind, items[0].level, items[0].text.as_str()), ("heading", Some(1), "Release notes"));
        assert_eq!(items[1].kind, "text");
        assert_eq!(items[1].runs.len(), 3);
        assert_eq!((items[1].runs[1].text.as_str(), items[1].runs[1].href.as_deref()), ("spec", Some("https://example.com/spec")));
        assert_eq!((items[2].kind, items[2].text.as_str(), items[2].expanded), ("button", "Show details", Some(false)));

        let btn_path = items[2].path.clone();
        let link_path = items[1].path.clone();
        assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": btn_path})));
        rt.frame(0.0);
        assert!(rt.state.0);
        let updated = rt.web_a11y(&mut mirror).expect("button flip updates expanded state");
        assert_ne!(first, updated);
        assert!(updated.contains("Hide details"));

        assert!(rt.web_a11y_action(&json!({"kind": "a11y_link", "path": link_path, "link": 0})));
        rt.frame(0.0);
        assert_eq!(rt.state.1, 1);
    }

    #[test]
    fn mac_meta_copy_and_select_all_work_on_selectable_text() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            crate::col().key("doc").selection_group(true)
                .child(crate::text("First paragraph").key("p1").font(crate::tok::type_::BODY).selectable(true))
                .child(crate::text("Second paragraph").key("p2").font(crate::tok::type_::BODY).selectable(true))
        }
        let mut rt = Runtime::new((), view, (400.0, 200.0), Env::default());
        let board = crate::clipboard::memory();
        rt.use_clipboard(&board);
        rt.frame(0.0);
        rt.pointer_down(12.0, 8.0);
        rt.pointer_up(12.0, 8.0);
        rt.frame(0.0);
        // Simulate macOS browser Meta+A (meta=true, ctrl=false) then Meta+C.
        rt.web_key(&json!({"kind":"key", "key":"a", "code":"KeyA", "meta":true, "ctrl":false, "shift":false, "alt":false}));
        rt.frame(0.0);
        assert_eq!(rt.copy_text().as_deref(), Some("First paragraph\nSecond paragraph"));
        rt.web_key(&json!({"kind":"key", "key":"c", "code":"KeyC", "meta":true, "ctrl":false, "shift":false, "alt":false}));
        rt.frame(0.0);
        crate::clipboard::flush_memory(&mut rt);
        assert_eq!(board.text().as_deref().map(String::as_str), Some("First paragraph\nSecond paragraph"));
    }
}
