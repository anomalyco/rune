use rune::{Color, LineCap};

use super::{flatten, over};
use crate::canvas::{Canvas, Op, dist, lerp};
use crate::{DiagramStyle, Hover, scene};

/// The colour Rune paints at `p` for `ops` over `surface`: every stroke segment is its own
/// capsule (or box, with butt caps) and blends on its own, as the renderer draws them.
fn painted(ops: &[Op], surface: Color, p: [f32; 2]) -> Color {
    ops.iter().fold(surface, |under, op| match op {
        Op::Fill(points, color) => (0..inside_count(points, p)).fold(under, |u, _| over(*color, u)),
        Op::Stroke { points, closed, width, cap, color } => {
            let mut segs: Vec<([f32; 2], [f32; 2])> = points.windows(2).map(|w| (w[0], w[1])).collect();
            if *closed && points.len() > 2 {
                segs.push((points[points.len() - 1], points[0]));
            }
            segs.iter().filter(|(a, b)| covers(*a, *b, *width, *cap, p)).fold(under, |u, _| over(*color, u))
        }
    })
}

fn inside_count(points: &[[f32; 2]], p: [f32; 2]) -> usize {
    let mut odd = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (a, c) = (points[i], points[j]);
        if (a[1] > p[1]) != (c[1] > p[1]) && p[0] < (c[0] - a[0]) * (p[1] - a[1]) / (c[1] - a[1]) + a[0] {
            odd = !odd;
        }
        j = i;
    }
    odd as usize
}

fn covers(a: [f32; 2], b: [f32; 2], width: f32, cap: LineCap, p: [f32; 2]) -> bool {
    let len = dist(a, b).max(1e-6);
    let d = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    let t = (p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1];
    let n = ((p[0] - a[0]) * -d[1] + (p[1] - a[1]) * d[0]).abs();
    match cap {
        LineCap::Butt => (0.0..=len).contains(&t) && n <= width / 2.0,
        LineCap::Round => dist(p, lerp(a, b, (t / len).clamp(0.0, 1.0))) <= width / 2.0,
    }
}

fn brighter(a: Color, b: Color) -> f32 {
    a.luminance() - b.luminance()
}

const FLOW: &str = "flowchart LR\n  R1[Retry at once] -->|fails| R2[Back off 1 s]\n  R2 -->|fails| Off[Offline]\n  R1 -->|ok| On[Online]";

#[test]
fn a_line_is_no_brighter_at_its_joints_under_its_head_or_round_a_corner() {
    let paint = DiagramStyle::neutral(true);
    let ops = scene(FLOW, &paint, None).expect("draws").ops;
    let at = |p| painted(&ops, paint.surface, p);
    let mut checked = 0;
    for op in &ops {
        let Op::Stroke { points, closed, .. } = op else { continue };
        let longest = points.windows(2).max_by(|a, b| dist(a[0], a[1]).total_cmp(&dist(b[0], b[1]))).expect("a segment");
        let body = at(lerp(longest[0], longest[1], 0.5));
        // Every joint, and an open line's ends, where it runs under its arrowhead.
        for &p in points.iter().skip(if *closed { 0 } else { 1 }) {
            let lift = brighter(at(p), body);
            assert!(lift < 0.002, "brighter by {lift} at {p:?} than along the stroke ({:?} vs {:?})", at(p), body);
            checked += 1;
        }
    }
    assert!(checked > 40, "edges and rounded node corners were checked: {checked}");
}

#[test]
fn every_diagram_draws_its_ink_opaque_over_an_opaque_surface() {
    let sources = [
        FLOW,
        "flowchart TD\n  subgraph S [Box]\n    A --> B\n  end\n  B -.-> C\n  C ==> A",
        "sequenceDiagram\n  participant A\n  participant B\n  A->>B: hi\n  B-->>A: back\n  A-)B: async\n  Note over A,B: note",
        "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : go\n  Busy --> [*]",
        "classDiagram\n  Animal <|-- Duck\n  Animal *-- Leg\n  Animal o-- Tail\n  class Animal {\n    +int age\n    +eat()\n  }",
        "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : contains",
        "journey\n  title Day\n  section Work\n    Code: 5: Me\n    Meet: 2: Me, You",
        "timeline\n  2024 : one\n  2025 : two : three",
        "gitGraph\n  commit\n  branch dev\n  commit\n  checkout main\n  merge dev",
    ];
    for dark in [true, false] {
        let paint = DiagramStyle::neutral(dark);
        assert_eq!(paint.surface.a, 1.0);
        for src in sources {
            let ops = scene(src, &paint, None).unwrap_or_else(|| panic!("draws {src:?}")).ops;
            for op in &ops {
                match op {
                    Op::Stroke { color, .. } => assert_eq!(color.a, 1.0, "a translucent stroke in {src:?}"),
                    Op::Fill(points, color) if points.len() == 3 => assert_eq!(color.a, 1.0, "a translucent arrowhead in {src:?}"),
                    _ => {}
                }
            }
        }
    }
}

#[test]
fn ink_takes_the_colour_of_the_fills_beneath_it() {
    let surface = Color { r: 0.02, g: 0.02, b: 0.02, a: 1.0 };
    let tint = Color::WHITE.alpha(0.1);
    let ink = Color::WHITE.alpha(0.5);
    let mut c = Canvas::default();
    c.fill(vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]], tint);
    c.stroke(vec![[10.0, 50.0], [90.0, 50.0]], false, 1.0, ink);
    c.stroke(vec![[110.0, 50.0], [190.0, 50.0]], false, 1.0, ink);
    c.fill(vec![[150.0, 40.0], [160.0, 50.0], [150.0, 60.0]], ink);
    flatten(&mut c.ops, &mut [], surface);
    let colors: Vec<Color> = c.ops.iter().map(|op| match op {
        Op::Fill(_, c) | Op::Stroke { color: c, .. } => *c,
    }).collect();
    assert_eq!(colors[0], tint, "an area keeps its alpha");
    assert_eq!(colors[1], over(ink, over(tint, surface)), "ink inside the tint");
    assert_eq!(colors[2], over(ink, surface), "ink on the surface");
    assert_eq!(colors[3], colors[2], "a head in the line's colour matches its line");
}

#[test]
fn hover_traces_light_in_opaque_ink_too() {
    let paint = DiagramStyle::neutral(true);
    let Some(crate::Parsed { diagram: crate::Diagram::Flowchart(f), .. }) = crate::parse(FLOW) else { panic!() };
    let mut c = crate::graph::flowchart::draw_canvas(&f, &paint, Some((Hover::Edges, paint.text.alpha(0.9)))).expect("draws");
    flatten(&mut c.ops, &mut c.traces, paint.surface);
    let strokes = c.traces.iter().flat_map(|t| &t.ops).filter_map(|op| match op {
        Op::Stroke { color, .. } => Some(color.a),
        _ => None,
    });
    assert!(strokes.clone().count() >= 3);
    assert!(strokes.into_iter().all(|a| a == 1.0));
}

#[test]
fn srgb_enc_dec_match_clamped_reference_bit_for_bit() {
    let ref_enc = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    };
    let ref_dec = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    for v in 0..=255u8 {
        let x = v as f32 / 255.0;
        assert_eq!(super::enc(x).to_bits(), ref_enc(x).to_bits(), "enc byte {v}");
        assert_eq!(super::dec(x).to_bits(), ref_dec(x).to_bits(), "dec byte {v}");
    }
    for x in [-1.0, -0.01, -0.0, 0.0, 0.003_130_8, 0.040_45, 0.5, 1.0, 1.5] {
        assert_eq!(super::enc(x).to_bits(), ref_enc(x).to_bits(), "enc {x}");
        assert_eq!(super::dec(x).to_bits(), ref_dec(x).to_bits(), "dec {x}");
    }
}
