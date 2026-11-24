use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::el::{row, text};
use crate::{Color, Direction, Spring, TextStyle};

struct ColState {
    rows: Vec<&'static str>,
    hold: bool,
    builds: Arc<AtomicUsize>,
}

fn col_view(s: &ColState, _: &mut Cx<ColState>) -> El {
    s.builds.fetch_add(1, Ordering::Relaxed);
    col().w_full().children(s.rows.iter().map(|k| {
        let t = if s.hold { Transition::opacity().hold_layout() } else { Transition::opacity() };
        el().key(*k).w_full().h(44.0).fill(Color::WHITE).transition(t)
    }))
}

#[test]
fn vertical_row_hold_layout_collapses_in_lockstep_with_zero_per_frame_layout() {
    let run = |hold: bool| {
        let builds = Arc::new(AtomicUsize::new(0));
        let mut rt = Runtime::new(
            ColState { rows: vec!["a", "b", "c"], hold, builds: builds.clone() },
            col_view,
            (200.0, 200.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        rt.update(|s, _| s.rows.retain(|k| *k != "b"));
        rt.frame(1.0 / 60.0);
        let commits_after_remove = rt.stats.commits;
        let builds_after_remove = builds.load(Ordering::Relaxed);

        let mut samples = Vec::new();
        if let (Some((b, op)), Some((c, _))) = (rt.presented("b"), rt.presented("c")) {
            samples.push((b, c, op));
        }
        let mut settled_at = None;
        for f in 0..180 {
            let needs_more = rt.frame(1.0 / 60.0);
            assert_eq!(rt.stats.commits, commits_after_remove, "no layout pass during exit frame {f}");
            assert_eq!(builds.load(Ordering::Relaxed), builds_after_remove, "no view build during exit frame {f}");
            if let (Some((b, op)), Some((c, _))) = (rt.presented("b"), rt.presented("c")) {
                samples.push((b, c, op));
            }
            if !needs_more {
                settled_at = Some(f);
                break;
            }
        }
        assert!(settled_at.is_some(), "exit must settle and stop requesting frames");
        assert!(rt.presented("b").is_none(), "exiting node unmounts once settled");
        assert!(!rt.frame(1.0 / 60.0), "idle window after exit asks for no frames");
        samples
    };

    let pop = run(false);
    let hold = run(true);
    assert!(pop.len() >= 8 && hold.len() >= 8);

    // Default pop_layout keeps full 44 pt height while fading.
    for (b, _, _) in &pop {
        assert!((b[3] - 44.0).abs() < 0.01, "pop_layout keeps height 44, got {}", b[3]);
    }

    // hold_layout shrinks b's height smoothly in lockstep with c.y (zero overlap).
    let mut saw_shrunk = false;
    for &(b, c, _) in &hold {
        assert!(b[3] < 44.0, "hold_layout must shrink b's height, got {}", b[3]);
        assert!(b[1] + b[3] <= c[1] + 0.05, "b bottom {} overlapped c top {}", b[1] + b[3], c[1]);
        assert!((b[1] + b[3] - c[1]).abs() < 0.5, "b bottom {} should track c top {}", b[1] + b[3], c[1]);
        if b[3] < 20.0 {
            saw_shrunk = true;
        }
    }
    assert!(saw_shrunk, "hold_layout should collapse b well below half height during exit");
}

#[test]
fn horizontal_gap_and_flex_grow_collapse_without_overlap() {
    struct S {
        items: Vec<&'static str>,
        grow: bool,
        center: bool,
        rtl: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut r = row().w(360.0).h(40.0).gap(8.0);
        if s.center {
            r = r.justify_center();
        }
        if s.rtl {
            r = r.dir(Direction::Rtl);
        }
        r.children(s.items.iter().map(|k| {
            let mut item = el().key(*k).h(32.0).fill(Color::WHITE).transition(Transition::opacity().hold_layout());
            item = if s.grow { item.grow(1.0) } else { item.w(80.0) };
            item
        }))
    }

    // Fixed gap, flex_grow, justify_center, and RTL: remove middle item.
    for (grow, center, rtl) in [(false, false, false), (true, false, false), (false, true, false), (false, false, true)] {
        let mut rt = Runtime::new(
            S { items: vec!["a", "b", "c"], grow, center, rtl },
            view,
            (400.0, 80.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let init_w = rt.presented("b").unwrap().0[2];
        rt.update(|s, _| s.items.retain(|k| *k != "b"));
        let mut saw_collapse = false;
        for f in 0..20 {
            rt.frame(1.0 / 60.0);
            let Some((b, _)) = rt.presented("b") else { break };
            let (a, _) = rt.presented("a").unwrap();
            let (c, _) = rt.presented("c").unwrap();
            let (left, right) = if a[0] < c[0] { (a, c) } else { (c, a) };
            assert!(left[0] + left[2] <= b[0] + 0.1, "grow={grow} center={center} rtl={rtl} f={f}: left {:?} overlaps b {:?}", left, b);
            assert!(b[0] + b[2] <= right[0] + 0.1, "grow={grow} center={center} rtl={rtl} f={f}: b {:?} overlaps right {:?}", b, right);
            if b[2] < init_w * 0.5 {
                saw_collapse = true;
            }
        }
        assert!(saw_collapse, "grow={grow} center={center} rtl={rtl}: b should collapse");
    }

    // Simultaneous removal of two adjacent items [a, b, c, d] -> [a, d].
    let mut rt = Runtime::new(
        S { items: vec!["a", "b", "c", "d"], grow: true, center: false, rtl: false },
        view,
        (400.0, 80.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, _| s.items.retain(|k| *k != "b" && *k != "c"));
    for f in 0..20 {
        rt.frame(1.0 / 60.0);
        let (a, _) = rt.presented("a").unwrap();
        let (d, _) = rt.presented("d").unwrap();
        if let (Some((b, _)), Some((c, _))) = (rt.presented("b"), rt.presented("c")) {
            assert!(a[0] + a[2] <= b[0] + 0.1, "f={f}: a overlaps b");
            assert!(b[0] + b[2] <= c[0] + 0.1, "f={f}: b overlaps c");
            assert!(c[0] + c[2] <= d[0] + 0.1, "f={f}: c overlaps d");
        }
    }

    // Rapid sequential removal: remove b, advance 3 frames, then remove a (b's left neighbour!).
    let mut rt = Runtime::new(
        S { items: vec!["a", "b", "c", "d"], grow: false, center: false, rtl: false },
        view,
        (400.0, 80.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, _| s.items.retain(|k| *k != "b"));
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    rt.update(|s, _| s.items.retain(|k| *k != "a"));
    for f in 0..20 {
        rt.frame(1.0 / 60.0);
        let (c, _) = rt.presented("c").unwrap();
        if let Some((b, _)) = rt.presented("b") {
            // b was between a and c, so after a is removed b must stay left of c (never jump past d).
            assert!(b[0] + b[2] <= c[0] + 0.1, "f={f}: rapid sequential b {:?} overlapped c {:?}", b, c);
        }
        if let Some((a, _)) = rt.presented("a") {
            assert!(a[0] + a[2] <= c[0] + 0.1, "f={f}: rapid sequential a {:?} overlapped c {:?}", a, c);
        }
    }
}

#[test]
fn mid_exit_revival_springs_smoothly_back_open() {
    let builds = Arc::new(AtomicUsize::new(0));
    let mut rt = Runtime::new(
        ColState { rows: vec!["a", "b", "c"], hold: true, builds },
        col_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, _| s.rows.retain(|k| *k != "b"));
    for _ in 0..5 {
        rt.frame(1.0 / 60.0);
    }
    let (mid_b, mid_op) = rt.presented("b").expect("b still exiting");
    assert!(mid_b[3] > 5.0 && mid_b[3] < 38.0, "b should be mid-collapse, got h={}", mid_b[3]);
    assert!(mid_op > 0.1 && mid_op < 0.95, "b should be mid-fade, got op={mid_op}");

    // Revive b mid-exit.
    rt.update(|s, _| s.rows = vec!["a", "b", "c"]);
    rt.frame(1.0 / 60.0);
    let (after_b, _) = rt.presented("b").expect("b revived");
    // C1 continuity: height does not snap straight back to 44 on the first frame.
    assert!(after_b[3] < 42.0 && after_b[3] > 2.0, "revival should spring smoothly from {}, got {}", mid_b[3], after_b[3]);

    for _ in 0..90 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    let (final_b, final_op) = rt.presented("b").expect("b settled present");
    assert!((final_b[3] - 44.0).abs() < 0.1, "b should return to full height 44, got {}", final_b[3]);
    assert!((final_op - 1.0).abs() < 0.01, "b should return to full opacity, got {final_op}");
}

#[test]
fn absolute_and_wrapped_and_reduced_motion_keep_size_without_collapsing() {
    // 1. Out-of-flow absolute element ignores hold_layout().
    fn abs_view(shown: &bool, _: &mut Cx<bool>) -> El {
        col().size(200.0, 200.0).children(shown.then(|| {
            el().key("pop").absolute(20.0, 20.0).size(100.0, 50.0).fill(Color::WHITE).transition(Transition::opacity().hold_layout())
        }))
    }
    let mut rt = Runtime::new(true, abs_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = false);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
        if let Some((r, _)) = rt.presented("pop") {
            assert!((r[2] - 100.0).abs() < 0.01 && (r[3] - 50.0).abs() < 0.01, "absolute ignores hold_layout: {:?}", r);
        }
    }

    // 2. FlexWrap::Wrap falls back to pop_layout without collapsing.
    fn wrap_view(rows: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
        row().w(200.0).h(200.0).flex_wrap().children(rows.iter().map(|k| {
            el().key(*k).size(80.0, 40.0).fill(Color::WHITE).transition(Transition::opacity().hold_layout())
        }))
    }
    let mut rt = Runtime::new(vec!["a", "b", "c"], wrap_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.retain(|k| *k != "b"));
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
        if let Some((r, _)) = rt.presented("b") {
            assert!((r[2] - 80.0).abs() < 0.01 && (r[3] - 40.0).abs() < 0.01, "wrapped flex falls back to pop_layout: {:?}", r);
        }
    }

    // 3. Reduced motion keeps the exit fade without snapping height to 0.
    let builds = Arc::new(AtomicUsize::new(0));
    let mut rt = Runtime::new(
        ColState { rows: vec!["a", "b", "c"], hold: true, builds },
        col_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|s, _| s.rows.retain(|k| *k != "b"));
    let mut saw_fading = false;
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
        if let Some((b, op)) = rt.presented("b") {
            assert!((b[3] - 44.0).abs() < 0.01, "reduced motion keeps height 44 while fading, got {}", b[3]);
            if op > 0.0 && op < 1.0 {
                saw_fading = true;
            }
        }
    }
    assert!(saw_fading, "reduced motion must still fade the exiting row");
}

#[test]
fn collapsing_exiting_node_clips_own_text_and_children_to_collapsing_rect() {
    // 1. Collapsing container clips its children to its collapsing rect.
    struct S { rows: Vec<&'static str> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().w(200.0).children(s.rows.iter().map(|k| {
            let color = if *k == "b" { crate::oklch(0.63, 0.22, 27.0) } else { Color::WHITE };
            col()
                .key(*k)
                .w(200.0)
                .h(44.0)
                .motion(Spring::new(0.4, 0.0))
                .transition(Transition::opacity().hold_layout())
                .child(el().key(format!("{k}-child")).w(200.0).h(44.0).fill(color))
        }))
    }
    let mut rt = Runtime::new(S { rows: vec!["a", "b", "c"] }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.rows.retain(|k| *k != "b"));
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let (b, _) = rt.presented("b").expect("b still exiting");
    assert!(b[3] < 35.0, "b should be collapsing, got h={}", b[3]);
    let b_bottom = b[1] + b[3];
    // b-child has the non-white accent fill (fill[0] > fill[2] + 0.1).
    let child_clips: Vec<[f32; 4]> = rt
        .instances
        .iter()
        .filter(|inst| inst.params[1] == crate::render::KIND_RECT && inst.fill[0] > inst.fill[2] + 0.1)
        .map(|inst| inst.clip)
        .collect();
    assert!(!child_clips.is_empty(), "expected child rect instance for b-child");
    for clip in child_clips {
        assert!(clip[3] <= b_bottom + 1.0, "child clip bottom {} exceeded collapsing rect bottom {}", clip[3], b_bottom);
    }

    // 2. Collapsing leaf text node clips its own glyphs to its collapsing rect.
    fn leaf_view(s: &S, _: &mut Cx<S>) -> El {
        col().w(200.0).children(s.rows.iter().map(|k| {
            let color = if *k == "b" { crate::oklch(0.63, 0.22, 27.0) } else { Color::WHITE };
            text("Collapsing leaf")
                .key(*k)
                .w(200.0)
                .h(44.0)
                .font(TextStyle::new(14.0, 400.0))
                .color(color)
                .motion(Spring::new(0.4, 0.0))
                .transition(Transition::opacity().hold_layout())
        }))
    }
    let mut rt = Runtime::new(S { rows: vec!["a", "b", "c"] }, leaf_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.rows.retain(|k| *k != "b"));
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let (b, _) = rt.presented("b").expect("b leaf still exiting");
    assert!(b[3] < 35.0, "b leaf should be collapsing, got h={}", b[3]);
    let b_bottom = b[1] + b[3];
    let glyph_clips: Vec<[f32; 4]> = rt
        .instances
        .iter()
        .filter(|inst| inst.params[1] == crate::render::KIND_GLYPH && inst.fill[0] > inst.fill[2] + 0.05)
        .map(|inst| inst.clip)
        .collect();
    assert!(!glyph_clips.is_empty(), "expected glyph instances for collapsing leaf text");
    for clip in glyph_clips {
        assert!(clip[3] <= b_bottom + 1.0, "leaf glyph clip bottom {} exceeded collapsing rect bottom {}", clip[3], b_bottom);
    }
}

#[test]
fn hold_layout_motion_gate() {
    use crate::motion_gate::{Run, gate_rows};

    struct State {
        vert: Vec<&'static str>,
        horiz: Vec<&'static str>,
    }
    fn gate_view(s: &State, _: &mut Cx<State>) -> El {
        let vert = col().key("vert").w(240.0).gap(6.0).children(s.vert.iter().map(|&k| {
            row()
                .key(k)
                .w_full()
                .h(40.0)
                .px(12.0)
                .items_center()
                .radius(8.0)
                .fill(crate::oklch(0.30, 0.03, 250.0))
                .motion(Spring::new(0.35, 0.0))
                .transition(Transition::opacity().hold_layout())
                .child(text(k).font(TextStyle::new(14.0, 500.0)).color(Color::WHITE))
        }));
        let horiz = row().key("horiz").w(320.0).h(36.0).gap(8.0).children(s.horiz.iter().map(|&k| {
            row()
                .key(k)
                .grow(1.0)
                .h_full()
                .px(10.0)
                .items_center()
                .radius(8.0)
                .fill(crate::oklch(0.36, 0.05, 220.0))
                .motion(Spring::new(0.35, 0.0))
                .transition(Transition::opacity().hold_layout())
                .child(text(k).font(TextStyle::new(13.0, 500.0)).color(Color::WHITE))
        }));
        col().w_full().h_full().padding(16.0).gap(16.0).fill(crate::oklch(0.18, 0.01, 250.0)).children([vert, horiz])
    }

    let make = || {
        let rt = Runtime::new(
            State { vert: vec!["v-a", "v-b", "v-c"], horiz: vec!["h-a", "h-b", "h-c"] },
            gate_view,
            (380.0, 260.0),
            Env::new(true, 1.0),
        );
        let mut run = Run::new(rt, 120.0);
        run.idle(0.5);
        run
    };

    gate_rows(
        vec![
            ("hold-layout/vertical".to_owned(), Box::new(move || {
                let mut run = make();
                run.step_n(4);
                run.act("remove v-b", |rt| rt.update(|s, _| s.vert.retain(|x| *x != "v-b")));
                run.play(1.2);
                run.quiet();
                run.step_n(4);
                run.rec
            }) as crate::motion_gate::Job),
            ("hold-layout/horizontal-revive".to_owned(), Box::new(move || {
                let mut run = make();
                run.step_n(4);
                run.act("remove h-b", |rt| rt.update(|s, _| s.horiz.retain(|x| *x != "h-b")));
                run.play(0.12);
                run.act("revive h-b", |rt| rt.update(|s, _| s.horiz = vec!["h-a", "h-b", "h-c"]));
                run.play(1.2);
                run.quiet();
                run.step_n(4);
                run.rec
            }) as crate::motion_gate::Job),
        ],
        &[],
    );
}
