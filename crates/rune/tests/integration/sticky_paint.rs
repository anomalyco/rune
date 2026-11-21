//! Headless paint, occlusion, edge-fade, and memo-replay tests for sticky elements.

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::runtime::ElementInfo;
use rune::{Color, Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, Shadow, col, el, memo, row, text};

fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key:?}"))
}

fn pixels<S: 'static>(gpu: &Gpu, renderer: &mut Renderer, rt: &mut Runtime<S>, size: (u32, u32)) -> Vec<u8> {
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

#[derive(Clone, Copy)]
struct OccludeState {
    memoized: bool,
    open: bool,
}

fn occlude_view(s: &OccludeState, _: &mut Cx<OccludeState>) -> El {
    let build_group = |open: bool| {
        let mut head = row()
            .key("head")
            .w_full()
            .h(24.0)
            .animate_layout()
            .child(text("Used 3 tools").color(Color::WHITE));
        if open {
            head = head
                .sticky_top(0.0)
                .fill(Color { r: 0.86, g: 0.2, b: 0.2, a: 1.0 })
                .stuck_shadow(Shadow {
                    y: 8.0,
                    blur: 8.0,
                    color: Color::BLACK,
                    ..Default::default()
                });
        }
        let mut g = col().key("group").w_full().pb(12.0).child(head);
        if open {
            let mut items = col().key("items").w_full().pt(4.0);
            for j in 0..16 {
                // Bright green rows that scroll directly under the red stuck header.
                items = items.child(
                    el().key(format!("item-{j}"))
                        .w_full()
                        .h(22.0)
                        .fill(Color { r: 0.0, g: 0.94, b: 0.16, a: 1.0 }),
                );
            }
            g = g.child(items);
        }
        g
    };
    let open = s.open;
    let memoized = s.memoized;
    let stack = col()
        .key("timeline")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            4,
            72.0,
            |_: &OccludeState, i| format!("row-{i}").into(),
            move |_: &OccludeState, i| {
                if i != 1 {
                    return el().key(format!("spacer-{i}")).w_full().h(60.0);
                }
                if memoized {
                    memo("row-1", open, move || {
                        col()
                            .w_full()
                            .max_w(200.0)
                            .streaming(false)
                            .child(build_group(open))
                    })
                } else {
                    col()
                        .w_full()
                        .max_w(200.0)
                        .streaming(false)
                        .child(build_group(open))
                }
            },
        );
    col()
        .w_full()
        .h_full()
        .fill(Color { r: 0.12, g: 0.125, b: 0.14, a: 1.0 })
        .child(
            col()
                .key("scroll")
                .w(200.0)
                .h(200.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .scroll_edge_fade(16.0)
                .child(stack),
        )
}

#[test]
fn sticky_header_stays_opaque_and_occludes_scrolled_rows_under_edge_fade_and_memo() {
    let scale = 2.0;
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let size = ((200.0 * scale) as u32, (200.0 * scale) as u32);
    let mut render_pixels = |memoized: bool| -> Vec<u8> {
        let mut rt = Runtime::new(
            OccludeState { memoized, open: false },
            occlude_view,
            (200.0, 200.0),
            Env::new(true, scale),
        );
        rt.frame(0.0);
        // Expand the group (matching a chat timeline's collapsible tool group inside `memo`)
        // and settle the growth animation.
        rt.update(|s, _| s.open = true);
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        // Scroll 135 pt so `head` sticks at y=0..24 and bright green items sit at y=-3..19
        // directly behind the top edge-fade band (y=0..16).
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 135.0));
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        let head = find(&rt, "head");
        assert_eq!(head.rect, [0.0, 0.0, 200.0, 24.0]);
        assert!((head.stuck - 75.0).abs() < 0.5, "stuck shift: {}", head.stuck);

        let before = pixels(&gpu, &mut renderer, &mut rt, size);
        // Rebuild with an unchanged memo version, then scroll away and back. A memo hit
        // must not replay paint geometry, clips or shadow state captured on an earlier frame.
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 80.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 135.0));
        rt.frame(0.0);
        let after = pixels(&gpu, &mut renderer, &mut rt, size);
        assert_eq!(before, after, "unchanged memo replay changed sticky fill/shadow pixels");
        after
    };

    let plain = render_pixels(false);
    let memoed = render_pixels(true);
    let px = |buf: &[u8], x_pt: f32, y_pt: f32| -> [u8; 4] {
        let x = (x_pt * scale) as usize;
        let y = (y_pt * scale) as usize;
        let idx = (y * size.0 as usize + x) * 4;
        [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]]
    };

    for (label, buf) in [("plain", &plain), ("memo", &memoed)] {
        // Sample at x=160pt (to the right of the header's "Used 3 tools" text).
        let top_row = px(buf, 160.0, 1.5);
        let mid_row = px(buf, 160.0, 8.0);
        let bot_row = px(buf, 160.0, 20.0);
        // Problem 2: the top pixels of the stuck header (y = 1..2) must not be faded by
        // `scroll_edge_fade(16.0)`; they match the unfaded bottom of the header.
        assert_eq!(
            top_row, bot_row,
            "{label}: top of stuck header at y=1.5pt was faded ({top_row:?} vs {bot_row:?})"
        );
        assert_eq!(mid_row, bot_row, "{label}: middle of stuck header at y=8pt ({mid_row:?} vs {bot_row:?})");
        // Problem 1: the bright green scrolled-under row must not bleed through the red header fill
        // (whose g and b channels are equal).
        assert!(
            top_row[0] > 200 && top_row[1] == top_row[2] && top_row[3] == 255,
            "{label}: scrolled green row bled through stuck header: {top_row:?}"
        );
        // `stuck_shadow` darkens the row immediately below the 24pt header (y=26pt vs y=38pt).
        let under_shadow = px(buf, 160.0, 26.0);
        let clear_of_shadow = px(buf, 160.0, 38.0);
        assert!(
            under_shadow[1] < clear_of_shadow[1].saturating_sub(25),
            "{label}: stuck_shadow did not darken below header ({under_shadow:?} vs {clear_of_shadow:?})"
        );
    }
    // Problem 3: wrapping the row in `memo` produces identical pixels for the stuck header and shadow.
    assert_eq!(plain, memoed, "memoized sticky header pixels differ from unmemoized row");
}

#[test]
fn sticky_header_inside_memo_growth_row_respects_animated_row_height() {
    let mut rt = Runtime::new(
        OccludeState { memoized: true, open: true },
        occlude_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    // Scroll to 380pt so the bottom of the expanded group pushes the header near the top.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 380.0));
    rt.frame(0.0);
    // Collapse the group inside `memo`; on the first animation frame the outer `memo` wrapper row's
    // growth height `g.h` is shrinking, and the nested group's sticky bound must follow it.
    rt.update(|s, _| s.open = false);
    rt.frame(1.0 / 120.0);
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 120.0);
    let wrapper = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row-1")).unwrap();
    let head = find(&rt, "head");
    assert!(
        head.rect[1] + head.rect[3] <= wrapper.rect[1] + wrapper.rect[3] + 0.5,
        "sticky header bottom ({:?}) exceeded animated memo row bottom ({:?})",
        head.rect,
        wrapper.rect
    );
}

#[derive(Clone, Copy)]
struct ThreeColumns { rows: bool, alpha: f32 }

fn three_columns(s: &ThreeColumns, _: &mut Cx<ThreeColumns>) -> El {
    let mut root = row().size(700.0, 320.0).padding(20.0).gap(20.0).fill(Color::BLACK);
    for column in 0..3 {
        let state = *s;
        let section = move || {
            let mut section = col().key("section").w_full().child(
                row().key("header").w_full().h(24.0)
                    .fill(Color { r: 1.0, g: 0.0, b: 0.0, a: state.alpha })
                    .sticky_top(0.0).child(text("header").color(Color::WHITE)),
            );
            for i in 0..20 {
                section = section.child(row().h(24.0).child(
                    text(if state.rows { format!("row {i}") } else { String::new() })
                        .color(Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }),
                ));
            }
            section
        };
        let build = move |_: &ThreeColumns, i| match i {
            0 => el().w_full().h(10.0),
            1 if column == 0 => col().key("section-row").w_full().child(section()),
            1 => memo("section-row", 0, move || col().w_full().child(section())),
            _ => el().w_full().h(400.0),
        };
        let mut stack = col().w_full().pt(40.0).growth(GrowthSpec::default());
        if column != 1 {
            stack = stack.virtual_rows(3, 72.0, |_: &ThreeColumns, i| format!("r{i}").into(), build);
        } else {
            for i in 0..3 { stack = stack.child(build(s, i)); }
        }
        root = root.child(col().key(format!("scroll-{column}")).size(200.0, 280.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .scroll_edge_fade(if column == 2 { 0.0 } else { 16.0 }).child(stack));
    }
    root
}

#[test]
fn sticky_three_column_repro_orders_rows_before_opaque_or_translucent_headers() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut differences = Vec::new();
    for alpha in [1.0, 0.5] {
        let mut snapshots = Vec::new();
        for rows in [false, true] {
            let mut rt = Runtime::new(ThreeColumns { rows, alpha }, three_columns, (700.0, 320.0), Env::default());
            rt.frame(0.0);
            rt.update(|_, tx| {
                for column in 0..3 { tx.scroll_to_instant(format!("scroll-{column}"), 0.0, 150.0); }
            });
            for _ in 0..120 { rt.frame(1.0 / 120.0); }
            // Verify the stream itself, not just an opaque fill hiding a bad ordering.
            for column in 0..3 {
                let x = 20.0 + column as f32 * 220.0;
                let header = rt.instances.iter().position(|p| p.rect[0] == x && p.fill[0] > 0.0
                    && p.fill[1] == 0.0 && p.fill[2] == 0.0).unwrap();
                for (index, p) in rt.instances.iter().enumerate() {
                    if p.rect[0] >= x && p.rect[0] < x + 200.0 && p.fill[1] > 0.0 && p.fill[0] == 0.0 {
                        assert!(index < header, "column {column}, alpha {alpha}: row primitive {index} follows header {header}");
                    }
                }
            }
            snapshots.push(pixels(&gpu, &mut renderer, &mut rt, (700, 320)));
        }
        for column in 0..3 {
            let mut changed = 0;
            for y in 20..44 {
                for x in (20 + column * 220)..(220 + column * 220) {
                    let at = (y * 700 + x) * 4;
                    changed += usize::from(snapshots[0][at..at + 4] != snapshots[1][at..at + 4]);
                }
            }
            eprintln!("column {column}, alpha {alpha}: {changed} header pixels affected by underlying text");
            if alpha == 1.0 { differences.push(changed); }
        }
    }
    assert_eq!(differences, vec![0; 3], "opaque headers must occlude all three columns");
}

#[derive(Clone, Copy)]
struct FadeEdges { edge: usize, fade: f32, explicit: bool }

fn fade_edges(s: &FadeEdges, _: &mut Cx<FadeEdges>) -> El {
    let header = el().key("header").size(24.0, 24.0).fill(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
    let header = match s.edge {
        0 => header.absolute(40.0, 40.0).sticky_top(0.0),
        1 => header.absolute(40.0, 536.0).sticky_bottom(0.0),
        _ => header.absolute(40.0, 40.0).sticky_left(0.0),
    };
    let clip = if s.explicit { [12.0; 4] } else { [0.0; 4] };
    col().size(200.0, 200.0).fill(Color::BLACK).clip_fade(clip[0], clip[1], clip[2], clip[3]).child(
        col().key("scroll").size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false }).scroll_xy().scroll_edge_fade(s.fade)
            .child(col().size(600.0, 600.0).child(header)),
    )
}

#[test]
fn sticky_fade_exemption_is_continuous_on_each_axis_and_preserves_explicit_clips() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    for edge in 0..3 {
        for fade in [0.0, 16.0] {
            for explicit in [false, true] {
                let mut rt = Runtime::new(FadeEdges { edge, fade, explicit }, fade_edges, (200.0, 200.0), Env::default());
                rt.frame(0.0);
                // Approach, reach, then pass the sticky threshold in each direction. The
                // exemption is declarative, not gated on shift != 0: no opacity pop at 0.
                for distance in [20.0, 10.0, 1.0, 0.0, -1.0, -20.0] {
                    rt.update(move |_, tx| {
                        let offset = if edge == 1 { 360.0 + distance } else { 40.0 - distance };
                        tx.scroll_to_instant("scroll", if edge == 2 { offset } else { 0.0 }, if edge == 2 { 0.0 } else { offset });
                    });
                    rt.frame(0.0);
                    let header = find(&rt, "header");
                    if fade > 0.0 && !explicit {
                        let fill = rt.instances.iter().find(|p| p.fill[0] == 1.0 && p.fill[1] == 0.0).unwrap();
                        let widths = rune::render::unpack_fade(fill.fade).0;
                        let (sticking, opposite) = match edge { 0 => (1, 3), 1 => (3, 1), _ => (0, 2) };
                        assert_eq!(widths[sticking], 0.0);
                        assert_eq!(widths[opposite], 16.0, "only the sticking edge is exempt");
                    }
                    let (x, y) = match edge {
                        0 => (50, header.rect[1] as usize + 1),
                        1 => (50, header.rect[1] as usize + 22),
                        _ => (header.rect[0] as usize + 1, 50),
                    };
                    let image = pixels(&gpu, &mut renderer, &mut rt, (200, 200));
                    let red = image[(y * 200 + x) * 4];
                    if explicit && distance <= 1.0 {
                        assert!(red < 100, "explicit clip was erased: edge {edge}, fade {fade}, distance {distance}: {red}");
                    } else if !explicit {
                        assert_eq!(red, 255, "sticky edge faded or popped: edge {edge}, fade {fade}, distance {distance}");
                    }
                }
            }
        }
    }
}

#[test]
fn sticky_scroll_fade_preserves_an_intervening_explicit_clip() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(16.0)
            .child(col().w_full().clip_fade(0.0, 8.0, 0.0, 0.0)
                .child(el().key("header").w_full().h(24.0).fill(Color::WHITE).sticky_top(0.0))
                .child(el().w_full().h(600.0)))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 100.0));
    rt.frame(0.0);
    let fill = rt.instances.iter().find(|p| p.fill == [1.0; 4]).unwrap();
    assert_eq!(rune::render::unpack_fade(fill.fade).0[1], 8.0, "remove the scroll fade, not the explicit inner clip");
}

#[test]
fn sticky_header_taller_than_a_new_growth_row_keeps_its_natural_size() {
    fn view(s: &bool, _: &mut Cx<bool>) -> El {
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default())
            .child(el().key("lead").w_full().h(200.0));
        if *s {
            stack = stack.child(memo("new-row", 0, || col().w_full()
                .child(col().w_full().child(el().key("header").w_full().h(48.0).fill(Color::WHITE).sticky_top(0.0))
                    .child(el().w_full().h(200.0)))));
        }
        col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(stack.child(el().key("tail").w_full().h(400.0)))
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|s, tx| { *s = true; tx.scroll_to_instant("scroll", 0.0, 200.0); });
    rt.frame(1.0 / 120.0);
    let wrapper = find(&rt, "new-row");
    let header = find(&rt, "header");
    assert!(wrapper.rect[3] < header.rect[3], "exercise a row shorter than its header: {wrapper:?} {header:?}");
    assert_eq!(header.rect[3], 48.0, "bounds do not resize the header");
    assert_eq!(header.rect[1], wrapper.rect[1], "the growing row reveals the header in place rather than lifting it outside the row");
    let fill = rt.instances.iter().find(|p| p.fill == [1.0; 4]).unwrap();
    assert_eq!(fill.rect[3], 48.0, "paint keeps the header size; the existing growth mask owns its reveal");
}
