//! Copy and paste formatted text. `cargo run -p rich-clipboard`.
//!
//! Paste into the field from a browser, Notes, TextEdit or Pages: the field still pastes the
//! plain text, and the panel below shows which forms arrived and the start of the sanitised
//! HTML (on macOS, RTF-only sources arrive converted to HTML). Each paste also prints its
//! HTML. "Copy formatted" puts plain text, HTML and RTF on the clipboard: paste it into
//! TextEdit or Mail and it keeps its bold and italic. "Read clipboard" looks without pasting.

#![forbid(unsafe_code)]

use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, ClipboardContent, Cx, El, TextEdit, Tx, col, row, text, text_input};

#[derive(Default)]
struct State {
    field: TextEdit,
    /// How the last paste or read arrived, and what it held.
    last: Option<(&'static str, ClipboardContent)>,
}

fn sample() -> ClipboardContent {
    ClipboardContent::new("Rune copies bold and italic text.")
        .with_html("Rune copies <b>bold</b> and <i>italic</i> text.")
        .with_rtf(r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}\f0 Rune copies {\b bold} and {\i italic} text.}")
}

fn seen(s: &mut State, how: &'static str, content: &ClipboardContent) {
    println!("{how}: text {} bytes, html {:?} bytes, rtf {:?} bytes", content.text().len(), content.html().map(str::len), content.rtf().map(str::len));
    if let Some(html) = content.html() { println!("{html}"); }
    s.last = Some((how, content.clone()));
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

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let (head, preview) = summary(&s.last);
    let field = text_input(&s.field, type_::BODY).key("field").w_full().min_h(80.0).padding(space::S2)
        .radius(radius::MD).fill(t.card).border(1.0, t.card_border).color(t.text)
        .on_edit(|s: &mut State| &mut s.field)
        // Look at every form, then let the editor paste the text as usual.
        .on_input_paste(|s: &mut State, content, _| { seen(s, "Pasted", content); false });
    col().w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg)
        .child(field)
        .child(row().gap(space::S2)
            .child(button("copy", "Copy formatted", |_, tx| tx.copy_rich(sample())))
            .child(button("read", "Read clipboard", |_, tx| tx.read_clipboard(|s: &mut State, content, _| seen(s, "Read", content)))))
        .child(text(head).key("summary").font(type_::BODY).color(t.text_secondary))
        .child(text(preview).key("preview").font(type_::LABEL).color(t.text_secondary).w_full())
}

fn main() {
    rune::run(AppSpec::new("Rich Clipboard", (560.0, 420.0), State::default, view));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime, clipboard};

    #[test]
    fn a_rich_paste_shows_its_forms_and_the_field_keeps_the_text() {
        let board = clipboard::memory();
        let mut rt = Runtime::new(State::default(), view, (560.0, 420.0), Env::new(true, 2.0));
        rt.use_clipboard(&board);
        rt.frame(0.0);
        rt.update(|s, _| s.field.focused = true);
        rt.frame(0.0);
        board.set_content(ClipboardContent::new("Hi").with_html("<b>Hi</b><img src=x onerror=alert(1)>"));
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
    }
}
