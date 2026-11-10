//! Vector shapes: a filled waveform envelope, polylines, meters, a Bézier path and a concave
//! polygon. `cargo run -p shapes -- --snap shapes.png` renders it headless; time animates the
//! waveform, so `--at 1.5` shows another moment.

use rune::{AppSpec, Color, Cx, El, LineCap, Path, Shape, Tx, canvas, col, memo, row, text};

struct State {
    t: f32,
}

const INK: Color = Color {
    r: 0.92,
    g: 0.93,
    b: 0.95,
    a: 1.0,
};
const ACCENT: Color = Color {
    r: 0.35,
    g: 0.55,
    b: 1.0,
    a: 1.0,
};
const SOFT: Color = Color {
    r: 0.35,
    g: 0.55,
    b: 1.0,
    a: 0.35,
};
const WARM: Color = Color {
    r: 1.0,
    g: 0.55,
    b: 0.3,
    a: 1.0,
};

fn level(t: f32, i: usize) -> f32 {
    let x = i as f32 * 0.09 + t * 2.0;
    ((x.sin() * 0.6 + (x * 2.7).sin() * 0.3 + (x * 7.1).cos() * 0.1).abs()).min(1.0)
}

fn waveform(t: f32, w: f32, h: f32) -> Shape {
    let n = 160;
    let mid = h / 2.0;
    let top: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            [
                i as f32 * w / (n - 1) as f32,
                mid - level(t, i) * (mid - 2.0) - 0.5,
            ]
        })
        .collect();
    let mut envelope = top.clone();
    envelope.extend(top.iter().rev().map(|p| [p[0], 2.0 * mid - p[1]]));
    let line: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            [
                i as f32 * w / (n - 1) as f32,
                mid + (i as f32 * 0.21 + t * 3.0).sin() * mid * 0.8,
            ]
        })
        .collect();
    // Drawn in a w × h box and stretched to the element's size.
    Shape::new()
        .fill(envelope, SOFT)
        .stroke(line, 1.5, ACCENT)
        .line([0.0, mid], [w, mid], 0.5, LineCap::Butt, INK)
        .view_box(w, h)
}

fn meters(t: f32) -> Shape {
    let mut s = Shape::new();
    for i in 0..24 {
        let x = 4.0 + i as f32 * 10.0;
        let v = level(t, i * 7) * 56.0 + 4.0;
        s = s.line(
            [x, 64.0],
            [x, 64.0 - v],
            6.0,
            LineCap::Round,
            if v > 48.0 { WARM } else { ACCENT },
        );
    }
    s
}

fn curves() -> Shape {
    let wave = Path::new()
        .move_to([4.0, 50.0])
        .cubic_to([40.0, -20.0], [80.0, 120.0], [116.0, 50.0])
        .quad_to([150.0, 0.0], [186.0, 50.0]);
    let blob = Path::new()
        .move_to([210.0, 30.0])
        .cubic_to([230.0, 0.0], [270.0, 0.0], [280.0, 30.0])
        .line_to([260.0, 80.0])
        .line_to([220.0, 80.0])
        .close();
    let star: Vec<[f32; 2]> = (0..10)
        .map(|i| {
            let a = i as f32 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { 40.0 } else { 16.0 };
            [340.0 + a.cos() * r, 46.0 + a.sin() * r]
        })
        .collect();
    Shape::new()
        .stroke_path(&wave, 3.0, LineCap::Round, WARM)
        .fill_path(&blob, ACCENT)
        .stroke_path(&blob, 1.0, LineCap::Round, INK)
        .fill(star, INK)
        .line([400.0, 10.0], [460.0, 86.0], 0.5, LineCap::Butt, INK)
        .line([410.0, 10.0], [470.0, 86.0], 1.0, LineCap::Butt, INK)
        .line([420.0, 10.0], [480.0, 86.0], 2.0, LineCap::Butt, INK)
}

fn label(s: &str) -> El {
    text(s).color(INK)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    col()
        .key("root")
        .padding(24.0)
        .gap(16.0)
        .fill(Color {
            r: 0.06,
            g: 0.065,
            b: 0.08,
            a: 1.0,
        })
        .child(label("Waveform: filled envelope, polyline, hairline"))
        .child(
            canvas(waveform(s.t, 260.0, 96.0))
                .key("wave")
                .size(520.0, 96.0)
                .fill(Color {
                    r: 0.1,
                    g: 0.11,
                    b: 0.13,
                    a: 1.0,
                })
                .radius(8.0),
        )
        .child(
            row()
                .gap(24.0)
                .child(
                    col()
                        .gap(8.0)
                        .child(label("Meters"))
                        .child(canvas(meters(s.t)).key("meters").size(244.0, 68.0)),
                )
                .child(
                    col()
                        .gap(8.0)
                        .child(label("Paths, a concave fill, line widths"))
                        .child(memo("curves", (), || canvas(curves()).size(490.0, 96.0))),
                ),
        )
}

fn poll(s: &mut State, t: f64, _: &mut Tx) -> bool {
    s.t = t as f32;
    true
}

fn main() {
    rune::run(AppSpec { poll: Some(poll), ..AppSpec::new("Shapes", (820.0, 360.0), || State { t: 0.0 }, view) });
}
