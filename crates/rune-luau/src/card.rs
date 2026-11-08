//! The card a surface shows in place of a render that failed: the error's first line, its
//! traceback, and Reload, which runs the script again from its current sources.

use std::rc::Weak;

use rune::{Color, El, TextStyle, col, row, text};

use crate::script::Inner;
use crate::ui::{Attach, Palette};

pub(crate) fn error_card(message: &str, palette: &Palette, dark: bool, attach: &'static Attach, script: Weak<Inner>) -> El {
    let color = |name: &str, fallback: Color| palette(name, dark).unwrap_or(fallback);
    let fg = color("text", if dark { Color::WHITE } else { Color::BLACK });
    let weak = color("text-secondary", fg.alpha(0.7));
    let danger = rune::color::hex("#e5484d").unwrap_or(fg);
    let mut lines = message.lines();
    let first = lines.next().unwrap_or("script error");
    let rest: Vec<&str> = lines.filter(|l| !l.trim().is_empty()).take(12).collect();
    let reload = row()
        .key("luau-reload")
        .px(10.0)
        .py(4.0)
        .radius(6.0)
        .fill(color("control-off", weak.alpha(0.2)))
        .hover(color("hover", weak.alpha(0.1)))
        .child(text("Reload").font(TextStyle::new(12.0, 500.0)).color(fg))
        .label("Reload script");
    let mut card = col()
        .key("luau-error")
        .w_full()
        .padding(12.0)
        .gap(8.0)
        .radius(8.0)
        .fill(color("card", Color::TRANSPARENT))
        .border(1.0, danger)
        .child(text(format!("Stopped drawing: {first}")).font(TextStyle::new(13.0, 500.0)).color(fg).wrap().w_full());
    if !rest.is_empty() {
        card = card.child(text(rest.join("\n")).font(TextStyle::new(11.0, 400.0).mono()).color(weak).wrap().w_full());
    }
    card.child(row().child((attach.reload)(reload, script)))
}

/// What a row shows when `renderRow` fails.
pub(crate) fn row_error(message: &str) -> El {
    let danger = rune::color::hex("#e5484d").unwrap_or(Color::WHITE);
    text(message.lines().next().unwrap_or("row failed").to_owned()).font(TextStyle::new(11.0, 400.0).mono()).color(danger).truncate().w_full()
}
