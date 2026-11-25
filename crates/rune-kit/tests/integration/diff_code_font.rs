//! `DiffView::code_font`: code lines are the font's line height tall.

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Runtime, TextStyle, col};
use rune_kit::Kit;
use rune_kit::diff::{DiffMode, DiffState, DiffView, FileDiff};

struct App {
    diff: DiffState,
    font: Option<TextStyle>,
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    let mut diff = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff);
    if let Some(font) = s.font {
        diff = diff.code_font(font);
    }
    col().w_full().h_full().child(diff.build(s, &k))
}

fn find(rt: &Runtime<App>, end: &str) -> ElementInfo {
    let suffix = format!("/{end}");
    rt.elements().into_iter().find(|e| e.path.ends_with(&suffix) && !e.exiting).unwrap_or_else(|| panic!("no {end}"))
}

fn line_height(mode: DiffMode, font: Option<TextStyle>) -> f32 {
    let mut diff = DiffState::new(vec![FileDiff::from_texts("a.rs", "a\nb\nc\n", "a\nx\nc\n")]);
    diff.set_mode(mode);
    let mut rt = Runtime::new(App { diff, font }, view, (800.0, 400.0), Env::new(true, 2.0));
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    find(&rt, "df0.c1").rect[3]
}

#[test]
fn code_font_sets_the_row_height() {
    let mono = Kit::neutral(true).ty.mono;
    for mode in [DiffMode::Unified, DiffMode::Split] {
        assert_eq!(line_height(mode, None), 20.0, "default rows are 20 pt");
        let tall = TextStyle { size: 12.0, line_height: 28.0, ..mono };
        assert_eq!(line_height(mode, Some(tall)), 28.0, "rows follow the font's line height");
        let auto = TextStyle { size: 16.0, line_height: 0.0, ..mono };
        assert_eq!(line_height(mode, Some(auto)), 20.0, "a 0 line height is 1.25 × size");
    }
}
