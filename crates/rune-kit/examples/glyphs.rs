//! Every built-in glyph at 16 pt, labelled, light beside dark. The still `demo/kit/glyphs.png`:
//! `-- --snap demo/kit/glyphs.png --size 1000x540 --scale 2`.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use rune::{AppSpec, Cx, El, Icon, col, icon, row, text};
use rune_kit::{Kit, glyphs};

const ALL: [&Icon; 26] = [
    &glyphs::CHECK,
    &glyphs::CHECK_SMALL,
    &glyphs::CHEVRON_DOWN,
    &glyphs::CHEVRON_UP,
    &glyphs::CARET_RIGHT,
    &glyphs::TRIANGLE_DOWN,
    &glyphs::TRIANGLE_RIGHT,
    &glyphs::CLOSE,
    &glyphs::COPY,
    &glyphs::SEARCH,
    &glyphs::CIRCLE_X,
    &glyphs::LOCK,
    &glyphs::EYE,
    &glyphs::EYE_OFF,
    &glyphs::DASH,
    &glyphs::FOLDER,
    &glyphs::FOLDER_OPEN,
    &glyphs::FILE,
    &glyphs::FILE_CODE,
    &glyphs::FILE_TEXT,
    &glyphs::FILE_IMAGE,
    &glyphs::MIC,
    &glyphs::KEYBOARD,
    &glyphs::CURSOR_TEXT,
    &glyphs::ARROW_RIGHT,
    &glyphs::SPARKLE,
];

fn panel(dark: bool) -> El {
    let k = Kit::neutral(dark);
    let tile = |i: &'static Icon| {
        col().w(96.0).h(64.0).gap(8.0).items_center().justify_center()
            .child(icon(i, 16.0).color(k.c.text_base))
            .child(text(i.name.trim_start_matches("kit-")).font(k.ty.text_xsmall).color(k.c.text_faint))
    };
    let rows = ALL.chunks(5).fold(col().gap(0.0), |c, r| c.child(r.iter().fold(row(), |row, &i| row.child(tile(i)))));
    col().grow(1.0).h_full().padding(10.0).fill(k.c.page).items_center().justify_center().child(rows)
}

fn view(_: &(), _: &mut Cx<()>) -> El {
    row().w_full().h_full().child(panel(false)).child(panel(true))
}

fn main() {
    rune::run(AppSpec::new("Glyphs", (1000.0, 540.0), || (), view));
}
