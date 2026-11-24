//! Copy and paste formatted text, inspect the clipboard revision counter, position a floating
//! formatting bar over `Cx::field_selection_rects`, hit-test byte offsets with
//! `Tx::text_offset_at`, style and inspect mention pills (`El::on_pill_press`) with leading
//! `Pixels` logos/rings/radii, track live caret and selection moves (`El::on_input_selection`),
//! and draw display-only plain-text `LineFill` code blocks and quotes.
//! `cargo run -p rich-clipboard` (or `-- --scene selected --tree` / `-- --scene decorations --snap target/rich-clipboard.png`).
//!
//! Paste into the field from a browser, Notes, TextEdit or Pages: the field still pastes the
//! plain text, and the panel below shows which forms arrived and the start of the sanitised
//! HTML (on macOS, RTF-only sources arrive converted to HTML). Each paste also prints its
//! HTML. "Copy formatted" puts plain text, HTML and RTF on the clipboard: paste it into
//! TextEdit or Mail and it keeps its bold and italic. "Read clipboard" looks without pasting.
//! "Check revision" reads `rune::clipboard::revision()` without opening the clipboard's
//! contents (available on macOS and Windows; unavailable on Linux and web).
//! Selecting text in the field shows a floating formatting bar above the selection's line boxes;
//! right-clicking the field or clicking the sample line prints the hit-tested UTF-8 byte offset.

#![forbid(unsafe_code)]

use rune::clipboard;
use rune::color::hex;
use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, ClipboardContent, Color, Cx, El, LineFill, Mark, OptionAsAlt, Phase, PillStyle, Pixels, Span, SpanBox, TextEdit, Tx, col, el, rich, row, text, text_input};

const SAMPLE_TEXT: &str = "Click here or right-click the field to hit-test a UTF-8 byte offset.";

const SWATCHES: &[(&str, &str, &str)] = &[
    ("color-red", "Red text colour", "#e05252"),
    ("color-amber", "Amber text colour", "#e09f3e"),
    ("color-green", "Green text colour", "#3fb950"),
    ("color-blue", "Blue text colour", "#3b82f6"),
];

const MARKDOWN_SAMPLE: &str = concat!(
    "Plain Markdown notes (display-only LineFill):\n",
    "```rust\n",
    "let pills = PillStyle::new().ring(border, 1.0).radius(999.0);\n",
    "let fill = LineFill::new(start, end).bg(code_bg);\n",
    "```\n",
    "> Quotes draw a left vertical bar across wrapped lines without changing TextEdit.",
);

struct State {
    field: TextEdit,
    notes: TextEdit,
    gmail_logo: Pixels,
    github_logo: Pixels,
    /// Whether the field marks misspelled words and shows suggestions in its menu.
    spellcheck: bool,
    /// How the last paste or read arrived, and what it held.
    last: Option<(&'static str, ClipboardContent)>,
    /// Last observed OS clipboard revision counter (`None` where unsupported).
    revision: Option<u64>,
    /// Last hit-tested target and UTF-8 byte offset from `Tx::text_offset_at`.
    hit: Option<(&'static str, usize)>,
    /// Last Copy/Cut observed by `El::on_input_copy`: `(cut, plain_payload)`.
    last_copy: Option<(bool, String)>,
    /// Last mention pill inspected via `El::on_pill_press`: `(id, [x, y])`.
    inspected_pill: Option<(String, [f32; 2])>,
    /// Live `(anchor, focus)` and event count updated in the same frame by `El::on_input_selection`.
    selection_readout: (usize, usize),
    selection_events: usize,
    /// macOS Option-as-Alt mode applied to the custom `El::ime_cursor` caret when active.
    option_as_alt: OptionAsAlt,
    /// Whether the custom `El::ime_cursor` caret is active instead of the text fields.
    custom_caret: bool,
}

fn badge_pixels(bg: [u8; 4], fg: [u8; 4], stripe: [u8; 4]) -> Pixels {
    let (w, h) = (16usize, 16usize);
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 7.5;
            let r2 = dx * dx + dy * dy;
            let px = if r2 > 58.0 {
                [0, 0, 0, 0]
            } else if (x == 4 || x == 11) && (4..=11).contains(&y) {
                fg
            } else if y == 5 && (5..=10).contains(&x) {
                stripe
            } else if y == 7 && (6..=9).contains(&x) {
                fg
            } else {
                bg
            };
            let i = (y * w + x) * 4;
            data[i..i + 4].copy_from_slice(&px);
        }
    }
    let pixels = Pixels::new();
    pixels.set(w as u32, h as u32, data);
    pixels
}

impl Default for State {
    fn default() -> Self {
        Self {
            field: TextEdit::new("").rich().allow_color(true),
            notes: TextEdit::new(MARKDOWN_SAMPLE),
            gmail_logo: badge_pixels([234, 67, 53, 255], [255, 255, 255, 255], [251, 188, 5, 255]),
            github_logo: badge_pixels([36, 41, 47, 255], [255, 255, 255, 255], [139, 148, 158, 255]),
            spellcheck: true,
            last: None,
            revision: clipboard::revision(),
            hit: None,
            last_copy: None,
            inspected_pill: None,
            selection_readout: (0, 0),
            selection_events: 0,
            option_as_alt: OptionAsAlt::None,
            custom_caret: false,
        }
    }
}

impl State {
    fn decorations_scene(&mut self) {
        let mut edit = TextEdit::new("Draft with ").rich().allow_color(true);
        edit.insert_pill("app:gmail", "@Gmail");
        edit.insert(" and ");
        edit.insert_pill("app:github", "@GitHub");
        edit.insert(" capsule chips.");
        self.field = edit;
    }

    fn pill_copy_scene(&mut self) {
        let mut edit = TextEdit::new("Open ").rich().allow_color(true);
        edit.insert_pill("app:gmail", "@Gmail");
        edit.insert(" or ");
        edit.insert_pill("app:github", "@GitHub");
        edit.insert(" now.");
        edit.focused = true;
        edit.set_selection(0, edit.text.len());
        self.field = edit;
    }

    fn ime_option_as_alt_scene(&mut self) {
        self.field.focused = false;
        self.notes.focused = false;
        self.custom_caret = true;
        self.option_as_alt = OptionAsAlt::Left;
    }
}

fn service_pill_payload(id: &str) -> Option<String> {
    match id {
        "app:gmail" => Some("https://mail.google.com".to_owned()),
        "app:github" => Some("https://github.com".to_owned()),
        _ => None,
    }
}

/// Find fenced code-block and `>`-quote byte ranges in plain Markdown text for `El::line_fills`.
fn markdown_line_fills(src: &str, code_bg: Color, quote_bar: Color) -> Vec<LineFill> {
    let mut fills = Vec::new();
    let mut offset = 0usize;
    let mut fence_start: Option<usize> = None;
    let mut quote_run: Option<(usize, usize)> = None;
    for raw in src.split_inclusive('\n') {
        let line_start = offset;
        let line_end = offset + raw.len();
        offset = line_end;
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            if let Some((qs, qe)) = quote_run.take() {
                fills.push(LineFill::new(qs, qe).left_bar(2.5, quote_bar));
            }
            if let Some(start) = fence_start.take() {
                fills.push(LineFill::new(start, line_end).bg(code_bg).radius(6.0));
            } else {
                fence_start = Some(line_start);
            }
        } else if fence_start.is_none() && trimmed.starts_with('>') {
            match &mut quote_run {
                Some((_, end)) => *end = line_end,
                None => quote_run = Some((line_start, line_end)),
            }
        } else if fence_start.is_none()
            && let Some((qs, qe)) = quote_run.take()
        {
            fills.push(LineFill::new(qs, qe).left_bar(2.5, quote_bar));
        }
    }
    if let Some(start) = fence_start {
        fills.push(LineFill::new(start, src.len()).bg(code_bg).radius(6.0));
    }
    if let Some((qs, qe)) = quote_run {
        fills.push(LineFill::new(qs, qe).left_bar(2.5, quote_bar));
    }
    fills
}

fn service_pill_style(id: &str) -> PillStyle {
    match id {
        "app:gmail" => PillStyle::new()
            .bg(Color { r: 0.92, g: 0.26, b: 0.21, a: 0.16 })
            .fg(Color { r: 1.0, g: 0.72, b: 0.69, a: 1.0 })
            .ring(Color { r: 0.92, g: 0.26, b: 0.21, a: 0.65 }, 1.0)
            .radius(999.0),
        "app:github" => PillStyle::new()
            .bg(Color { r: 0.55, g: 0.62, b: 0.72, a: 0.16 })
            .fg(Color { r: 0.88, g: 0.92, b: 0.98, a: 1.0 })
            .ring(Color { r: 0.55, g: 0.62, b: 0.72, a: 0.65 }, 1.0)
            .radius(999.0),
        _ => PillStyle::new().radius(6.0),
    }
}

fn sample() -> ClipboardContent {
    ClipboardContent::new("Rune copies bold, italic and coloured text.")
        .with_html("Rune copies <b>bold</b>, <i>italic</i> and <span style=\"color:#e05252\">coloured</span> text.")
        .with_rtf(r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}\f0 Rune copies {\b bold}, {\i italic} and coloured text.}")
}

fn seen(s: &mut State, how: &'static str, content: &ClipboardContent) {
    println!("{how}: text {} bytes, html {:?} bytes, rtf {:?} bytes", content.text().len(), content.html().map(str::len), content.rtf().map(str::len));
    if let Some(html) = content.html() { println!("{html}"); }
    s.last = Some((how, content.clone()));
    s.revision = clipboard::revision();
}

fn revision_status(revision: Option<u64>) -> String {
    match revision {
        Some(n) => format!("Clipboard revision: {n}"),
        None => "Clipboard revision: unavailable".into(),
    }
}

fn hit_status(s: &State) -> String {
    let Some((target, offset)) = s.hit else {
        return "Hit offset: none (click sample text or right-click the field)".into();
    };
    let src = if target == "field" { s.field.text.as_str() } else { SAMPLE_TEXT };
    let tail = src.get(offset..).unwrap_or("");
    let next = tail.chars().next().map_or("<end>".to_owned(), |c| format!("{c:?}"));
    format!("Hit offset ({target}): byte {offset} / {} (next {next})", src.len())
}

fn summary(last: &Option<(&'static str, ClipboardContent)>) -> (String, String) {
    let Some((how, content)) = last else { return ("Paste formatted text into the field".into(), String::new()) };
    let form = |name: &str, len: Option<usize>| match len { Some(n) => format!("{name} {n} B"), None => format!("no {name}") };
    let own = if content.has_own_pills() { ", own pills" } else { "" };
    let head = format!("{how}: text {} B, {}, {}{own}", content.text().len(), form("HTML", content.html().map(str::len)), form("RTF", content.rtf().map(str::len)));
    let preview: String = content.html().unwrap_or("").chars().take(400).collect();
    (head, preview)
}

fn copy_status(last_copy: &Option<(bool, String)>) -> String {
    match last_copy {
        Some((cut, plain)) => format!("Last {}: {plain}", if *cut { "cut" } else { "copy" }),
        None => "Copy a @Gmail or @GitHub pill to export its URL payload while keeping the pill on paste.".into(),
    }
}

fn pill_status(inspected: &Option<(String, [f32; 2])>) -> String {
    match inspected {
        Some((id, [x, y])) => match service_pill_payload(id) {
            Some(url) => format!("Inspected pill {id} ({url}) at ({x:.0}, {y:.0})"),
            None => format!("Inspected pill {id} at ({x:.0}, {y:.0})"),
        },
        None => "Click a @Gmail or @GitHub pill to inspect it via on_pill_press (Shift+click extends selection).".into(),
    }
}

fn selection_status((anchor, focus): (usize, usize), moves: usize) -> String {
    if anchor == focus {
        format!("Selection: caret at byte {focus} ({moves} moves)")
    } else {
        let (a, b) = (anchor.min(focus), anchor.max(focus));
        format!("Selection: bytes {a}..{b} (anchor {anchor}, focus {focus}, {moves} moves)")
    }
}

fn option_as_alt_label(mode: OptionAsAlt) -> &'static str {
    match mode {
        OptionAsAlt::None => "Off",
        OptionAsAlt::Left => "Left",
        OptionAsAlt::Right => "Right",
        OptionAsAlt::Both => "Both",
    }
}

fn option_radio(key: &'static str, mode: OptionAsAlt, active: bool) -> El {
    let t = tok::theme(true);
    row()
        .key(key)
        .role("radio")
        .label(format!("Option as Alt: {}", option_as_alt_label(mode)))
        .checked(active)
        .focusable(true)
        .press_on_return()
        .padding(space::S2)
        .radius(radius::MD)
        .fill(t.card)
        .border(if active { 2.0 } else { 1.0 }, if active { t.text } else { t.card_border })
        .child(text(option_as_alt_label(mode)).font(type_::LABEL).color(t.text))
        .on_press(move |s: &mut State| {
            s.field.focused = false;
            s.notes.focused = false;
            s.custom_caret = true;
            s.option_as_alt = mode;
        })
}

fn button(key: &'static str, label: &str, f: impl Fn(&mut State, &mut Tx) + 'static) -> El {
    let t = tok::theme(true);
    row().key(key).role("button").padding(space::S2).radius(radius::MD).fill(t.card).border(1.0, t.card_border)
        .child(text(label).font(type_::LABEL).color(t.text))
        .on_press_tx(f)
}

fn swatch_button(key: &'static str, label: &'static str, code: &'static str, active: bool) -> El {
    let t = tok::theme(true);
    let fill = hex(code).unwrap_or(t.text);
    row()
        .key(key)
        .role("button")
        .label(label)
        .center()
        .size(26.0, 26.0)
        .radius(radius::MD)
        .fill(t.card)
        .border(if active { 2.0 } else { 1.0 }, if active { t.text } else { t.card_border })
        .child(el().size(14.0, 14.0).radius(3.0).fill(fill))
        .on_press(move |s: &mut State| {
            let next = if s.field.active_color() == Some(code) { None } else { Some(code) };
            s.field.set_color(next);
        })
}

fn selection_bar(sel_rects: &[[f32; 4]], active_color: Option<&str>) -> Option<El> {
    if sel_rects.is_empty() { return None; }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &[rx, ry, rw, rh] in sel_rects {
        x0 = x0.min(rx);
        y0 = y0.min(ry);
        x1 = x1.max(rx + rw);
        y1 = y1.max(ry + rh);
    }
    let bar_w = 128.0_f32;
    let bar_h = 34.0_f32;
    let bar_x = ((x0 + x1) * 0.5 - bar_w * 0.5).max(8.0);
    let bar_y = if y0 >= bar_h + 64.0 { y0 - bar_h - 6.0 } else { y1 + 6.0 };
    let t = tok::theme(true);
    Some(
        row()
            .key("selection-bar")
            .role("toolbar")
            .label("Selection formatting")
            .keeps_text_selection()
            .raise()
            .absolute(bar_x, bar_y)
            .gap(space::S1)
            .padding(space::S1)
            .center_y()
            .radius(radius::MD)
            .fill(t.bg)
            .border(1.0, t.card_border)
            .child(button("float-bold", "B", |s, _| s.field.toggle_mark(Mark::Bold)))
            .child(button("float-italic", "I", |s, _| s.field.toggle_mark(Mark::Italic)))
            .child(swatch_button("float-color-red", "Highlight red", "#e05252", active_color == Some("#e05252"))),
    )
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let (head, preview) = summary(&s.last);
    let active_color = s.field.active_color();
    let sel_rects = cx.field_selection_rects("field");
    let mut toolbar = row().key("toolbar").gap(space::S2).center_y()
        .child(button("bold", "B", |s, _| s.field.toggle_mark(Mark::Bold)));
    for &(key, label, code) in SWATCHES {
        toolbar = toolbar.child(swatch_button(key, label, code, active_color == Some(code)));
    }
    toolbar = toolbar
        .child(button("color-clear", "Reset colour", |s, _| s.field.set_color(None)))
        .child(button("mention-gmail", "@Gmail", |s, _| {
            s.field.undo_group(|e| { e.insert_pill("app:gmail", "@Gmail"); e.insert(" "); });
        }))
        .child(button("mention-github", "@GitHub", |s, _| {
            s.field.undo_group(|e| { e.insert_pill("app:github", "@GitHub"); e.insert(" "); });
        }));

    let (gmail, github) = (s.gmail_logo.clone(), s.github_logo.clone());
    let field = text_input(&s.field, type_::BODY).key("field").label("Rich text editor").w_full().min_h(68.0).padding(space::S2)
        .radius(radius::MD).fill(t.card).border(1.0, t.card_border).color(t.text)
        .spellcheck(s.spellcheck)
        .pill_style(service_pill_style)
        .pill_image(move |id| match id {
            "app:gmail" => Some(gmail.clone()),
            "app:github" => Some(github.clone()),
            _ => None,
        })
        .pill_copy_text(service_pill_payload)
        .on_edit(|s: &mut State| &mut s.field)
        .on_pill_press(|s: &mut State, id, press, _| {
            s.inspected_pill = Some((id.to_owned(), [press.x, press.y]));
            true
        })
        .on_input_selection(|s: &mut State, _| {
            s.selection_readout = (s.field.anchor, s.field.focus);
            s.selection_events += 1;
        })
        // Observe Copy/Cut (keyboard or context menu) and fall back to pill_copy_text + rich HTML.
        .on_input_copy(|s: &mut State, cut, _| {
            let plain = s.field.selected_with_pills(service_pill_payload);
            s.last_copy = Some((cut, plain));
            None
        })
        // Look at every form, then let the editor paste the text as usual.
        .on_input_paste(|s: &mut State, content, _| { seen(s, "Pasted", content); false })
        .on_secondary_press(|_: &mut State, p, tx| {
            tx.text_offset_at("field", p.x, p.y, |s: &mut State, at, _| {
                s.hit = at.map(|o| ("field", o));
            });
        });

    let sample_line = text(SAMPLE_TEXT).key("sample").font(type_::BODY).color(t.text_secondary).w_full().wrap()
        .on_drag(|_: &mut State, d, tx| {
            if d.phase == Phase::Down {
                let [wx, wy] = d.window_point();
                tx.text_offset_at("sample", wx, wy, |s: &mut State, at, _| {
                    s.hit = at.map(|o| ("sample", o));
                });
            }
        })
        .on_secondary_press(|_: &mut State, p, tx| {
            tx.text_offset_at("sample", p.x, p.y, |s: &mut State, at, _| {
                s.hit = at.map(|o| ("sample", o));
            });
        });

    let code_bg = Color { r: 0.25, g: 0.45, b: 0.85, a: 0.14 };
    let quote_bar = Color { r: 0.88, g: 0.62, b: 0.24, a: 0.85 };
    let note_fills = markdown_line_fills(&s.notes.text, code_bg, quote_bar);
    let notes_field = text_input(&s.notes, type_::BODY).key("markdown-field").w_full().min_h(116.0).padding(space::S2)
        .radius(radius::MD).fill(t.card).border(1.0, t.card_border).color(t.text)
        .line_fills(note_fills)
        .on_edit(|s: &mut State| &mut s.notes);

    let static_caption = "Static chips: @Gmail and @GitHub";
    let g0 = static_caption.find("@Gmail").unwrap();
    let h0 = static_caption.find("@GitHub").unwrap();
    let static_pills = text(static_caption)
        .key("static-pills")
        .font(type_::LABEL)
        .color(t.text_secondary)
        .pill_with_image(g0..g0 + "@Gmail".len(), service_pill_style("app:gmail"), s.gmail_logo.clone())
        .pill_with_image(h0..h0 + "@GitHub".len(), service_pill_style("app:github"), s.github_logo.clone());

    // Adjacent padded SpanBox search-filter chips: squared inner corners, rounded outer corners.
    let filter_src = "Filters: from:ada  in:#general";
    let fk0 = filter_src.find("from:").unwrap_or(0);
    let fk1 = fk0 + "from:".len();
    let fv1 = fk1 + "ada".len();
    let ik0 = filter_src.find("in:").unwrap_or(0);
    let ik1 = ik0 + "in:".len();
    let iv1 = ik1 + "#general".len();
    let key_box = SpanBox::new()
        .bg(Color { r: 0.24, g: 0.28, b: 0.36, a: 0.9 })
        .pad_x(6.0, 4.0)
        .corners([4.0, 0.0, 0.0, 4.0]);
    let val_box = SpanBox::new()
        .bg(Color { r: 0.16, g: 0.42, b: 0.78, a: 0.45 })
        .pad_x(4.0, 6.0)
        .corners([0.0, 4.0, 4.0, 0.0])
        .border(Color { r: 0.38, g: 0.66, b: 1.0, a: 0.7 }, 1.0);
    let filter_chips = rich(filter_src, vec![
        Span::new(fk0, fk1).color(t.text_secondary),
        Span::new(fk1, fv1).bold().color(t.text),
        Span::new(ik0, ik1).color(t.text_secondary),
        Span::new(ik1, iv1).bold().color(t.text),
    ])
        .key("filter-chips")
        .font(type_::LABEL)
        .color(t.text_secondary)
        .selectable(true)
        .span_boxes([(fk0..fk1, key_box), (fk1..fv1, val_box), (ik0..ik1, key_box), (ik1..iv1, val_box)]);

    // Overline coexisting with underline and strikethrough, plus a wrapped overline run.
    let overline_src = "OVER + BOTH (over/strike/under) and a wrapped overline segment across narrow widths";
    let over_end = "OVER".len();
    let both_start = overline_src.find("BOTH").unwrap_or(0);
    let both_end = both_start + "BOTH".len();
    let wrap_start = overline_src.find("wrapped").unwrap_or(0);
    let over_color = Color { r: 0.94, g: 0.56, b: 0.24, a: 1.0 };
    let overline_sample = rich(overline_src, vec![
        Span::new(0, over_end).bold().color(over_color),
        Span::new(both_start, both_end).underline(Color { r: 0.35, g: 0.68, b: 1.0, a: 1.0 }),
    ])
        .key("overline-sample")
        .font(type_::LABEL)
        .color(t.text_secondary)
        .w_full()
        .wrap()
        .strike(both_start, both_end, Some(Color { r: 0.4, g: 0.85, b: 0.5, a: 1.0 }))
        .overline(0, over_end, None)
        .overlines([(both_start, both_end, Some(over_color)), (wrap_start, overline_src.len(), Some(over_color))]);

    let html_out = s.field.to_html();
    let mut root = col().w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg)
        .on_spellcheck(s.spellcheck, |s: &mut State, on, _| s.spellcheck = on)
        .on_spellcheck_languages(|_: &mut State, _, _| {})
        .child(toolbar)
        .child(field)
        .child(static_pills)
        .child(filter_chips)
        .child(overline_sample)
        .child(notes_field)
        .child(sample_line)
        .child(text(hit_status(s)).key("hit-offset").font(type_::LABEL).color(t.text_secondary))
        .child(row().gap(space::S2)
            .child(button("copy", "Copy formatted", |_, tx| tx.copy_rich(sample())))
            .child(button("read", "Read clipboard", |_, tx| tx.read_clipboard(|s: &mut State, content, _| seen(s, "Read", content))))
            .child(button("check-revision", "Check revision", |s, _| s.revision = clipboard::revision())))
        .child(
            row()
                .key("option-as-alt-row")
                .gap(space::S2)
                .center_y()
                .child(text("macOS Option-as-Alt:").font(type_::LABEL).color(t.text_secondary))
                .child(
                    row()
                        .key("option-as-alt-group")
                        .role("radiogroup")
                        .label("macOS Option-as-Alt")
                        .gap(space::S1)
                        .child(option_radio("opt-none", OptionAsAlt::None, s.option_as_alt == OptionAsAlt::None))
                        .child(option_radio("opt-left", OptionAsAlt::Left, s.option_as_alt == OptionAsAlt::Left))
                        .child(option_radio("opt-right", OptionAsAlt::Right, s.option_as_alt == OptionAsAlt::Right))
                        .child(option_radio("opt-both", OptionAsAlt::Both, s.option_as_alt == OptionAsAlt::Both)),
                )
                .children(s.custom_caret.then(|| {
                    el().key("custom-ime-caret")
                        .size(2.0, 16.0)
                        .fill(t.text)
                        .ime_cursor()
                        .ime_option_as_alt_mode(s.option_as_alt)
                })),
        )
        .child(text(revision_status(s.revision)).key("revision").font(type_::LABEL).color(t.text_secondary))
        .child(text(pill_status(&s.inspected_pill)).key("pill-status").font(type_::LABEL).color(t.text_secondary).w_full())
        .child(text(selection_status(s.selection_readout, s.selection_events)).key("selection-status").font(type_::LABEL).color(t.text_secondary).w_full())
        .child(text(copy_status(&s.last_copy)).key("copy-status").font(type_::LABEL).color(t.text_secondary).w_full())
        .child(text(head).key("summary").font(type_::BODY).color(t.text_secondary))
        .child(text(preview).key("preview").font(type_::LABEL).color(t.text_secondary).w_full())
        .child(text(html_out).key("html-out").font(type_::LABEL).color(t.text_secondary).w_full());
    if let Some(bar) = selection_bar(&sel_rects, active_color) {
        root = root.child(bar);
    }
    root
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("selected", |s| {
                s.field = TextEdit::new("Rune copies bold, italic and coloured text.").rich();
                s.field.focused = true;
                s.field.set_selection(12, 24);
            }),
            ("decorations", State::decorations_scene),
            ("pill-copy", State::pill_copy_scene),
            ("ime-option-as-alt", State::ime_option_as_alt_scene),
        ],
        ..AppSpec::new("Rich Clipboard", (560.0, 620.0), State::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn a_rich_paste_shows_its_forms_and_the_field_keeps_the_text() {
        let board = clipboard::memory();
        assert_eq!(board.revision(), 0);
        let mut rt = Runtime::new(State::default(), view, (560.0, 620.0), Env::new(true, 2.0));
        rt.use_clipboard(&board);
        rt.frame(0.0);
        assert!(rt.find("revision").is_some());
        assert!(rt.accessibility_json().to_string().contains("Rich text editor"));
        rt.update(|s, _| s.field.focused = true);
        rt.frame(0.0);
        board.set_content(ClipboardContent::new("Hi").with_html("<b>Hi</b><img src=x onerror=alert(1)>"));
        assert_eq!(board.revision(), 1);
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
        assert_eq!(rt.state.field.text, "Hi");
        let (how, content) = rt.state.last.clone().unwrap();
        assert_eq!((how, content.html()), ("Pasted", Some(r#"<b>Hi</b><img src="x">"#)));

        let (x, y) = rt.find("copy").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(board.content(), Some(sample()));
        assert_eq!(board.revision(), 2);

        let (x, y) = rt.find("check-revision").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.revision, clipboard::revision());
        assert_eq!(revision_status(Some(7)), "Clipboard revision: 7");
        assert_eq!(revision_status(None), "Clipboard revision: unavailable");

        // Selecting text and pressing a colour swatch applies Mark::Color and updates to_html().
        rt.update(|s, _| s.field.set_selection(0, 2));
        rt.frame(0.0);
        let (rx, ry) = rt.find("color-red").unwrap();
        rt.pointer_down(rx, ry);
        rt.pointer_up(rx, ry);
        rt.frame(0.0);
        assert_eq!(rt.state.field.active_color(), Some("#e05252"));
        assert!(rt.state.field.to_html().contains("<span style=\"#e05252\">") || rt.state.field.to_html().contains("<span style=\"color:#e05252\">"));
    }

    #[test]
    fn floating_selection_bar_and_byte_offset_hit_test() {
        let mut rt = Runtime::new(State::default(), view, (560.0, 620.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(rt.find("selection-bar").is_none(), "hidden without a selection");

        // Select text in the field: the floating bar mounts over the selection's line boxes.
        rt.update(|s, tx| {
            s.field = TextEdit::new("Rune copies bold, italic and coloured text.").rich();
            s.field.focused = true;
            s.field.set_selection(12, 24);
            tx.instant();
        });
        rt.frame(0.0);
        assert!(rt.find("selection-bar").is_some(), "floating bar appears for selection");

        // Pressing Bold in the floating bar keeps focus and applies Bold to the selection.
        let (bx, by) = rt.find("float-bold").unwrap();
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.field.focused);
        assert!(rt.state.field.to_html().contains("<strong>bold, italic</strong>"), "{}", rt.state.field.to_html());

        // Right-clicking the field resolves the UTF-8 byte offset via Tx::text_offset_at.
        let (fx, fy) = rt.find("field").unwrap();
        rt.secondary_down(fx, fy);
        rt.secondary_up(fx, fy);
        rt.frame(0.0);
        let (target, offset) = rt.state.hit.expect("field secondary click should hit-test offset");
        assert_eq!(target, "field");
        assert!(rt.state.field.text.is_char_boundary(offset));

        // Clicking the read-only sample text resolves its UTF-8 byte offset too.
        let (sx, sy) = rt.find("sample").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        rt.frame(0.0);
        let (target, offset) = rt.state.hit.expect("sample click should hit-test offset");
        assert_eq!(target, "sample");
        assert!(SAMPLE_TEXT.is_char_boundary(offset));
    }

    #[test]
    fn decorations_scene_renders_pill_logos_rings_and_markdown_line_fills() {
        let mut s = State::default();
        s.decorations_scene();
        let mut rt = Runtime::new(s, view, (560.0, 620.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.find("markdown-field").is_some());
        assert!(rt.find("static-pills").is_some());
        assert!(rt.find("filter-chips").is_some());
        assert!(rt.find("overline-sample").is_some());
        let fills = markdown_line_fills(&rt.state.notes.text, Color::WHITE, Color::WHITE);
        assert_eq!(fills.len(), 2, "markdown sample produces one code block fill and one quote bar");
    }

    #[test]
    fn pill_copy_scene_exports_url_payload_and_round_trips_own_pills() {
        let board = clipboard::memory();
        let mut s = State::default();
        s.pill_copy_scene();
        let mut rt = Runtime::new(s, view, (560.0, 620.0), Env::new(true, 1.0));
        rt.use_clipboard(&board);
        rt.frame(0.0);
        assert!(rt.find("copy-status").is_some());

        rt.key(rune::KeyEvent {
            key: rune::Key::Command('c'),
            mods: rune::Mods { cmd: true, ..rune::Mods::default() },
        });
        rt.frame(0.0);

        let expected_plain = "Open https://mail.google.com or https://github.com now.";
        let copied = board.content().unwrap();
        assert_eq!(copied.text(), expected_plain);
        assert!(copied.html().is_some_and(|h| h.contains(r#"<span data-pill="app:gmail">@Gmail</span>"#)));
        assert_eq!(rt.state.last_copy, Some((false, expected_plain.to_owned())));

        // Paste back into the field: has_own_pills() is true and both mention pills round-trip.
        rt.update(|s, _| {
            s.field.clear();
            s.field.focused = true;
        });
        rt.frame(0.0);
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
        let (_, pasted) = rt.state.last.as_ref().unwrap();
        assert!(pasted.has_own_pills());
        assert_eq!(
            rt.state.field.pills().map(|(_, id)| id).collect::<Vec<_>>(),
            ["app:gmail", "app:github"]
        );
    }

    #[test]
    fn pill_press_inspects_mention_and_input_selection_updates_readout() {
        let mut s = State::default();
        s.decorations_scene();
        s.field.set_caret(0);
        let mut rt = Runtime::new(s, view, (560.0, 680.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.find("pill-status").is_some());
        assert!(rt.find("selection-status").is_some());

        // Focus field and arrow right: on_input_selection updates readout in the same frame.
        rt.update(|s, _| s.field.focused = true);
        rt.frame(1.0 / 60.0);
        rt.key(rune::KeyEvent::from(rune::Key::Right));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selection_readout, (1, 1));
        assert_eq!(rt.state.selection_events, 1);

        // Scan across the first line of the field to locate the @Gmail pill (Cursor::Pointer).
        let fr = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).unwrap().rect;
        let py = fr[1] + 16.0;
        let mut pill_pt = None;
        for step in 0..80 {
            let px = fr[0] + 8.0 + step as f32 * 4.0;
            rt.pointer_move(px, py);
            if rt.cursor() == rune::Cursor::Pointer {
                pill_pt = Some((px, py));
                break;
            }
        }
        let (px, py) = pill_pt.expect("expected Cursor::Pointer over @Gmail pill");
        let sel_before = (rt.state.field.anchor, rt.state.field.focus);
        rt.pointer_down(px, py);
        rt.pointer_up(px, py);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.inspected_pill.as_ref().map(|(id, _)| id.as_str()), Some("app:gmail"));
        assert_eq!((rt.state.field.anchor, rt.state.field.focus), sel_before, "consuming pill press keeps caret still");
    }

    #[test]
    fn ime_option_as_alt_scene_selects_option_keys_and_mounts_caret() {
        let mut s = State::default();
        s.ime_option_as_alt_scene();
        let mut rt = Runtime::new(s, view, (560.0, 680.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.find("custom-ime-caret").is_some());
        assert!(rt.ime_area.is_some());
        assert_eq!(rt.state.option_as_alt, OptionAsAlt::Left);

        let tree = rt.accessibility_json().to_string();
        assert!(tree.contains("macOS Option-as-Alt"));
        assert!(tree.contains("Option as Alt: Left"));

        for (key, expected) in [
            ("opt-right", OptionAsAlt::Right),
            ("opt-both", OptionAsAlt::Both),
            ("opt-none", OptionAsAlt::None),
            ("opt-left", OptionAsAlt::Left),
        ] {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(0.0);
            assert_eq!(rt.state.option_as_alt, expected);
            assert!(rt.state.custom_caret);
        }
    }
}
