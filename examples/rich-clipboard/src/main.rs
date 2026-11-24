//! Copy and paste formatted text, inspect the clipboard revision counter, position a floating
//! formatting bar over `Cx::field_selection_rects`, and hit-test byte offsets with
//! `Tx::text_offset_at`.
//! `cargo run -p rich-clipboard` (or `-- --scene selected --tree`).
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
use rune::{AppSpec, ClipboardContent, Cx, El, Mark, Phase, TextEdit, Tx, col, el, row, text, text_input};

const SAMPLE_TEXT: &str = "Click here or right-click the field to hit-test a UTF-8 byte offset.";

const SWATCHES: &[(&str, &str, &str)] = &[
    ("color-red", "Red text colour", "#e05252"),
    ("color-amber", "Amber text colour", "#e09f3e"),
    ("color-green", "Green text colour", "#3fb950"),
    ("color-blue", "Blue text colour", "#3b82f6"),
];

struct State {
    field: TextEdit,
    /// Whether the field marks misspelled words and shows suggestions in its menu.
    spellcheck: bool,
    /// How the last paste or read arrived, and what it held.
    last: Option<(&'static str, ClipboardContent)>,
    /// Last observed OS clipboard revision counter (`None` where unsupported).
    revision: Option<u64>,
    /// Last hit-tested target and UTF-8 byte offset from `Tx::text_offset_at`.
    hit: Option<(&'static str, usize)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            field: TextEdit::new("").rich(),
            spellcheck: true,
            last: None,
            revision: clipboard::revision(),
            hit: None,
        }
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
    let head = format!("{how}: text {} B, {}, {}", content.text().len(), form("HTML", content.html().map(str::len)), form("RTF", content.rtf().map(str::len)));
    let preview: String = content.html().unwrap_or("").chars().take(400).collect();
    (head, preview)
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
    toolbar = toolbar.child(button("color-clear", "Reset colour", |s, _| s.field.set_color(None)));

    let field = text_input(&s.field, type_::BODY).key("field").label("Rich text editor").w_full().min_h(80.0).padding(space::S2)
        .radius(radius::MD).fill(t.card).border(1.0, t.card_border).color(t.text)
        .spellcheck(s.spellcheck)
        .on_edit(|s: &mut State| &mut s.field)
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
    let html_out = s.field.to_html();
    let mut root = col().w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg)
        .on_spellcheck(s.spellcheck, |s: &mut State, on, _| s.spellcheck = on)
        .on_spellcheck_languages(|_: &mut State, _, _| {})
        .child(toolbar)
        .child(field)
        .child(sample_line)
        .child(text(hit_status(s)).key("hit-offset").font(type_::LABEL).color(t.text_secondary))
        .child(row().gap(space::S2)
            .child(button("copy", "Copy formatted", |_, tx| tx.copy_rich(sample())))
            .child(button("read", "Read clipboard", |_, tx| tx.read_clipboard(|s: &mut State, content, _| seen(s, "Read", content))))
            .child(button("check-revision", "Check revision", |s, _| s.revision = clipboard::revision())))
        .child(text(revision_status(s.revision)).key("revision").font(type_::LABEL).color(t.text_secondary))
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
        scenes: &[("selected", |s| {
            s.field = TextEdit::new("Rune copies bold, italic and coloured text.").rich();
            s.field.focused = true;
            s.field.set_selection(12, 24);
        })],
        ..AppSpec::new("Rich Clipboard", (560.0, 480.0), State::default, view)
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
        let mut rt = Runtime::new(State::default(), view, (560.0, 480.0), Env::new(true, 2.0));
        rt.use_clipboard(&board);
        rt.frame(0.0);
        assert!(rt.find("revision").is_some());
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
        let mut rt = Runtime::new(State::default(), view, (560.0, 480.0), Env::new(true, 2.0));
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
}
