//! Pure-`std` XML scanner and `<familyset>` parser for Android's `font_fallback.xml` and
//! `fonts.xml`.

use std::path::PathBuf;

use super::{FamilyVariant, LangTag, XmlAlias, XmlFamily, XmlFont, normalize_base_stem, parse_weight};

pub(super) struct ParsedXml {
    pub(super) families: Vec<XmlFamily>,
    pub(super) aliases: Vec<XmlAlias>,
}

struct PendingFamily {
    name: Option<String>,
    langs: Vec<LangTag>,
    variant: FamilyVariant,
    fonts: Vec<XmlFont>,
}

struct PendingFont {
    weight: u16,
    italic: bool,
    index: u32,
    fallback_for: Option<String>,
    text: String,
    wght_axis_values: Vec<u16>,
}

/// Parses `xml` as an Android `<familyset>` document, resolving each `<font>` filename via
/// `resolve_file` and skipping entries whose file does not resolve.
pub(super) fn parse_familyset(xml: &str, mut resolve_file: impl FnMut(&str) -> Option<PathBuf>) -> ParsedXml {
    let mut families = Vec::new();
    let mut aliases = Vec::new();
    let mut parser = XmlScanner::new(xml);

    let mut in_familyset = false;
    let mut familyset_depth: usize = 0;
    let mut depth: usize = 0;
    let mut current_family: Option<PendingFamily> = None;
    let mut current_font: Option<PendingFont> = None;
    let mut font_depth: usize = 0;

    while let Some(token) = parser.next_token() {
        match token {
            XmlToken::Start { name, attrs, self_closing } => {
                let next_depth = depth + 1;
                if !in_familyset {
                    if depth == 0 && name == "familyset" && !self_closing {
                        in_familyset = true;
                        familyset_depth = next_depth;
                    }
                } else if depth == familyset_depth {
                    if name == "family" {
                        let ignore = attr_val(&attrs, "ignore")
                            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
                        if !self_closing && !ignore {
                            let fam_name = attr_val(&attrs, "name")
                                .map(|s| s.trim().to_owned())
                                .filter(|s| !s.is_empty());
                            let langs = attr_val(&attrs, "lang")
                                .map(|raw| {
                                    raw.split([',', ' ', '\t', '\n', '\r'])
                                        .filter_map(LangTag::parse)
                                        .collect()
                                })
                                .unwrap_or_default();
                            let variant = match attr_val(&attrs, "variant")
                                .map(|s| s.trim().to_ascii_lowercase())
                                .as_deref()
                            {
                                Some("compact") => FamilyVariant::Compact,
                                Some("elegant") => FamilyVariant::Elegant,
                                _ => FamilyVariant::Default,
                            };
                            current_family = Some(PendingFamily {
                                name: fam_name,
                                langs,
                                variant,
                                fonts: Vec::new(),
                            });
                        }
                    } else if name == "alias" {
                        let alias_name = attr_val(&attrs, "name")
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let to = attr_val(&attrs, "to")
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let weight = attr_val(&attrs, "weight").and_then(parse_weight);
                        if let (Some(alias_name), Some(to)) = (alias_name, to) {
                            aliases.push(XmlAlias { name: alias_name, to, weight });
                        }
                    }
                } else if depth == familyset_depth + 1
                    && current_family.is_some()
                    && name == "font"
                    && !self_closing
                {
                    let weight = attr_val(&attrs, "weight").and_then(parse_weight).unwrap_or(400);
                    let italic = attr_val(&attrs, "style").is_some_and(|s| {
                        let lower = s.trim().to_ascii_lowercase();
                        lower == "italic" || lower == "oblique"
                    });
                    let index = attr_val(&attrs, "index")
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(0);
                    let fallback_for = attr_val(&attrs, "fallbackFor")
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty());
                    current_font = Some(PendingFont {
                        weight,
                        italic,
                        index,
                        fallback_for,
                        text: String::new(),
                        wght_axis_values: Vec::new(),
                    });
                    font_depth = next_depth;
                } else if current_font.is_some()
                    && depth == font_depth
                    && name == "axis"
                    && let Some(font) = current_font.as_mut()
                    && attr_val(&attrs, "tag").is_some_and(|t| t.trim().eq_ignore_ascii_case("wght"))
                    && let Some(val) = attr_val(&attrs, "stylevalue")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                {
                    font.wght_axis_values.push(val.round().clamp(100.0, 900.0) as u16);
                }

                if !self_closing {
                    depth = next_depth;
                }
            }
            XmlToken::End { name } => {
                if current_font.is_some() && depth == font_depth && name == "font" {
                    if let (Some(font), Some(fam)) = (current_font.take(), current_family.as_mut()) {
                        let raw_file = font.text.trim();
                        if !raw_file.is_empty()
                            && let Some(path) = resolve_file(raw_file)
                        {
                            let file_name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| raw_file.to_owned());
                            let base_stem = normalize_base_stem(&file_name);
                            fam.fonts.push(XmlFont {
                                path,
                                file_name,
                                base_stem,
                                weight: font.weight,
                                italic: font.italic,
                                index: font.index,
                                fallback_for: font.fallback_for,
                                wght_axis_values: font.wght_axis_values,
                            });
                        }
                    }
                } else if current_family.is_some()
                    && depth == familyset_depth + 1
                    && name == "family"
                {
                    current_font = None;
                    if let Some(fam) = current_family.take()
                        && !fam.fonts.is_empty()
                    {
                        families.push(XmlFamily {
                            name: fam.name,
                            langs: fam.langs,
                            variant: fam.variant,
                            fonts: fam.fonts,
                        });
                    }
                } else if in_familyset && depth == familyset_depth && name == "familyset" {
                    current_font = None;
                    current_family = None;
                    in_familyset = false;
                }
                depth = depth.saturating_sub(1);
            }
            XmlToken::Text(text) => {
                if let Some(font) = current_font.as_mut()
                    && depth == font_depth
                {
                    font.text.push_str(&text);
                }
            }
        }
    }

    ParsedXml { families, aliases }
}

enum XmlToken {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End {
        name: String,
    },
    Text(String),
}

fn attr_val<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Minimal, panic-free XML scanner supporting elements, attributes, comments, PIs, DOCTYPE,
/// CDATA, and standard XML character/entity references.
struct XmlScanner<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> XmlScanner<'a> {
    fn new(src: &'a str) -> Self {
        let pos = if src.starts_with('\u{FEFF}') { 3 } else { 0 };
        Self { src, pos }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn next_token(&mut self) -> Option<XmlToken> {
        loop {
            let rest = self.rest();
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                let text = &rest[..end];
                self.pos += end;
                return Some(XmlToken::Text(unescape_xml(text)));
            }
            if let Some(after) = rest.strip_prefix("<!--") {
                if let Some(end) = after.find("-->") {
                    self.pos += 4 + end + 3;
                    continue;
                }
                self.pos = self.src.len();
                return None;
            }
            if let Some(after) = rest.strip_prefix("<![CDATA[") {
                if let Some(end) = after.find("]]>") {
                    let data = &after[..end];
                    self.pos += 9 + end + 3;
                    return Some(XmlToken::Text(data.to_owned()));
                }
                self.pos = self.src.len();
                return None;
            }
            if let Some(after) = rest.strip_prefix("<?") {
                if let Some(end) = after.find("?>") {
                    self.pos += 2 + end + 2;
                    continue;
                }
                self.pos = self.src.len();
                return None;
            }
            if let Some(after) = rest.strip_prefix("<!") {
                if let Some(end) = after.find('>') {
                    self.pos += 2 + end + 1;
                    continue;
                }
                self.pos = self.src.len();
                return None;
            }
            if rest.starts_with("</") {
                self.pos += 2;
                let rest = self.rest();
                let Some(end) = rest.find('>') else {
                    self.pos = self.src.len();
                    return None;
                };
                let name = rest[..end].trim().to_owned();
                self.pos += end + 1;
                if !name.is_empty() {
                    return Some(XmlToken::End { name });
                }
                continue;
            }
            // Start tag `<name ...>` or `<name .../>`.
            self.pos += 1;
            self.skip_ws();
            let name = self.read_name();
            if name.is_empty() {
                // Malformed `<`: skip past `>` or advance to EOF to guarantee progress.
                if let Some(gt) = self.rest().find('>') {
                    self.pos += gt + 1;
                } else {
                    self.pos = self.src.len();
                }
                continue;
            }
            let mut attrs = Vec::new();
            let mut self_closing = false;
            loop {
                self.skip_ws();
                let r = self.rest();
                if r.is_empty() {
                    return None;
                }
                if r.starts_with("/>") {
                    self.pos += 2;
                    self_closing = true;
                    break;
                }
                if r.starts_with('>') {
                    self.pos += 1;
                    break;
                }
                let attr_name = self.read_name();
                if attr_name.is_empty() {
                    // Malformed token inside tag: advance past this char.
                    let step = r.chars().next().map_or(1, char::len_utf8);
                    self.pos += step;
                    continue;
                }
                self.skip_ws();
                if !self.rest().starts_with('=') {
                    continue;
                }
                self.pos += 1;
                self.skip_ws();
                let Some(quote) = self.rest().as_bytes().first().copied().filter(|&b| b == b'"' || b == b'\'') else {
                    // Unquoted attribute value: read until whitespace or tag end.
                    let r = self.rest();
                    let end = r
                        .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                        .unwrap_or(r.len());
                    let val = unescape_xml(&r[..end]);
                    self.pos += end;
                    attrs.push((attr_name, val));
                    continue;
                };
                self.pos += 1;
                let r = self.rest();
                let Some(end) = r.find(quote as char) else {
                    self.pos = self.src.len();
                    return None;
                };
                let val = unescape_xml(&r[..end]);
                self.pos += end + 1;
                attrs.push((attr_name, val));
            }
            return Some(XmlToken::Start { name, attrs, self_closing });
        }
    }

    fn skip_ws(&mut self) {
        let trimmed = self.rest().trim_start_matches(|c: char| c.is_ascii_whitespace());
        self.pos = self.src.len() - trimmed.len();
    }

    fn read_name(&mut self) -> String {
        let r = self.rest();
        let end = r
            .find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '>' | '/' | '<' | '"' | '\''))
            .unwrap_or(r.len());
        let name = &r[..end];
        self.pos += end;
        name.to_owned()
    }
}

fn unescape_xml(input: &str) -> String {
    if !input.contains('&') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';') {
            let entity = &after[..semi];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => {
                    entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}
