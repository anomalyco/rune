//! Selectors: which element of a surface an op means. A key or key-path suffix, `role=…`,
//! `text=…`, a `:focus` suffix, and an object form adding a text filter. Only visible elements
//! match, first in tree order.

use serde_json::{Value, json};

use super::protocol::Fail;
use crate::runtime::Runtime;

#[derive(Clone, Debug, PartialEq)]
enum By {
    /// An element key, or a key-path suffix when it holds a `/`.
    Key(String),
    Role(String),
    Text(String),
    /// Only the object form's text filter.
    Any,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Selector {
    by: By,
    focus: bool,
    /// The object form's `text`: the label or text holds it (equals it when `exact`).
    text: Option<String>,
    exact: bool,
}

/// What a selector reads of one element.
pub(crate) struct Candidate<'a> {
    pub key: Option<&'a str>,
    pub path: &'a str,
    pub role: &'a str,
    pub label: Option<&'a str>,
    pub text: Option<&'a str>,
    /// Drawn rect, points.
    pub rect: [f32; 4],
    /// The part of `rect` its ancestors' clips leave, points.
    pub visible: [f32; 4],
    /// Effective, ancestors included.
    pub opacity: f32,
    pub exiting: bool,
    pub focused: bool,
    pub enabled: bool,
}

impl Selector {
    /// A string, or `{"selector": …, "text"?: …, "exact"?: bool}`.
    pub(crate) fn parse(value: &Value) -> Result<Selector, Fail> {
        match value {
            Value::String(s) => Ok(Self::of(s, None, false)),
            Value::Object(o) => {
                let selector = o.get("selector").and_then(Value::as_str).unwrap_or("");
                let text = o.get("text").and_then(Value::as_str).map(str::to_owned);
                let exact = o.get("exact").and_then(Value::as_bool).unwrap_or(false);
                if selector.is_empty() && text.is_none() { return Err(Fail::bad("a selector needs \"selector\" or \"text\"")); }
                Ok(Self::of(selector, text, exact))
            }
            _ => Err(Fail::bad("a selector is a string or {\"selector\": …}")),
        }
    }

    fn of(s: &str, text: Option<String>, exact: bool) -> Selector {
        let (s, focus) = match s.strip_suffix(":focus") { Some(rest) => (rest, true), None => (s, false) };
        let s = s.trim();
        let attr = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(s);
        let by = if let Some(role) = attr.strip_prefix("role=") {
            By::Role(unquote(role).to_owned())
        } else if let Some(t) = s.strip_prefix("text=") {
            By::Text(unquote(t).to_owned())
        } else if s.is_empty() {
            By::Any
        } else {
            By::Key(s.to_owned())
        };
        Selector { by, focus, text, exact }
    }

    /// Whether `c` is this selector's element, visible inside a `viewport` (points) window.
    pub(crate) fn matches(&self, c: &Candidate, viewport: (f32, f32)) -> bool {
        visible(c, viewport) && self.names(c)
    }

    fn names(&self, c: &Candidate) -> bool {
        if self.focus && !c.focused { return false; }
        let shows = |want: &str, exact: bool| [c.label, c.text].into_iter().flatten().any(|t| if exact { t == want } else { t.contains(want) });
        let by = match &self.by {
            By::Key(k) if k.contains('/') => c.path == k || c.path.strip_suffix(k.as_str()).is_some_and(|p| p.ends_with('/')),
            By::Key(k) => c.key == Some(k.as_str()),
            By::Role(r) => c.role == r,
            By::Text(t) => shows(t, self.exact),
            By::Any => true,
        };
        by && self.text.as_deref().is_none_or(|t| shows(t, self.exact))
    }
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"').and_then(|r| r.strip_suffix('"')).or_else(|| s.strip_prefix('\'').and_then(|r| r.strip_suffix('\''))).unwrap_or(s)
}

/// Presented with area, effective opacity above 0.01, not exiting, and inside the window.
fn visible(c: &Candidate, (w, h): (f32, f32)) -> bool {
    let [x, y, vw, vh] = c.visible;
    vw > 0.0 && vh > 0.0 && c.opacity > 0.01 && !c.exiting && x < w && y < h && x + vw > 0.0 && y + vh > 0.0
}

/// The first visible element `sel` matches: its drawn rect.
pub(crate) fn find<S: 'static>(rt: &Runtime<S>, sel: &Selector) -> Option<[f32; 4]> {
    let mut found = None;
    rt.strobe_each(|c| {
        if sel.matches(c, rt.viewport) { found = Some(c.rect); }
        found.is_some()
    });
    found
}

/// What `query` reports of the elements `sel` matches: the first, or with `all` every one.
pub(crate) fn query<S: 'static>(rt: &Runtime<S>, sel: &Selector, all: bool) -> Vec<Value> {
    let mut out = Vec::new();
    rt.strobe_each(|c| {
        if sel.matches(c, rt.viewport) {
            let r = c.rect;
            out.push(json!({
                "key": c.key, "path": c.path, "role": c.role, "label": c.label, "text": c.text,
                "rect": [r[0], r[1], r[2], r[3]], "focused": c.focused, "enabled": c.enabled,
            }));
        }
        !all && !out.is_empty()
    });
    out
}

/// A rect's centre.
pub(crate) fn centre(r: [f32; 4]) -> (f32, f32) { (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5) }
