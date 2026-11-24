//! A pencil over a canvas (`Cursor::Image` with a 2x PNG, hot spot on the tip) plus corner and
//! edge resize handles (`Cursor::ResizeNwse`, `Cursor::ResizeNesw`, `Cursor::ResizeEw`,
//! `Cursor::ResizeNs`). Drag to draw; hover the handle strip or open `--scene handles` to try
//! the resize cursors. `cargo run -p sketch`. `scripts/pencil.py` draws `assets/pencil.png`.

#![forbid(unsafe_code)]

use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, Color, Cursor, CursorImage, Cx, El, LineCap, Phase, Shape, canvas, col, el, row, text};

/// 64×64 px, so 32×32 pt, with the graphite tip at (2, 30) pt.
const PENCIL: CursorImage = CursorImage::png(include_bytes!("../assets/pencil.png"), [2.0, 30.0]);
const INK: Color = Color { r: 0.11, g: 0.11, b: 0.12, a: 1.0 };

#[derive(Default)]
struct State {
    strokes: Vec<Vec<[f32; 2]>>,
    show_handles: bool,
}

fn drawing(s: &State) -> Shape {
    s.strokes.iter().fold(Shape::new(), |shape, stroke| match stroke.as_slice() {
        [p] => shape.line(*p, *p, 2.5, LineCap::Round, INK),
        points => shape.stroke(points.to_vec(), 2.5, INK),
    })
}

fn handle_chip(key: &'static str, label: &'static str, cursor: Cursor, t: &tok::Theme) -> El {
    el().key(key).px(space::S2).h(26.0).center().radius(radius::SM)
        .fill(t.card).border(1.0, t.card_border).hover(t.hover).hit().cursor(cursor)
        .child(text(label).font(type_::LABEL).color(t.text_secondary))
}

fn crop_handles(t: &tok::Theme) -> El {
    let dot = |key: &'static str, x: f32, y: f32, cursor: Cursor| {
        el().key(key).size(14.0, 14.0).radius(3.0).fill(Color::WHITE).border(1.5, INK)
            .hover(t.hover).hit().cursor(cursor).absolute(x, y)
    };
    let (w, h) = (260.0, 170.0);
    el().w_full().grow(1.0).center().radius(radius::LG).fill(Color::WHITE).border(1.0, t.card_border)
        .child(el().size(w + 14.0, h + 14.0)
            .child(el().size(w, h).border(1.0, INK).absolute(7.0, 7.0).center()
                .child(text("Hover corners and edges").font(type_::BODY).color(t.text_secondary)))
            .child(dot("crop-nw", 0.0, 0.0, Cursor::ResizeNwse))
            .child(dot("crop-n", w * 0.5, 0.0, Cursor::ResizeNs))
            .child(dot("crop-ne", w, 0.0, Cursor::ResizeNesw))
            .child(dot("crop-e", w, h * 0.5, Cursor::ResizeEw))
            .child(dot("crop-se", w, h, Cursor::ResizeNwse))
            .child(dot("crop-s", w * 0.5, h, Cursor::ResizeNs))
            .child(dot("crop-sw", 0.0, h, Cursor::ResizeNesw))
            .child(dot("crop-w", 0.0, h * 0.5, Cursor::ResizeEw)))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(false);
    let toggle = el().key("toggle-handles").role("button").label("Toggle resize handles")
        .px(space::S2).h(26.0).center().radius(radius::SM)
        .fill(t.card).border(1.0, t.card_border).hover(t.hover).focusable(true).cursor(Cursor::Pointer)
        .on_press(|s: &mut State| s.show_handles = !s.show_handles)
        .child(text(if s.show_handles { "Draw" } else { "Crop handles" }).font(type_::LABEL).color(t.text));
    let strip = row().gap(space::S2).items_center()
        .child(handle_chip("handle-nwse", "NW–SE", Cursor::ResizeNwse, &t))
        .child(handle_chip("handle-nesw", "NE–SW", Cursor::ResizeNesw, &t))
        .child(handle_chip("handle-ew", "E–W", Cursor::ResizeEw, &t))
        .child(handle_chip("handle-ns", "N–S", Cursor::ResizeNs, &t))
        .child(toggle);
    let header = row().w_full().items_center().gap(space::S3)
        .child(text(if s.show_handles { "Resize handles" } else { "Drag to draw" })
            .font(type_::BODY).color(t.text_secondary).grow(1.0))
        .child(strip);
    let stage = if s.show_handles {
        crop_handles(&t)
    } else {
        canvas(drawing(s)).key("paper").w_full().grow(1.0).radius(radius::LG)
            .fill(Color::WHITE).border(1.0, t.card_border)
            .cursor(Cursor::Image(PENCIL))
            .on_drag(|s: &mut State, d, _| match d.phase {
                Phase::Down => s.strokes.push(vec![[d.x, d.y]]),
                _ => if let Some(stroke) = s.strokes.last_mut() { stroke.push([d.x, d.y]) },
            })
    };
    col().w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg).child(header).child(stage)
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("handles", |s| s.show_handles = true)],
        ..AppSpec::new("Sketch", (560.0, 420.0), State::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn the_paper_shows_the_pencil_and_takes_strokes() {
        let mut rt = Runtime::new(State::default(), view, (560.0, 420.0), Env { system_dark: false, scale: 2.0 });
        rt.frame(0.0);
        let (x, y) = rt.find("paper").unwrap();
        rt.pointer_move(x, y);
        assert_eq!(rt.cursor(), Cursor::Image(PENCIL));
        rt.pointer_down(x, y);
        rt.pointer_move(x + 30.0, y + 10.0);
        rt.pointer_up(x + 30.0, y + 10.0);
        rt.frame(0.0);
        assert_eq!(rt.state.strokes.len(), 1);
        assert!(rt.state.strokes[0].len() >= 2, "{:?}", rt.state.strokes);
    }

    #[test]
    fn resize_handles_show_diagonal_and_bidirectional_cursors() {
        let mut rt = Runtime::new(State::default(), view, (560.0, 420.0), Env { system_dark: false, scale: 2.0 });
        rt.frame(0.0);
        for (key, expected) in [
            ("handle-nwse", Cursor::ResizeNwse),
            ("handle-nesw", Cursor::ResizeNesw),
            ("handle-ew", Cursor::ResizeEw),
            ("handle-ns", Cursor::ResizeNs),
        ] {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_move(x, y);
            assert_eq!(rt.cursor(), expected, "{key}");
        }
        let (tx, ty) = rt.find("toggle-handles").unwrap();
        rt.pointer_down(tx, ty);
        rt.pointer_up(tx, ty);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        for (key, expected) in [
            ("crop-nw", Cursor::ResizeNwse),
            ("crop-se", Cursor::ResizeNwse),
            ("crop-ne", Cursor::ResizeNesw),
            ("crop-sw", Cursor::ResizeNesw),
            ("crop-e", Cursor::ResizeEw),
            ("crop-w", Cursor::ResizeEw),
            ("crop-n", Cursor::ResizeNs),
            ("crop-s", Cursor::ResizeNs),
        ] {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_move(x, y);
            assert_eq!(rt.cursor(), expected, "{key}");
        }
    }
}
