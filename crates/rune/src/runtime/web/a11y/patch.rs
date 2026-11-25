//! Keyed incremental diffing for the web accessibility mirror.
//!
//! Caches previously sent items (in parent-relative coordinates, keyed by `"{tag}:{path}"`) and
//! per-parent child key orders so scroll settle and single-element edits emit compact patches
//! (`{ "p": 1, "d": [...], "u": [...], "o": {...} }`) instead of rebuilding and re-serializing
//! the entire scene.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use serde_json::{Value, json};

use super::{A11yFont, A11yItem, A11yRun, MirrorAria, round1};
use crate::runtime::HashMap;

#[inline]
pub(super) fn clean_zero(v: f32) -> f32 {
    if v == 0.0 { 0.0 } else { v }
}

/// HTML tag `bridge.js` creates for `item`.
pub(crate) fn item_tag(item: &A11yItem) -> String {
    match item.kind {
        "group" => item.tag.unwrap_or("div").to_owned(),
        "heading" => format!("h{}", item.level.unwrap_or(2)),
        "button" => "button".to_owned(),
        "link" => "a".to_owned(),
        "image" | "progressbar" | "scroll_x" => "div".to_owned(),
        _ => "p".to_owned(),
    }
}

/// Stable DOM-node map key matching `bridge.js` (`"${tag}:${item.p}"`).
pub(crate) fn item_key(item: &A11yItem) -> String {
    format!("{}:{}", item_tag(item), item.path)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SentItem {
    pub kind: &'static str,
    pub path: String,
    /// `[rel_x, rel_y, w, h]` relative to its parent container's origin, rounded to 0.1 px.
    pub rect: [f32; 4],
    pub font_size: f32,
    pub level: Option<u32>,
    pub expanded: Option<bool>,
    pub text: String,
    pub runs: Vec<A11yRun>,
    pub tag: Option<&'static str>,
    pub role: Option<&'static str>,
    pub id: Option<String>,
    pub href: Option<String>,
    pub font: Option<A11yFont>,
    pub selectable: bool,
    pub description: String,
    pub value: String,
    pub pressed: Option<&'static str>,
    pub selected: Option<bool>,
    pub checked: Option<&'static str>,
    pub set: Option<(usize, usize)>,
    pub live: Option<&'static str>,
    pub range: Option<[f64; 3]>,
    pub value_text: String,
    pub disabled: bool,
    pub busy: bool,
    /// App-mode scroll viewport translation `[tx, ty]` in CSS pixels.
    pub scroll: Option<[f32; 2]>,
    pub aria: MirrorAria,
    pub dir: Option<&'static str>,
}

impl SentItem {
    pub(crate) fn from_item(item: &A11yItem, parent_origin: [f32; 2]) -> Self {
        Self {
            kind: item.kind,
            path: item.path.clone(),
            rect: [
                clean_zero(round1(item.rect[0] - parent_origin[0])),
                clean_zero(round1(item.rect[1] - parent_origin[1])),
                clean_zero(item.rect[2]),
                clean_zero(item.rect[3]),
            ],
            font_size: item.font_size,
            level: item.level,
            expanded: item.expanded,
            text: item.text.clone(),
            runs: item.runs.clone(),
            tag: item.tag,
            role: item.role,
            id: item.id.clone(),
            href: item.href.clone(),
            font: item.font.clone(),
            selectable: item.selectable,
            description: item.description.clone(),
            value: item.value.clone(),
            pressed: item.pressed,
            selected: item.selected,
            checked: item.checked,
            set: item.set,
            live: item.live,
            range: item.range,
            value_text: item.value_text.clone(),
            disabled: item.disabled,
            busy: item.busy,
            scroll: item.scroll.map(|[x, y]| [clean_zero(x), clean_zero(y)]),
            aria: item.aria.clone(),
            dir: item.dir,
        }
    }

    pub(crate) fn to_patch_json(&self) -> Value {
        Value::Object(item_json_fields(
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
            None,
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

#[allow(clippy::too_many_arguments)]
pub(super) fn item_json_fields(
    kind: &'static str,
    path: &str,
    rect: [f32; 4],
    font_size: f32,
    level: Option<u32>,
    expanded: Option<bool>,
    text: &str,
    runs: &[A11yRun],
    tag: Option<&'static str>,
    role: Option<&'static str>,
    parent: Option<usize>,
    id: Option<&str>,
    href: Option<&str>,
    font: Option<&A11yFont>,
    selectable: bool,
    description: &str,
    value: &str,
    pressed: Option<&'static str>,
    selected: Option<bool>,
    checked: Option<&'static str>,
    set: Option<(usize, usize)>,
    live: Option<&'static str>,
    range: Option<[f64; 3]>,
    value_text: &str,
    disabled: bool,
    busy: bool,
    scroll: Option<[f32; 2]>,
    aria: &MirrorAria,
    dir: Option<&'static str>,
) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    obj.insert("k".into(), json!(kind));
    obj.insert("p".into(), json!(path));
    obj.insert("r".into(), json!(rect));
    if font_size != 0.0 { obj.insert("s".into(), json!(font_size)); }
    if let Some(l) = level { obj.insert("l".into(), json!(l)); }
    if let Some(x) = expanded { obj.insert("x".into(), json!(x)); }
    if !text.is_empty() { obj.insert("t".into(), json!(text)); }
    if !description.is_empty() { obj.insert("d".into(), json!(description)); }
    if !value.is_empty() { obj.insert("v".into(), json!(value)); }
    if let Some(pr) = pressed { obj.insert("pr".into(), json!(pr)); }
    if let Some(s) = selected { obj.insert("se".into(), json!(s)); }
    if let Some(c) = checked { obj.insert("c".into(), json!(c)); }
    if let Some((pos, size)) = set { obj.insert("n".into(), json!([pos, size])); }
    if let Some(lv) = live { obj.insert("lv".into(), json!(lv)); }
    if let Some([min, max, val]) = range { obj.insert("rg".into(), json!([min, max, val])); }
    if !value_text.is_empty() { obj.insert("vt".into(), json!(value_text)); }
    if disabled { obj.insert("dis".into(), json!(true)); }
    if busy { obj.insert("b".into(), json!(true)); }
    if let Some(g) = tag { obj.insert("g".into(), json!(g)); }
    if let Some(o) = role { obj.insert("o".into(), json!(o)); }
    if let Some(a) = parent { obj.insert("a".into(), json!(a)); }
    if let Some(id) = id { obj.insert("i".into(), json!(id)); }
    if let Some(h) = href { obj.insert("h".into(), json!(h)); }
    if let Some(f) = font {
        obj.insert("f".into(), json!([f.family, f.weight, f.line_height, f.tracking, f.italic, f.align]));
    }
    if selectable { obj.insert("e".into(), json!(true)); }
    if let Some([sx, sy]) = scroll {
        obj.insert("sc".into(), json!([clean_zero(sx), clean_zero(sy)]));
    }
    if aria.multiselectable { obj.insert("ms".into(), json!(true)); }
    if aria.required { obj.insert("rq".into(), json!(true)); }
    if let Some(hp) = aria.has_popup { obj.insert("hp".into(), json!(hp)); }
    if let Some(or) = aria.orientation { obj.insert("or".into(), json!(or)); }
    if let Some(ac) = aria.auto_complete { obj.insert("ac".into(), json!(ac)); }
    if let Some(ct) = aria.controls.as_deref() { obj.insert("ct".into(), json!(ct)); }
    if let Some(ow) = aria.owns.as_deref() { obj.insert("ow".into(), json!(ow)); }
    if let Some(dir) = dir {
        obj.insert("dir".into(), json!(dir));
    }
    if !runs.is_empty() {
        let out: Vec<Value> = runs.iter().map(|r| {
            let mut ro = serde_json::Map::new();
            ro.insert("t".into(), json!(r.text));
            if let Some(h) = &r.href { ro.insert("h".into(), json!(h)); }
            if let Some(p) = &r.path { ro.insert("p".into(), json!(p)); }
            if let Some(i) = r.link { ro.insert("i".into(), json!(i)); }
            if let Some(d) = &r.description { ro.insert("d".into(), json!(d)); }
            Value::Object(ro)
        }).collect();
        obj.insert("u".into(), Value::Array(out));
    }
    obj
}

/// Previously sent web accessibility mirror state, used to emit compact incremental patches.
#[derive(Default)]
pub(crate) struct PatchState {
    sent: HashMap<String, SentItem>,
    /// Child key lists per parent key (`""` for the mirror root, `"{tag}:{path}"` for containers).
    orders: HashMap<String, Vec<String>>,
    root_dir: Option<&'static str>,
    root_lang: Option<String>,
}

impl PatchState {
    /// Diffs `items` against the previously sent state and updates the cache.
    /// Returns `None` when nothing changed, or a patch object `{ "p": 1, "d"?, "u"?, "o"? }`.
    #[cfg(test)]
    pub(crate) fn diff(&mut self, items: &[A11yItem]) -> Option<Value> {
        self.diff_inner(items, None, None)
    }

    /// Diffs `items` together with the mirror root's `dir` (`"ltr"` or `"rtl"`) and optional
    /// BCP 47 `lang` tag (`""` emitted when a previously set `lang` is cleared).
    pub(crate) fn diff_with_root(
        &mut self,
        items: &[A11yItem],
        root_dir: &'static str,
        root_lang: Option<&str>,
    ) -> Option<Value> {
        self.diff_inner(items, Some(root_dir), Some(root_lang))
    }

    fn diff_inner(
        &mut self,
        items: &[A11yItem],
        next_root_dir: Option<&'static str>,
        next_root_lang: Option<Option<&str>>,
    ) -> Option<Value> {
        let keys: Vec<String> = items.iter().map(item_key).collect();
        let mut new_sent: HashMap<String, SentItem> = HashMap::default();
        new_sent.reserve(items.len());
        let mut new_orders: HashMap<String, Vec<String>> = HashMap::default();
        let mut key_parent: HashMap<String, String> = HashMap::default();
        let mut upsert_order: Vec<String> = Vec::with_capacity(items.len());

        for (idx, item) in items.iter().enumerate() {
            let key = &keys[idx];
            let parent_key = item.parent.map_or("", |p| keys[p].as_str());
            let parent_origin = item.parent.map_or([0.0, 0.0], |p| [items[p].rect[0], items[p].rect[1]]);
            let sent_item = SentItem::from_item(item, parent_origin);
            if let Some(old_parent) = key_parent.insert(key.clone(), parent_key.to_owned()) {
                if let Some(siblings) = new_orders.get_mut(&old_parent) {
                    siblings.retain(|k| k != key);
                }
            } else {
                upsert_order.push(key.clone());
            }
            new_sent.insert(key.clone(), sent_item);
            new_orders.entry(parent_key.to_owned()).or_default().push(key.clone());
        }

        let mut removed: Vec<String> = self.sent.keys()
            .filter(|k| !new_sent.contains_key(*k))
            .cloned()
            .collect();
        removed.sort_unstable();

        let mut upserts: Vec<Value> = Vec::new();
        for key in &upsert_order {
            let next = &new_sent[key];
            if self.sent.get(key) != Some(next) {
                upserts.push(next.to_patch_json());
            }
        }

        let mut changed_orders = serde_json::Map::new();
        for (parent_key, next_children) in &new_orders {
            if self.orders.get(parent_key) != Some(next_children) {
                changed_orders.insert(parent_key.clone(), json!(next_children));
            }
        }

        self.sent = new_sent;
        self.orders = new_orders;

        let dir_changed = next_root_dir.is_some_and(|dir| self.root_dir != Some(dir));
        if let Some(dir) = next_root_dir {
            self.root_dir = Some(dir);
        }
        let lang_changed = next_root_lang.is_some_and(|lang| self.root_lang.as_deref() != lang);
        if let Some(lang) = next_root_lang
            && self.root_lang.as_deref() != lang
        {
            self.root_lang = lang.map(str::to_owned);
        }

        if removed.is_empty() && upserts.is_empty() && changed_orders.is_empty() && !dir_changed && !lang_changed {
            return None;
        }
        let mut patch = serde_json::Map::new();
        patch.insert("p".into(), json!(1));
        if dir_changed && let Some(dir) = next_root_dir {
            patch.insert("dir".into(), json!(dir));
        }
        if lang_changed && let Some(lang) = next_root_lang {
            patch.insert("lang".into(), json!(lang.unwrap_or("")));
        }
        if !removed.is_empty() {
            patch.insert("d".into(), json!(removed));
        }
        if !upserts.is_empty() {
            patch.insert("u".into(), Value::Array(upserts));
        }
        if !changed_orders.is_empty() {
            patch.insert("o".into(), Value::Object(changed_orders));
        }
        Some(Value::Object(patch))
    }
}

#[cfg(test)]
#[path = "patch_tests.rs"]
mod tests;
