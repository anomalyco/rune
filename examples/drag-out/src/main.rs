//! Drag a file, an app bundle or a line of text out of a Rune window into Finder, System
//! Settings or a text field. `cargo run -p drag-out`; set `DRAG_OUT_APP` to drag a different
//! app bundle. A tile dims while it's dragged; each drag's end prints
//! `drag-end <tile> <outcome>`.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, Cursor, Cx, DragItem, DragOutcome, El, col, row, text};

struct State {
    file: PathBuf,
    app: PathBuf,
    last: Option<(&'static str, DragOutcome)>,
    dragging: Option<&'static str>,
}

fn tile(s: &State, key: &'static str, title: &str, detail: String, item: fn(&State) -> DragItem) -> El {
    let t = tok::theme(true);
    col().key(key).role("button").size(150.0, 92.0).padding(space::S3).gap(space::S1).radius(radius::LG)
        .fill(t.card).border(1.0, t.card_border).hover(t.hover).press(t.press).cursor(Cursor::Grab)
        .opacity(if s.dragging == Some(key) { 0.28 } else { 1.0 })
        .child(text(title).font(type_::TITLE).color(t.text))
        .child(text(detail).font(type_::LABEL).color(t.text_secondary))
        .drag_out(move |s: &State| Some(item(s).opaque(tok::theme(true).bg)))
        .on_drag_start(move |s: &mut State, _| s.dragging = Some(key))
        .on_drag_end(move |s: &mut State, outcome, _| {
            println!("drag-end {key} {outcome:?}");
            s.dragging = None;
            s.last = Some((key, outcome));
        })
}

fn name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let status = match s.last {
        Some((key, outcome)) => format!("Last drag: {key}, {outcome:?}"),
        None => "Drag a tile out of the window".into(),
    };
    col().w_full().h_full().center().gap(space::S6).fill(t.bg)
        .child(row().gap(space::S3)
            .child(tile(s, "file", "A file", name(&s.file), |s| DragItem::new().file(&s.file)))
            .child(tile(s, "app", "An app", name(&s.app), |s| DragItem::new().file(&s.app)))
            .child(tile(s, "text", "Some text", "Hello from Rune".into(), |_| DragItem::new().text("Hello from Rune"))))
        .child(text(status).key("status").font(type_::BODY).color(t.text_secondary))
}

fn initial() -> State {
    let file = std::env::temp_dir().join("Dragged out of Rune.txt");
    std::fs::write(&file, "This file was dragged out of a Rune window.\n").ok();
    let app = std::env::var_os("DRAG_OUT_APP").map_or_else(|| PathBuf::from("/System/Applications/Calculator.app"), PathBuf::from);
    State { file, app, last: None, dragging: None }
}

fn main() {
    rune::run(AppSpec::new("Drag Out", (520.0, 200.0), initial, view));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn dragging_a_tile_hands_its_file_to_the_system() {
        let state = State { file: "/tmp/a.txt".into(), app: "/Applications/Example.app".into(), last: None, dragging: None };
        let mut rt = Runtime::new(state, view, (520.0, 200.0), Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);
        let (x, y) = rt.find("app").unwrap();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_move(x + 20.0, y);
        assert!(rt.events.iter().any(|e| e.starts_with("drag-out ") && e.ends_with("app")), "{:?}", rt.events);
        rt.frame(0.0);
        // No system took the drag here, so it ends Cancelled in the same frame it started.
        assert_eq!((rt.state.dragging, rt.state.last), (None, Some(("app", DragOutcome::Cancelled))));
    }
}
