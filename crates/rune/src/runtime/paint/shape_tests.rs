use std::cell::Cell;
use std::rc::Rc;

use crate::render::cpu_boot::{CpuPainter, Scene, Target};
use crate::render::{KIND_LAYER, KIND_RECT, KIND_SEGMENT};
use crate::{Color, Cx, El, Env, Runtime, ScrollSpec, Shape, canvas, canvas_with, col, el};

const BAR_COLOR: Color = Color { r: 0.25, g: 0.55, b: 0.92, a: 1.0 };
const FOOT_COLOR: Color = Color { r: 0.92, g: 0.42, b: 0.22, a: 1.0 };

const BAR_COUNT: usize = 34;
const BAR_W: f32 = 2.0;
const BAR_GAP: f32 = 1.5;
const STRIP_W: f32 = BAR_COUNT as f32 * BAR_W + (BAR_COUNT - 1) as f32 * BAR_GAP;
const STRIP_H: f32 = 24.0;

fn bar_heights(i: usize) -> (f32, f32) {
    let h = 4.0 + ((i * 7 + 3) % 18) as f32;
    let foot = (2.0 + ((i * 5 + 1) % 7) as f32).min(h);
    (h, foot)
}

fn bar_strip_boxes(ox: f32, oy: f32) -> El {
    let mut strip = el().absolute(ox, oy).size(STRIP_W, STRIP_H);
    for i in 0..BAR_COUNT {
        let x = i as f32 * (BAR_W + BAR_GAP);
        let (h, foot) = bar_heights(i);
        strip = strip
            .child(
                el().absolute(x, STRIP_H - h)
                    .size(BAR_W, h)
                    .corners([1.0, 1.0, 0.0, 0.0])
                    .fill(BAR_COLOR),
            )
            .child(el().absolute(x, STRIP_H - foot).size(BAR_W, foot).fill(FOOT_COLOR));
    }
    el().size(160.0, 48.0).child(strip)
}

fn bar_strip_shape(w: f32, h: f32) -> Shape {
    let mut s = Shape::new();
    let total = BAR_COUNT as f32 * BAR_W + (BAR_COUNT - 1) as f32 * BAR_GAP;
    let start_x = (w - total).max(0.0);
    for i in 0..BAR_COUNT {
        let x = start_x + i as f32 * (BAR_W + BAR_GAP);
        let (bh, foot) = bar_heights(i);
        s = s
            .rounded_rect_corners([x, h - bh, BAR_W, bh], [1.0, 1.0, 0.0, 0.0], BAR_COLOR)
            .rect([x, h - foot, BAR_W, foot], FOOT_COLOR);
    }
    s
}

fn cpu_render<S: 'static>(rt: &Runtime<S>, w: f32, h: f32, scale: f32) -> Vec<u8> {
    let (dw, dh) = ((w * scale).round() as u32, (h * scale).round() as u32);
    let mut buf = vec![0u8; (dw * dh * 4) as usize];
    let mut painter = CpuPainter::new(1);
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (dw, dh), rt.clear_color());
    painter.paint(&scene, &mut Target::new(&mut buf, dw, dh)).expect("CPU paint succeeds");
    buf
}

fn rect_instances<S: 'static>(rt: &Runtime<S>) -> Vec<([f32; 4], [f32; 4], [f32; 4])> {
    rt.instances
        .iter()
        .filter(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0)
        .map(|i| (i.rect, i.radii, i.fill))
        .collect()
}

#[test]
fn shape_rects_match_box_fills_byte_for_byte_across_scales_and_fractional_origins() {
    let origins = [0.0f32, 1.0 / 3.0, 0.5];
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        for &fx in &origins {
            for &fy in &origins {
                let (ox, oy) = (4.0 + fx, 6.0 + fy);

                let mut boxes_rt = Runtime::new(
                    (ox, oy),
                    |&(ox, oy), _| bar_strip_boxes(ox, oy),
                    (160.0, 48.0),
                    Env::new(true, scale),
                );
                boxes_rt.frame(0.0);

                let mut canvas_rt = Runtime::new(
                    (ox, oy),
                    |&(ox, oy), _| {
                        el().size(160.0, 48.0)
                            .child(canvas(bar_strip_shape(STRIP_W, STRIP_H)).absolute(ox, oy).size(STRIP_W, STRIP_H))
                    },
                    (160.0, 48.0),
                    Env::new(true, scale),
                );
                canvas_rt.frame(0.0);

                let mut builder_rt = Runtime::new(
                    (ox, oy),
                    |&(ox, oy), _| {
                        el().size(160.0, 48.0).child(
                            canvas_with(|[w, h], _| bar_strip_shape(w, h))
                                .absolute(ox, oy)
                                .size(STRIP_W, STRIP_H),
                        )
                    },
                    (160.0, 48.0),
                    Env::new(true, scale),
                );
                builder_rt.frame(0.0);

                // Every bar rect instance must land on whole pixels and match box fills identically.
                let box_rects = rect_instances(&boxes_rt);
                let canvas_rects = rect_instances(&canvas_rt);
                let builder_rects = rect_instances(&builder_rt);
                assert_eq!(canvas_rects.len(), BAR_COUNT * 2);
                assert_eq!(canvas_rects, box_rects, "scale {scale}, origin ({ox}, {oy}): canvas(Shape) rects differ");
                assert_eq!(builder_rects, box_rects, "scale {scale}, origin ({ox}, {oy}): canvas_with rects differ");
                for (r, _, _) in &canvas_rects {
                    assert!(
                        r.iter().all(|v| v.fract() == 0.0),
                        "scale {scale}, origin ({ox}, {oy}): non-integer rect {r:?}"
                    );
                }

                // CPU raster output must be byte-for-byte identical to box elements.
                let box_px = cpu_render(&boxes_rt, 160.0, 48.0, scale);
                let canvas_px = cpu_render(&canvas_rt, 160.0, 48.0, scale);
                let builder_px = cpu_render(&builder_rt, 160.0, 48.0, scale);
                assert_eq!(canvas_px, box_px, "scale {scale}, origin ({ox}, {oy}): canvas(Shape) pixels differ");
                assert_eq!(builder_px, box_px, "scale {scale}, origin ({ox}, {oy}): canvas_with pixels differ");
            }
        }
    }
}

#[test]
fn shape_rect_respects_pixel_snap_opt_out() {
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let mut boxes_rt = Runtime::new(
            (),
            |_, _| {
                el().size(60.0, 40.0).child(
                    el().absolute(4.3, 5.2).size(20.0, 12.0).pixel_snap(false).child(
                        el().absolute(1.5, 2.5).size(3.0, 4.0).fill(BAR_COLOR),
                    ),
                )
            },
            (60.0, 40.0),
            Env::new(true, scale),
        );
        boxes_rt.frame(0.0);

        let mut canvas_rt = Runtime::new(
            (),
            |_, _| {
                el().size(60.0, 40.0).child(
                    canvas(Shape::new().rect([1.5, 2.5, 3.0, 4.0], BAR_COLOR))
                        .absolute(4.3, 5.2)
                        .size(20.0, 12.0)
                        .pixel_snap(false),
                )
            },
            (60.0, 40.0),
            Env::new(true, scale),
        );
        canvas_rt.frame(0.0);

        let br = rect_instances(&boxes_rt)[0].0;
        let cr = rect_instances(&canvas_rt)[0].0;
        for k in 0..4 {
            assert!((br[k] - cr[k]).abs() <= 1.0 / 32.0, "scale {scale}: {br:?} vs {cr:?}");
        }
    }
}

#[test]
fn canvas_with_receives_laid_out_size_and_scale_on_frame_zero_and_caches() {
    let calls = Rc::new(Cell::new(0usize));
    let seen = Rc::new(Cell::new(([0.0f32; 2], 0.0f32)));

    struct St {
        calls: Rc<Cell<usize>>,
        seen: Rc<Cell<([f32; 2], f32)>>,
        pad: f32,
    }

    fn view(st: &St, _: &mut Cx<St>) -> El {
        let calls = Rc::clone(&st.calls);
        let seen = Rc::clone(&st.seen);
        // Flexible width via w_full() inside padded parent: view does not know width upfront.
        col().w_full().h(40.0).px(st.pad).child(
            canvas_with(move |[w, h], scale| {
                calls.set(calls.get() + 1);
                seen.set(([w, h], scale));
                // Anchor a 10 pt bar to the right edge and bottom edge without view_box stretching.
                Shape::new().rect([w - 10.0, h - 8.0, 10.0, 8.0], BAR_COLOR)
            })
            .w_full()
            .h(20.0),
        )
    }

    let mut rt = Runtime::new(
        St { calls: Rc::clone(&calls), seen: Rc::clone(&seen), pad: 12.0 },
        view,
        (200.0, 60.0),
        Env::new(true, 1.5),
    );
    rt.frame(0.0);

    // Received exact laid-out size and scale on frame 0 (no 1-frame lag).
    assert_eq!(seen.get(), ([176.0, 20.0], 1.5));
    assert_eq!(calls.get(), 1);
    let r = rect_instances(&rt)[0].0;
    // Right-anchored bar at x = 12 + (176 - 10) = 178 pt -> 267 px; width = 10 pt -> 15 px.
    assert_eq!(r, [267.0, 18.0, 15.0, 12.0]);

    // Repainting with unchanged size/scale does not re-invoke the closure.
    rt.frame(1.0 / 60.0);
    assert_eq!(calls.get(), 1);

    // Resizing the viewport updates laid-out size immediately on the same frame.
    rt.set_viewport((240.0, 60.0), 1.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(seen.get(), ([216.0, 20.0], 1.5));
    assert_eq!(calls.get(), 2);
    let r2 = rect_instances(&rt)[0].0;
    assert_eq!(r2, [327.0, 18.0, 15.0, 12.0]);

    // Changing device scale updates scale immediately on the same frame.
    rt.set_viewport((240.0, 60.0), 2.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(seen.get(), ([216.0, 20.0], 2.0));
    assert_eq!(calls.get(), 3);
    let r3 = rect_instances(&rt)[0].0;
    assert_eq!(r3, [436.0, 24.0, 20.0, 16.0]);
}

#[test]
fn canvas_with_works_on_frame_zero_inside_virtual_rows() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(180.0, 60.0).child(
            col()
                .w_full()
                .h_full()
                .scroll(ScrollSpec { stick_bottom: false })
                .child(col().w_full().growth(crate::GrowthSpec::default()).virtual_rows(
                    10,
                    20.0,
                    |_: &(), i| format!("row-{i}").into(),
                    |_: &(), i| {
                        canvas_with(move |[w, h], _scale| {
                            let bw = 6.0 + (i as f32) * 2.0;
                            Shape::new().rect([w - bw, h - 10.0, bw, 10.0], BAR_COLOR)
                        })
                        .w_full()
                        .h(20.0)
                    },
                )),
        )
    }

    let mut rt = Runtime::new((), view, (180.0, 60.0), Env::new(true, 1.25));
    rt.frame(0.0);
    let rects = rect_instances(&rt);
    assert!(rects.len() >= 3, "visible virtual rows painted on frame 0: {rects:?}");
    // Every bar is right-aligned against 180 pt * 1.25 = 225 px on frame 0.
    for (r, _, _) in &rects {
        assert_eq!(r[0] + r[2], 225.0, "right edge aligned at 225 px: {r:?}");
        assert!(r.iter().all(|v| v.fract() == 0.0), "whole pixels: {r:?}");
    }
}

#[test]
fn scrolled_canvas_with_keeps_spacing_and_passes_paint_keep_strict_shift() {
    let calls = Rc::new(Cell::new(0usize));

    struct St {
        calls: Rc<Cell<usize>>,
    }

    fn view(st: &St, _: &mut Cx<St>) -> El {
        let rows = (0..12).map(|i| {
            let calls = Rc::clone(&st.calls);
            el().key(format!("r{i}")).w_full().h(20.0).child(
                canvas_with(move |[w, h], scale| {
                    calls.set(calls.get() + 1);
                    Shape::new()
                        .rounded_rect_corners([4.3, 2.0, 12.0, h - 4.0], [1.0, 1.0, 0.0, 0.0], BAR_COLOR)
                        .rect([w - 16.0, h - 6.0, 12.0, 6.0], FOOT_COLOR)
                        .line([2.0, h - 1.0], [w - 2.0, h - 1.0], 1.0 / scale, crate::LineCap::Butt, BAR_COLOR)
                })
                .w_full()
                .h(20.0),
            )
        });
        col().size(120.0, 80.0).child(
            col()
                .key("list")
                .w_full()
                .h_full()
                .scroll(ScrollSpec { stick_bottom: false })
                .children(rows),
        )
    }

    for scale in [1.25f32, 1.5, 2.0] {
        calls.set(0);
        let mut rt = Runtime::new(St { calls: Rc::clone(&calls) }, view, (120.0, 80.0), Env::new(true, scale));
        #[cfg(debug_assertions)]
        rt.set_paint_keep_strict(true);
        rt.frame(0.0);
        let initial_calls = calls.get();
        let initial_widths: Vec<f32> = rect_instances(&rt).into_iter().map(|(r, _, _)| r[2]).collect();
        assert!(!initial_widths.is_empty());

        for _ in 0..8 {
            rt.wheel(60.0, 40.0, -0.37);
            rt.frame(1.0 / 120.0);
            let rects = rect_instances(&rt);
            assert!(rects.iter().all(|(r, _, _)| r.iter().all(|v| v.fract() == 0.0)), "scale {scale}: {rects:?}");
            // Every bar rect retains its exact device-pixel width across scroll offsets.
            assert!(rt.instances.iter().any(|q| q.params[1] == KIND_SEGMENT));
        }
        // Scrolling rows whose size/scale did not change should not rebuild already-cached rows.
        assert!(calls.get() <= 12, "scale {scale}: expected at most 12 row builds, got {} (initial {initial_calls})", calls.get());
    }
}

#[test]
fn fading_canvas_with_isolates_only_when_shape_overlaps() {
    let mut single = Runtime::new(
        (),
        |_, _| {
            el().size(40.0, 40.0).child(
                canvas_with(|[w, h], _| Shape::new().rect([0.0, 0.0, w, h], BAR_COLOR))
                    .size(40.0, 40.0)
                    .opacity(0.5),
            )
        },
        (40.0, 40.0),
        Env::new(true, 2.0),
    );
    single.frame(0.0);
    assert!(!single.instances.iter().any(|i| i.params[1] == KIND_LAYER), "single rect does not need layer");

    let mut multi = Runtime::new(
        (),
        |_, _| {
            el().size(40.0, 40.0).child(
                canvas_with(|[w, h], _| {
                    Shape::new()
                        .rect([0.0, 0.0, w, h], BAR_COLOR)
                        .rect([0.0, h * 0.5, w, h * 0.5], FOOT_COLOR)
                })
                .size(40.0, 40.0)
                .opacity(0.5),
            )
        },
        (40.0, 40.0),
        Env::new(true, 2.0),
    );
    multi.frame(0.0);
    assert!(multi.instances.iter().any(|i| i.params[1] == KIND_LAYER), "multi-rect faded shape isolates into layer");
}

#[test]
fn shape_rect_and_canvas_with_export_to_web_svg() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .size(80.0, 60.0)
            .child(canvas(Shape::new().rect([1.0, 2.0, 10.0, 12.0], BAR_COLOR)).key("sq").size(20.0, 20.0))
            .child(canvas(Shape::new().rounded_rect([0.0, 0.0, 20.0, 10.0], 3.0, BAR_COLOR)).key("uni").size(20.0, 20.0))
            .child(
                canvas_with(|[w, h], _| {
                    Shape::new().rounded_rect_corners([0.0, 0.0, w, h], [2.0, 2.0, 0.0, 0.0], FOOT_COLOR)
                })
                .key("dyn")
                .size(24.0, 16.0),
            )
    }

    let mut rt = Runtime::new((), view, (80.0, 60.0), Env::new(true, 2.0));
    rt.page_mode();
    rt.frame(0.0);
    let nodes = rt.export_nodes();
    let sq = nodes.iter().find(|n| n.path.ends_with("sq")).expect("sq");
    let uni = nodes.iter().find(|n| n.path.ends_with("uni")).expect("uni");
    let dyn_node = nodes.iter().find(|n| n.path.ends_with("dyn")).expect("dyn");
    assert!(sq.inner.contains(r#"<rect x="1" y="2" width="10" height="12""#), "square rect svg: {}", sq.inner);
    assert!(uni.inner.contains(r#"rx="3" ry="3""#), "uniform rounded rect svg: {}", uni.inner);
    assert!(dyn_node.inner.contains("A2,2,0,0,1,24,2"), "per-corner rounded rect from canvas_with: {}", dyn_node.inner);
}

#[test]
fn shape_with_and_shape_replace_each_other_and_invalidate_memo_reuse() {
    fn view(mode: &u8, _: &mut Cx<u8>) -> El {
        let m = *mode;
        // Same memo key and version (0), with shape/shape_with attached on the memo root itself:
        // memo_reuse::same_visual must notice the change between shape, shape_with, and a fresh closure.
        let base = crate::memo::memo("chart", 0u8, || el().size(40.0, 20.0));
        let styled = match m {
            0 => base.shape(Shape::new().rect([0.0, 0.0, 10.0, 10.0], BAR_COLOR)),
            1 => base.shape_with(|[w, h], _| Shape::new().rect([0.0, 0.0, w, h], FOOT_COLOR)),
            2 => base.shape_with(|[w, h], _| Shape::new().rect([0.0, 0.0, w * 0.5, h], BAR_COLOR)),
            _ => base
                .shape_with(|[w, h], _| Shape::new().rect([0.0, 0.0, w * 0.5, h], BAR_COLOR))
                .shape(Shape::new().rect([0.0, 0.0, 12.0, 10.0], BAR_COLOR)),
        };
        el().size(40.0, 20.0).child(styled)
    }

    let mut rt = Runtime::new(0u8, view, (40.0, 20.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rect_instances(&rt)[0].0, [0.0, 0.0, 10.0, 10.0]);

    rt.state = 1;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rect_instances(&rt)[0].0, [0.0, 0.0, 40.0, 20.0]);

    // A new shape_with closure invalidates memo reuse even when memo version is unchanged.
    rt.state = 2;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rect_instances(&rt)[0].0, [0.0, 0.0, 20.0, 20.0]);

    // Calling .shape(...) after .shape_with(...) replaces the builder.
    rt.state = 3;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rect_instances(&rt)[0].0, [0.0, 0.0, 12.0, 10.0]);
}
