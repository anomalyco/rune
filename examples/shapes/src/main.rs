//! Vector shapes, element cutouts, and image sampling/tinting: a filled waveform envelope,
//! polylines, meters, a Bézier path, a concave polygon, compound fills with transparent holes
//! under `FillRule::NonZero` and `FillRule::EvenOdd`, SVG 2 path data (`Path::from_svg`,
//! `Shape::fill_svg`, `Shape::fill_svg_with`, `Shape::stroke_svg`), multi-hole circular cutouts
//! (`El::cutout_circles`), nearest-neighbour pixel-art scaling (`El::image_filter`,
//! `ImageFilter::Nearest`), runtime alpha-tinted symbolic icons (`El::pixels_tint`), and
//! wrapped/tiled images (`El::pixels_wrap`, `El::pixels_tile`, `PixelsWrap::{Repeat, Mirror, Extend}`).
//! `cargo run -p shapes -- --snap shapes.png` renders it headless; time animates the waveform,
//! so `--at 1.5` shows another moment.

#![forbid(unsafe_code)]

use rune::{
    AppSpec, Color, Cx, El, FillRule, ImageFilter, LineCap, Path, Pixels, PixelsWrap, Shape,
    SurfaceFit, Tx, canvas, col, el, memo, row, text,
};

struct State {
    t: f32,
    sprite: Pixels,
    symbolic: Pixels,
    motif: Pixels,
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
const STRIPE: Color = Color {
    r: 0.25,
    g: 0.78,
    b: 0.58,
    a: 0.85,
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

/// Appends a circle polygon (`ccw = false` traces clockwise in y-down space).
fn push_circle(mut path: Path, cx: f32, cy: f32, r: f32, steps: usize, reverse: bool) -> Path {
    for i in 0..steps {
        let k = if reverse { steps - i } else { i };
        let a = (k % steps) as f32 * std::f32::consts::TAU / steps as f32;
        let p = [cx + a.cos() * r, cy + a.sin() * r];
        path = if i == 0 { path.move_to(p) } else { path.line_to(p) };
    }
    path.close()
}

/// A self-crossing 5-pointed star (pentagram).
fn pentagram(cx: f32, cy: f32, r: f32) -> Path {
    let order = [0usize, 2, 4, 1, 3];
    let mut p = Path::new();
    for (i, &k) in order.iter().enumerate() {
        let a = k as f32 * std::f32::consts::TAU / 5.0 - std::f32::consts::FRAC_PI_2;
        let pt = [cx + a.cos() * r, cy + a.sin() * r];
        p = if i == 0 { p.move_to(pt) } else { p.line_to(pt) };
    }
    p.close()
}

/// Ring with a transparent hole, same-winding concentric frame under both rules, and a
/// self-crossing pentagram rendered under `FillRule::NonZero` and `FillRule::EvenOdd` over a
/// backdrop bar so transparent holes show through.
fn compound_fills() -> Shape {
    let ring = push_circle(push_circle(Path::new(), 56.0, 52.0, 44.0, 48, false), 56.0, 52.0, 22.0, 36, true);
    let same_winding = |x: f32| {
        Path::new()
            .move_to([x, 10.0]).line_to([x + 84.0, 10.0]).line_to([x + 84.0, 94.0]).line_to([x, 94.0]).close()
            .move_to([x + 22.0, 32.0]).line_to([x + 62.0, 32.0]).line_to([x + 62.0, 72.0]).line_to([x + 22.0, 72.0]).close()
    };
    let star_nz = pentagram(480.0, 54.0, 44.0);
    let star_eo = pentagram(610.0, 54.0, 44.0);
    let translucent_accent = Color { a: 0.65, ..ACCENT };
    let translucent_warm = Color { a: 0.72, ..WARM };

    Shape::new()
        // Backdrop stripe that remains visible right through every transparent hole.
        .fill(vec![[4.0, 44.0], [676.0, 44.0], [676.0, 60.0], [4.0, 60.0]], STRIPE)
        // Translucent ring with opposite-winding hole (NonZero default, zero bridge seams).
        .fill_path(&ring, translucent_accent)
        // Same-winding concentric squares: solid under NonZero, hollow frame under EvenOdd.
        .fill_path_with(&same_winding(148.0), FillRule::NonZero, translucent_accent)
        .fill_path_with(&same_winding(276.0), FillRule::EvenOdd, translucent_accent)
        // Self-crossing pentagram: solid center under NonZero, hollow pentagon under EvenOdd.
        .fill_path_with(&star_nz, FillRule::NonZero, translucent_warm)
        .fill_path_with(&star_eo, FillRule::EvenOdd, translucent_warm)
}

/// SVG 2 path data showcase: a smooth cubic heart (`Shape::fill_svg`), a compound arc donut
/// with a transparent cutout (`Shape::fill_svg_with` + `FillRule::EvenOdd`), a stroked arc and
/// quadratic wave (`Shape::stroke_svg`), and a reusable `Path::from_svg` both filled and stroked.
fn svg_paths() -> Shape {
    let heart = "M 54 78 C 54 78 14 54 14 30 C 14 14 26 6 38 6 C 46 6 51 11 54 18                  C 57 11 62 6 70 6 C 82 6 94 14 94 30 C 94 54 54 78 54 78 Z";
    let arc_donut = "M 224 44 A 36 36 0 1 0 152 44 A 36 36 0 1 0 224 44 Z                      M 206 44 A 18 18 0 1 0 170 44 A 18 18 0 1 0 206 44 Z";
    let arc_wave = "M 272 56 A 26 26 0 0 1 324 56 Q 350 12 376 56 T 428 56";
    let crescent = Path::from_svg(
        "M 512 12 A 34 34 0 1 0 512 76 A 24 24 0 1 1 512 12 Z",
    );
    let badge = "M 576 14 H 644 A 12 12 0 0 1 656 26 V 62 A 12 12 0 0 1 644 74                  H 576 A 12 12 0 0 1 564 62 V 26 A 12 12 0 0 1 576 14 Z                  M 594 44 L 606 56 L 628 32";

    Shape::new()
        // Backdrop stripe so the arc donut's EvenOdd hole is visibly transparent.
        .fill(vec![[136.0, 38.0], [240.0, 38.0], [240.0, 50.0], [136.0, 50.0]], STRIPE)
        .fill_svg(heart, WARM)
        .fill_svg_with(arc_donut, FillRule::EvenOdd, Color { a: 0.72, ..ACCENT })
        .stroke_svg(arc_wave, 3.0, LineCap::Round, ACCENT)
        .fill_path(&crescent, Color { a: 0.85, ..WARM })
        .stroke_path(&crescent, 1.5, LineCap::Round, INK)
        .stroke_svg(badge, 2.5, LineCap::Round, INK)
}

fn label(s: &str) -> El {
    text(s).color(INK)
}

/// A ticket card with two edge notches and a strip with four punched holes (`El::cutout_circles`)
/// laid over a contrasting stripe so every circular cutout shows the backdrop through.
fn cutout_cards() -> El {
    let ticket = col()
        .size(220.0, 88.0)
        .radius(12.0)
        .clip_rounded(12.0)
        .fill(ACCENT)
        .center()
        .cutout_circles([
            ([0.0, 44.0], 9.0, 10.0),
            ([220.0, 44.0], 9.0, 10.0),
        ])
        .child(text("Ticket notches (2 holes)").color(INK));
    let punched = col()
        .size(220.0, 88.0)
        .radius(12.0)
        .clip_rounded(12.0)
        .fill(WARM)
        .cutout_circles((0..4).map(|i| ([35.0 + i as f32 * 50.0, 44.0], 9.0, 10.0)));
    el()
        .size(464.0, 88.0)
        .child(el().absolute(0.0, 34.0).size(464.0, 20.0).fill(STRIPE))
        .child(row().gap(24.0).child(ticket).child(punched))
}

/// A 4×4 pixel-art sprite (contrasting colour tiles) for comparing `ImageFilter::Linear` and
/// `ImageFilter::Nearest` when scaled up to 64×64.
fn pixel_sprite() -> Pixels {
    const PALETTE: [[u8; 4]; 4] = [
        [20, 24, 34, 255],
        [90, 140, 255, 255],
        [255, 140, 76, 255],
        [240, 244, 248, 255],
    ];
    const GRID: [[usize; 4]; 4] = [
        [0, 1, 1, 0],
        [1, 3, 2, 1],
        [1, 2, 3, 1],
        [0, 1, 1, 0],
    ];
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for row in GRID {
        for idx in row {
            rgba.extend_from_slice(&PALETTE[idx]);
        }
    }
    let px = Pixels::new();
    px.set(4, 4, rgba);
    px
}

/// An 8×8 symbolic folder-style alpha mask painted in dark grey (`#2e3436`, like a Freedesktop
/// `*-symbolic.svg` loaded at run time) to demonstrate recolouring with `El::pixels_tint`.
fn symbolic_icon() -> Pixels {
    const MASK: [&str; 8] = [
        "........",
        ".###....",
        ".######.",
        ".######.",
        ".##++##.",
        ".######.",
        ".######.",
        "........",
    ];
    let mut rgba = Vec::with_capacity(8 * 8 * 4);
    for line in MASK {
        for ch in line.bytes() {
            let a = match ch {
                b'#' => 255,
                b'+' => 140,
                _ => 0,
            };
            rgba.extend_from_slice(&[0x2e, 0x34, 0x36, a]);
        }
    }
    let px = Pixels::new();
    px.set(8, 8, rgba);
    px
}

fn pixel_sampling_and_tint(sprite: &Pixels, symbolic: &Pixels) -> El {
    let tile = |title: &str, preview: El| {
        col()
            .gap(6.0)
            .child(
                el()
                    .size(64.0, 64.0)
                    .radius(8.0)
                    .fill(Color { r: 0.12, g: 0.13, b: 0.16, a: 1.0 })
                    .center()
                    .child(preview),
            )
            .child(text(title).color(INK))
    };
    row()
        .gap(20.0)
        .child(tile(
            "Linear (default)",
            el().size(64.0, 64.0).radius(8.0).pixels(sprite),
        ))
        .child(tile(
            "Nearest (4x4)",
            el().size(64.0, 64.0).radius(8.0).pixels(sprite).image_filter(ImageFilter::Nearest),
        ))
        .child(tile(
            "Raw #2e3436",
            el().size(48.0, 48.0).pixels_fit(symbolic, SurfaceFit::Fit).image_filter(ImageFilter::Nearest),
        ))
        .child(tile(
            "Tint: INK",
            el().size(48.0, 48.0).pixels_fit(symbolic, SurfaceFit::Fit).image_filter(ImageFilter::Nearest).pixels_tint(INK),
        ))
        .child(tile(
            "Tint: ACCENT",
            el().size(48.0, 48.0).pixels_fit(symbolic, SurfaceFit::Fit).image_filter(ImageFilter::Nearest).pixels_tint(ACCENT),
        ))
        .child(tile(
            "Tint: 0.6 white",
            el().size(48.0, 48.0).pixels_fit(symbolic, SurfaceFit::Fit).image_filter(ImageFilter::Nearest).pixels_tint(Color::WHITE.alpha(0.6)),
        ))
}

/// An asymmetric 8×8 directional motif (amber top-left corner, blue diagonal arrow, crisp border
/// accents) so `Repeat`, `Mirror`, and `Extend` produce visually distinct patterns.
fn wrap_motif() -> Pixels {
    const PALETTE: [[u8; 4]; 5] = [
        [24, 28, 38, 255],
        [72, 132, 245, 255],
        [255, 148, 72, 255],
        [232, 238, 246, 255],
        [72, 198, 146, 255],
    ];
    const GRID: [[usize; 8]; 8] = [
        [2, 2, 2, 0, 0, 0, 0, 4],
        [2, 3, 0, 0, 0, 1, 1, 0],
        [2, 0, 0, 0, 1, 3, 1, 0],
        [0, 0, 0, 1, 3, 1, 0, 0],
        [0, 0, 1, 3, 1, 0, 0, 0],
        [0, 1, 3, 1, 0, 0, 0, 0],
        [0, 1, 1, 0, 0, 0, 4, 4],
        [0, 0, 0, 0, 0, 4, 4, 4],
    ];
    let mut rgba = Vec::with_capacity(8 * 8 * 4);
    for r in GRID {
        for idx in r {
            rgba.extend_from_slice(&PALETTE[idx]);
        }
    }
    let px = Pixels::new();
    px.set(8, 8, rgba);
    px
}

fn pixel_wrap_and_tile(motif: &Pixels, symbolic: &Pixels) -> El {
    let swatch = |title: &str, box_el: El| {
        col()
            .gap(6.0)
            .child(box_el)
            .child(text(title).color(INK))
    };
    let placed = [36.0, 14.0, 40.0, 40.0];
    row()
        .gap(16.0)
        .child(swatch(
            "pixels_tile (24x24)",
            el()
                .size(132.0, 68.0)
                .radius(8.0)
                .pixels_tile(motif, [24.0, 24.0])
                .image_filter(ImageFilter::Nearest),
        ))
        .child(swatch(
            "Wrap::Repeat",
            el()
                .size(132.0, 68.0)
                .radius(8.0)
                .pixels_wrap(motif, placed, PixelsWrap::Repeat)
                .image_filter(ImageFilter::Nearest),
        ))
        .child(swatch(
            "Wrap::Mirror",
            el()
                .size(132.0, 68.0)
                .radius(8.0)
                .pixels_wrap(motif, placed, PixelsWrap::Mirror)
                .image_filter(ImageFilter::Nearest),
        ))
        .child(swatch(
            "Wrap::Extend",
            el()
                .size(132.0, 68.0)
                .radius(8.0)
                .pixels_wrap(motif, placed, PixelsWrap::Extend)
                .image_filter(ImageFilter::Nearest),
        ))
        .child(swatch(
            "Tile + tint",
            el()
                .size(132.0, 68.0)
                .radius(8.0)
                .fill(Color { r: 0.12, g: 0.13, b: 0.16, a: 1.0 })
                .pixels_tile(symbolic, [22.0, 22.0])
                .pixels_tint(ACCENT.alpha(0.35))
                .image_filter(ImageFilter::Nearest),
        ))
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
        .child(
            col()
                .gap(8.0)
                .child(label("Compound fills: ring with hole, same-winding NonZero vs EvenOdd, crossing star NonZero vs EvenOdd"))
                .child(memo("compound", (), || canvas(compound_fills()).size(680.0, 104.0))),
        )
        .child(
            col()
                .gap(8.0)
                .child(label("SVG 2 path data: fill_svg, fill_svg_with (EvenOdd arc ring), stroke_svg, Path::from_svg"))
                .child(memo("svg_paths", (), || canvas(svg_paths()).size(680.0, 88.0))),
        )
        .child(
            col()
                .gap(8.0)
                .child(label("Circular cutouts (El::cutout_circles): ticket notches and four punched holes"))
                .child(memo("cutouts", (), cutout_cards)),
        )
        .child(
            col()
                .gap(8.0)
                .child(label("Image sampling & alpha tint: ImageFilter::Linear vs Nearest (4x4 sprite) and El::pixels_tint on a #2e3436 symbolic mask"))
                .child(pixel_sampling_and_tint(&s.sprite, &s.symbolic)),
        )
        .child(
            col()
                .gap(8.0)
                .child(label("Image tiling & wrapping (El::pixels_tile, El::pixels_wrap): Repeat, Mirror, Extend, and tinted tiled motif"))
                .child(pixel_wrap_and_tile(&s.motif, &s.symbolic)),
        )
}

fn poll(s: &mut State, t: f64, _: &mut Tx) -> bool {
    s.t = t as f32;
    true
}

fn main() {
    rune::run(AppSpec {
        poll: Some(poll),
        ..AppSpec::new(
            "Shapes",
            (820.0, 1010.0),
            || State {
                t: 0.0,
                sprite: pixel_sprite(),
                symbolic: symbolic_icon(),
                motif: wrap_motif(),
            },
            view,
        )
    });
}
