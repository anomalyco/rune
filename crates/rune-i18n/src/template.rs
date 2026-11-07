//! `{{name}}` placeholders: whitespace inside the braces is ignored, and a name with no argument
//! renders as nothing.

use core::fmt::Display;

/// One piece of a template: literal text or a placeholder's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part<'a> {
    Text(&'a str),
    Arg(&'a str),
}

/// The template's literal runs and placeholders in order, borrowed from it, for templates whose
/// arguments render as their own elements (a bold name, a key chip).
pub fn parts(template: &str) -> impl Iterator<Item = Part<'_>> {
    let mut rest = template;
    let mut pending: Option<&str> = None;
    core::iter::from_fn(move || {
        if let Some(arg) = pending.take() {
            return Some(Part::Arg(arg));
        }
        if rest.is_empty() {
            return None;
        }
        match placeholder(rest) {
            Some((0, name, end)) => {
                rest = &rest[end..];
                Some(Part::Arg(name))
            }
            Some((start, name, end)) => {
                let text = &rest[..start];
                rest = &rest[end..];
                pending = Some(name);
                Some(Part::Text(text))
            }
            None => {
                let text = rest;
                rest = "";
                Some(Part::Text(text))
            }
        }
    })
}

/// The template with each `{{name}}` replaced by its argument's `Display`.
pub fn render(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    for part in parts(template) {
        match part {
            Part::Text(text) => out.push_str(text),
            Part::Arg(name) => {
                if let Some((_, value)) = args.iter().find(|(n, _)| *n == name) {
                    out += &value.to_string();
                }
            }
        }
    }
    out
}

/// The next `{{ name }}` in `text`: its start, its trimmed name and the end just past `}}`.
fn placeholder(text: &str) -> Option<(usize, &str, usize)> {
    let mut from = 0;
    loop {
        let open = from + text[from..].find("{{")?;
        let inner = open + 2;
        let close = inner + text[inner..].find('}')?;
        if close > inner && text[close..].starts_with("}}") {
            return Some((open, text[inner..close].trim(), close + 2));
        }
        from = open + 1;
    }
}
