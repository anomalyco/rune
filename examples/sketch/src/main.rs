//! A pencil over a canvas: `Cursor::Image` with a 2x PNG, its hot spot on the tip. Drag to
//! draw. `cargo run -p sketch`. `scripts/pencil.py` draws `assets/pencil.png`.

#![forbid(unsafe_code)]

use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, Color, Cursor, CursorImage, Cx, El, LineCap, Phase, Shape, canvas, col, text};

/// 64×64 px, so 32×32 pt, with the graphite tip at (2, 30) pt.
const PENCIL: CursorImage = CursorImage::png(include_bytes!("../assets/pencil.png"), [2.0, 30.0]);
const INK: Color = Color { r: 0.11, g: 0.11, b: 0.12, a: 1.0 };

#[derive(Default)]
struct State {
    strokes: Vec<Vec<[f32; 2]>>,
}

fn drawing(s: &State) -> Shape {
    s.strokes.iter().fold(Shape::new(), |shape, stroke| match stroke.as_slice() {
        [p] => shape.line(*p, *p, 2.5, LineCap::Round, INK),
        points => shape.stroke(points.to_vec(), 2.5, INK),
    })
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(false);
    col().w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg)
        .child(text("Drag to draw").font(type_::BODY).color(t.text_secondary))
        .child(canvas(drawing(s)).key("paper").w_full().grow(1.0).radius(radius::LG)
            .fill(Color::WHITE).border(1.0, t.card_border)
            .cursor(Cursor::Image(PENCIL))
            .on_drag(|s: &mut State, d, _| match d.phase {
                Phase::Down => s.strokes.push(vec![[d.x, d.y]]),
                _ => if let Some(stroke) = s.strokes.last_mut() { stroke.push([d.x, d.y]) },
            }))
}

fn main() {
    rune::run(AppSpec::new("Sketch", (560.0, 420.0), State::default, view));
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
}
