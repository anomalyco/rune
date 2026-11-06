//! Code examples cut from Rune's own source when the page is built, so an example can't drift
//! from the code it shows. A piece starts at the first line beginning with `from` (attributes
//! above it come along) and ends at `to`, just before `until`, or at the closing brace at the
//! starting line's indent. A marker that stops matching fails the tests.

use rune::{El, Span, col, rich, row, text};
use rune_kit::syntax::{Paint, Syntax, highlight};

use crate::REPO;
use crate::tokens::*;
use crate::ui::hairline;

const HELLO: &str = include_str!("../../../examples/hello/src/main.rs");
const SPRING_TESTS: &str = include_str!("../../../crates/rune-motion/tests/spring.rs");

/// Where a snippet ends.
#[derive(Clone, Copy)]
pub enum End {
    /// Just before the first later line starting with this, trailing blank lines dropped.
    Until(&'static str),
    /// At the first line from the start that starts with this.
    To(&'static str),
    /// At the closing brace at the first line's indent.
    Brace,
}

pub struct Snippet {
    pub file: &'static str,
    source: &'static str,
    from: &'static str,
    end: End,
}

pub const HELLO_VIEW: Snippet = Snippet { file: "examples/hello/src/main.rs", source: HELLO, from: "fn view(", end: End::Until("fn main()") };
pub const HELLO_TEST: Snippet = Snippet { file: "examples/hello/src/main.rs", source: HELLO, from: "    fn a_tap_flips_the_switch", end: End::Brace };
pub const RETARGET: Snippet = Snippet {
    file: "crates/rune-motion/tests/spring.rs",
    source: SPRING_TESTS,
    from: "fn animated_value_retargets_keeping_velocity",
    end: End::To("    assert!((a.velocity(0.1) - v1).abs()"),
};

/// A cut: the code and the 1-based lines it came from.
#[derive(Debug, PartialEq)]
pub struct Cut {
    pub code: String,
    pub start: usize,
    pub end: usize,
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

impl Snippet {
    pub fn cut(&self) -> Option<Cut> {
        let lines: Vec<&str> = self.source.trim_end_matches('\n').split('\n').collect();
        let first = lines.iter().position(|l| l.starts_with(self.from))?;
        let mut start = first;
        while start > 0 && lines[start - 1].trim().starts_with("#[") {
            start -= 1;
        }
        let end = match self.end {
            End::Until(until) => {
                let mut end = (first + 1..lines.len()).find(|&i| lines[i].starts_with(until))?.checked_sub(1)?;
                while end > first && lines[end].trim().is_empty() {
                    end -= 1;
                }
                end
            }
            End::To(to) => (first..lines.len()).find(|&i| lines[i].starts_with(to))?,
            End::Brace => {
                let close = format!("{}}}", &lines[first][..indent(lines[first])]);
                (first + 1..lines.len()).find(|&i| lines[i].trim_end() == close)?
            }
        };
        let taken = &lines[start..=end];
        let pad = taken.iter().filter(|l| !l.trim().is_empty()).map(|l| indent(l)).min().unwrap_or(0);
        let mut out: Vec<String> = taken.iter().map(|l| l.get(pad..).unwrap_or("").to_owned()).collect();
        // A block cut short keeps its shape: an elision inside, then its closing brace.
        let joined = out.join("\n");
        if joined.matches('{').count() > joined.matches('}').count() {
            let lead = " ".repeat(indent(lines[first]).saturating_sub(pad));
            out.push(format!("{lead}    // …"));
            out.push(format!("{lead}}}"));
        }
        Some(Cut { code: out.join("\n"), start: start + 1, end: end + 1 })
    }
}

fn paint(kind: Syntax) -> rune::Color {
    match kind {
        Syntax::Keyword => code_keyword(),
        Syntax::String | Syntax::Number => code_string(),
        Syntax::Function => code_call(),
        Syntax::Comment => fg3(),
        Syntax::Punctuation => fg2(),
        _ => fg(),
    }
}

/// `code` as Rust, coloured like the rest of the page's code.
pub fn colored(code: &str) -> Vec<Span> {
    highlight(Some("rust"), code)
        .iter()
        .map(|(range, p)| {
            let color = match p {
                Paint::Kind(k) => paint(*k),
                Paint::Color(c) => *c,
            };
            Span::new(range.start, range.end).color(color)
        })
        .collect()
}

/// A code block: the file and lines it came from, linked to GitHub, over the code.
pub fn block(key: &str, s: &Snippet) -> El {
    let Some(cut) = s.cut() else { return col().key(key) };
    let href = format!("{REPO}/blob/dev/{}#L{}-L{}", s.file, cut.start, cut.end);
    let file = format!("{}:{}–{}", s.file, cut.start, cut.end);
    let lines_from = s.file.len();
    let head = row().key("head").w_full().justify_between().gap(16.0).px(16.0).py(8.0)
        .child(row().key("source").href(href).shrink().hover_group()
            .child(rich(file.clone(), vec![Span::new(0, lines_from).color(fg2()), Span::new(lines_from, file.len()).color(fg3())])
                .font(MONO_12).color(fg2()).wrap_anywhere().shrink().group_color(fg())))
        .child(text("Rust").font(MONO_12).color(fg3()));
    let body = col().key("scroll").w_full().scroll_x()
        .child(rich(cut.code.clone(), colored(&cut.code)).key("code").font(CODE).color(fg()).selectable(true).padding(16.0));
    col().key(key).role("figure").w_full().min_w(0.0).radius(R2).fill(surface1()).border(1.0, line()).clip_rounded(R2)
        .child(head)
        .child(hairline("rule"))
        .child(body)
}
