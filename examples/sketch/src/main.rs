//! A pencil over a canvas (`Cursor::Image` with a 2x PNG, hot spot on the tip), right-button
//! panning (`El::pointer_buttons([PointerButton::Secondary])` keeping right-click for the context
//! menu), extra mouse button actions (`El::on_button`: Back undoes, Forward redoes, Middle resets
//! pan), and corner/edge resize handles (`Cursor::ResizeNwse`, `Cursor::ResizeNesw`,
//! `Cursor::ResizeEw`, `Cursor::ResizeNs`). Drag to draw; right-drag to pan; right-click for a
//! menu; open `--scene handles` for resize cursors or `--scene panned` for a panned canvas.
//! `cargo run -p sketch`. `scripts/pencil.py` draws `assets/pencil.png`.

#![forbid(unsafe_code)]

use rune::tok::{self, radius, space, type_};
use rune::{
    AppSpec, Color, Cursor, CursorImage, Cx, El, LineCap, Phase, PointerButton, PointerPress,
    Shape, canvas, col, el, row, text,
};

/// 64×64 px, so 32×32 pt, with the graphite tip at (2, 30) pt.
const PENCIL: CursorImage = CursorImage::png(include_bytes!("../assets/pencil.png"), [2.0, 30.0]);
const INK: Color = Color { r: 0.11, g: 0.11, b: 0.12, a: 1.0 };
const GRID: Color = Color { r: 0.86, g: 0.87, b: 0.89, a: 1.0 };

#[derive(Default)]
struct State {
    strokes: Vec<Vec<[f32; 2]>>,
    redo: Vec<Vec<[f32; 2]>>,
    pan: [f32; 2],
    pan_start: [f32; 2],
    menu: Option<PointerPress>,
    show_handles: bool,
}

fn drawing(s: &State) -> Shape {
    let [px, py] = s.pan;
    let step = 32.0_f32;
    let ox = px.rem_euclid(step);
    let oy = py.rem_euclid(step);
    let mut shape = Shape::new();
    for gx in 0..20 {
        for gy in 0..14 {
            let p = [ox + gx as f32 * step, oy + gy as f32 * step];
            shape = shape.line(p, p, 2.0, LineCap::Round, GRID);
        }
    }
    s.strokes.iter().fold(shape, |shape, stroke| {
        let shifted: Vec<[f32; 2]> = stroke.iter().map(|[x, y]| [x + px, y + py]).collect();
        match shifted.as_slice() {
            [p] => shape.line(*p, *p, 2.5, LineCap::Round, INK),
            points => shape.stroke(points.to_vec(), 2.5, INK),
        }
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

fn context_menu(at: PointerPress, t: &tok::Theme) -> El {
    let item = |key: &'static str, label: &'static str, action: fn(&mut State)| {
        el().key(key).role("menuitem").w_full().h(28.0).px(space::S2).items_center()
            .radius(radius::SM).hover(t.hover).cursor(Cursor::Pointer)
            .on_press(move |s: &mut State| {
                action(s);
                s.menu = None;
            })
            .child(text(label).font(type_::LABEL).color(t.text))
    };
    col().key("canvas-menu").role("menu")
        .absolute(at.x.min(410.0), at.y.min(340.0))
        .w(136.0).padding(4.0).gap(2.0).radius(radius::MD)
        .fill(t.card).border(1.0, t.card_border)
        .modal(true, |s: &mut State| s.menu = None)
        .child(item("menu-reset-pan", "Reset pan", |s| s.pan = [0.0, 0.0]))
        .child(item("menu-clear", "Clear strokes", |s| {
            s.strokes.clear();
            s.redo.clear();
        }))
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
    let hint = text(format!(
        "Right-drag pan ({:.0}, {:.0}) · Right-click menu · Back/Fwd undo/redo ({}/{}) · Middle reset",
        s.pan[0], s.pan[1], s.strokes.len(), s.redo.len(),
    ))
    .font(type_::LABEL)
    .color(t.text_secondary);
    let stage = if s.show_handles {
        crop_handles(&t)
    } else {
        canvas(drawing(s)).key("paper").w_full().grow(1.0).radius(radius::LG)
            .fill(Color::WHITE).border(1.0, t.card_border).clip_rounded(radius::LG)
            .cursor(Cursor::Image(PENCIL))
            .pointer_buttons([PointerButton::Secondary])
            .on_drag(|s: &mut State, d, tx| match tx.button() {
                PointerButton::Secondary => match d.phase {
                    Phase::Down => {
                        s.pan_start = s.pan;
                        s.menu = None;
                    }
                    Phase::Move => s.pan = [s.pan_start[0] + d.dx, s.pan_start[1] + d.dy],
                    Phase::Up => {
                        if d.cancelled() {
                            s.pan = s.pan_start;
                        } else {
                            s.pan = [s.pan_start[0] + d.dx, s.pan_start[1] + d.dy];
                        }
                    }
                },
                _ => match d.phase {
                    Phase::Down => {
                        s.menu = None;
                        s.redo.clear();
                        s.strokes.push(vec![[d.x - s.pan[0], d.y - s.pan[1]]]);
                    }
                    _ => {
                        if let Some(stroke) = s.strokes.last_mut() {
                            stroke.push([d.x - s.pan[0], d.y - s.pan[1]]);
                        }
                    }
                },
            })
            .on_secondary_press(|s: &mut State, p, _| s.menu = Some(p))
    };
    let mut root = col().key("root").w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg)
        .on_button(|s: &mut State, btn, _| match btn {
            PointerButton::Back => {
                if let Some(stroke) = s.strokes.pop() {
                    s.redo.push(stroke);
                }
                true
            }
            PointerButton::Forward => {
                if let Some(stroke) = s.redo.pop() {
                    s.strokes.push(stroke);
                }
                true
            }
            PointerButton::Middle => {
                s.pan = [0.0, 0.0];
                true
            }
            _ => false,
        })
        .child(header)
        .child(stage)
        .child(hint);
    if let Some(at) = s.menu {
        root = root.child(context_menu(at, &t));
    }
    root
}

fn panned_scene(s: &mut State) {
    s.strokes.push(vec![[70.0, 90.0], [140.0, 50.0], [210.0, 120.0], [280.0, 70.0]]);
    s.pan = [48.0, 24.0];
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("handles", |s| s.show_handles = true), ("panned", panned_scene)],
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
    fn right_drag_pans_right_click_opens_menu_and_on_button_handles_undo_redo_reset() {
        let mut rt = Runtime::new(State::default(), view, (560.0, 420.0), Env { system_dark: false, scale: 2.0 });
        rt.frame(0.0);
        let (x, y) = rt.find("paper").unwrap();

        // Draw one stroke.
        rt.pointer_down(x, y);
        rt.pointer_move(x + 25.0, y + 15.0);
        rt.pointer_up(x + 25.0, y + 15.0);
        rt.frame(0.0);
        assert_eq!(rt.state.strokes.len(), 1);

        // Right-drag pans without opening the context menu.
        rt.secondary_down(x, y);
        rt.pointer_move(x + 40.0, y - 20.0);
        rt.secondary_up(x + 40.0, y - 20.0);
        rt.frame(0.0);
        assert_eq!(rt.state.pan, [40.0, -20.0]);
        assert!(rt.state.menu.is_none(), "right-drag suppresses context menu");

        // Stationary right-click opens the context menu.
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.menu.is_some(), "right-click opens context menu");

        // Dismiss menu by clicking outside, then test Back/Forward/Middle via on_button.
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(0.0);
        assert!(rt.state.menu.is_none());

        rt.button_down(x, y, PointerButton::Back);
        rt.button_up(x, y, PointerButton::Back);
        rt.frame(0.0);
        assert_eq!((rt.state.strokes.len(), rt.state.redo.len()), (0, 1));

        rt.button_down(x, y, PointerButton::Forward);
        rt.button_up(x, y, PointerButton::Forward);
        rt.frame(0.0);
        assert_eq!((rt.state.strokes.len(), rt.state.redo.len()), (1, 0));

        rt.button_down(x, y, PointerButton::Middle);
        rt.button_up(x, y, PointerButton::Middle);
        rt.frame(0.0);
        assert_eq!(rt.state.pan, [0.0, 0.0]);
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
