//! The shell set up as a desktop agent-chat app lays it out: a framed main pane with the
//! toolbar inside it, a padded sidebar whose contents slide with its edge, one spring for both
//! panes with a late fade, the closed sidebar's peek, the toggle at the header's end, and tall
//! rows with a status slot, a hover close button and drag to reorder.

use std::time::Duration;

use rune::{Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Runtime, Spring, col, el};
use rune_kit::shell::{PaneMotion, PaneState, Reorder, RowStatus, Sidebar, SidebarPeek, SidebarRow, SidebarToggle, SplitView, Toolbar};
use rune_kit::{ControlSize, Kit};

const ROWS: usize = 5;

struct S {
    sidebar: PaneState,
    inspector: PaneState,
    order: Vec<usize>,
    reorder: Reorder,
    picked: usize,
    closed: Vec<usize>,
    reduced: bool,
}

fn state() -> S {
    S {
        sidebar: PaneState::new(276.0).range(136.0, 640.0),
        inspector: PaneState::new(300.0),
        order: (0..ROWS).collect(),
        reorder: Reorder::new().gap(4.0),
        picked: 0,
        closed: Vec::new(),
        reduced: false,
    }
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let rows = s.order.iter().enumerate().map(|(i, &id)| {
        let mut r = SidebarRow::new(format!("Session {id}"))
            .height(45.0)
            .avatar(el().size(16.0, 16.0).radius(4.0).fill(k.c.bg_accent))
            .subtitle("project")
            .status(if id == 0 { RowStatus::Busy } else { RowStatus::Unread })
            .on_close(move |s: &mut S, _| s.closed.push(id))
            .on_select(move |s: &mut S, _| s.picked = id)
            .reorder(&s.reorder, i, s.order.len(), |s: &mut S| &mut s.reorder, |s: &mut S, from, to| {
                let id = s.order.remove(from);
                s.order.insert(to, id);
            });
        r = r.selected(s.picked == id);
        r.build(&k).key(format!("row{id}"))
    });
    let sidebar = Sidebar::new().inset(0.0).header_gap(4.0).header_padding(8.0, 16.0).row_gap(4.0).header(SidebarRow::new("Home").build(&k).key("home"));
    let sidebar = rows.fold(sidebar, |sb, r| sb.child(r));
    SplitView::new(col().key("body").w_full().h_full())
        .sidebar(&s.sidebar, |s: &mut S| &mut s.sidebar, sidebar.build(&k))
        .inspector(&s.inspector, |s: &mut S| &mut s.inspector, col().key("details"))
        .toolbar(Toolbar::new().leading(el().key("lead").size(20.0, 20.0)).title("Title"))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(18.0, 10.0)
        .sidebar_toggle(SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).edge(18.0).gap(4.0).in_header().unread(true))
        .pane_motion(PaneMotion::new().spring(Spring::new(0.3, 0.0)).scale_with_width(480.0, 0.15, 0.3).fade_tail(0.15))
        .sidebar_peek(SidebarPeek::new().edge(16.0, ms(40)).toggle_delay(ms(80)).below(56.0).hold(15.0, ms(100)).spring(Spring::new(0.18, 0.0)))
        .reduced_motion(s.reduced)
        .build(&k)
}

fn runtime(s: S) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (1000.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn settle(rt: &mut Runtime<S>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no {key}"))
}

/// The first element keyed `key` inside the element keyed `parent`.
fn inside(rt: &Runtime<S>, parent: &str, key: &str) -> ElementInfo {
    let all = rt.elements();
    let p = all.iter().position(|e| e.key.as_deref() == Some(parent) && !e.exiting).unwrap_or_else(|| panic!("no {parent}"));
    let within = |mut i: usize| {
        while let Some(up) = all[i].parent {
            if up == p {
                return true;
            }
            i = up;
        }
        false
    };
    let i = (0..all.len()).find(|&i| all[i].key.as_deref() == Some(key) && within(i)).unwrap_or_else(|| panic!("no {key} in {parent}"));
    all[i].clone()
}

fn closed() -> S {
    let mut s = state();
    s.sidebar.set_open(false);
    s
}

#[test]
fn the_main_pane_is_framed_and_the_toolbar_sits_inside_it() {
    let mut rt = runtime(state());
    let card = find(&rt, "main-pane").rect;
    let inspector = find(&rt, "inspector-pane").rect;
    assert_eq!((card[0], card[1]), (276.0, 8.0), "beside the sidebar, the frame inset from the top");
    assert_eq!(card[1] + card[3], 592.0, "and from the bottom");
    assert_eq!(card[0] + card[2], inspector[0], "up to the inspector");
    let toolbar = find(&rt, "toolbar").rect;
    assert_eq!((toolbar[1], toolbar[3]), (8.0, 48.0), "the toolbar is the pane's top 48 points");
    let (x, y) = (500.0, 4.0);
    rt.pointer_down(x, y);
    assert_eq!(rt.take_titlebar_action(), Some(rune::TitlebarAction::Drag), "the strip above the pane drags the window");
    rt.pointer_up(x, y);

    rt.update(|s, tx| {
        tx.instant();
        s.sidebar.set_open(false);
        s.inspector.set_open(false);
    });
    rt.frame(1.0 / 60.0);
    let card = find(&rt, "main-pane").rect;
    assert_eq!((card[0], card[1], card[0] + card[2]), (8.0, 8.0, 992.0), "inset on every side with both panes closed");
}

#[test]
fn the_sidebar_pads_its_contents_and_they_slide_with_its_edge() {
    let mut rt = runtime(state());
    let home = find(&rt, "home").rect;
    assert_eq!(home[0], 18.0, "padded 18 at the start");
    assert_eq!(home[0] + home[2], 266.0, "and 10 at the end");
    rt.update(|s, _| s.sidebar.set_open(false));
    for _ in 0..12 {
        rt.frame(1.0 / 60.0);
        let pane = find(&rt, "sidebar-pane").rect;
        let content = find(&rt, "pane-content").rect;
        assert!((content[0] + content[2] - (pane[0] + pane[2])).abs() < 0.5, "pinned to the moving edge: {content:?} in {pane:?}");
        assert_eq!(content[2], 276.0, "at full width");
    }
}

#[test]
fn both_panes_share_one_spring_scaled_by_width_and_the_contents_fade_late() {
    let mut rt = runtime(state());
    rt.update(|s, _| {
        s.sidebar.set_open(false);
        s.inspector.set_open(false);
    });
    let mut frames = 0;
    let mut faded_early = false;
    loop {
        rt.frame(1.0 / 120.0);
        frames += 1;
        let side = find(&rt, "sidebar-pane").rect[2] / 276.0;
        let insp = find(&rt, "inspector-pane").rect[2] / 300.0;
        assert!((side - insp).abs() < 0.01, "frame {frames}: both panes move as one ({side} vs {insp})");
        let opacity = inside(&rt, "sidebar-pane", "pane-content").opacity;
        let expected = (side / 0.15).min(1.0);
        assert!((opacity - expected).abs() < 0.06, "frame {frames}: opacity {opacity} for {side} open");
        faded_early |= side > 0.3 && opacity < 0.99;
        if side < 0.001 || frames > 120 {
            break;
        }
    }
    assert!(!faded_early);
    // 0.3 s × sqrt(276 / 480) ≈ 0.23 s: settled (within a pixel) well before 0.3 s's frames.
    assert!(frames < 50, "{frames} frames at 120 Hz");
}

#[test]
fn the_toggle_sits_at_the_headers_end_and_floats_past_the_lights_when_closed() {
    let mut rt = runtime(state());
    let inset = 74.0;
    rt.set_titlebar_inset(inset);
    rt.frame(1.0 / 60.0);
    let t = find(&rt, "sidebar-toggle").rect;
    assert_eq!((t[0], t[1], t[2]), (276.0 - 10.0 - 28.0, 8.0 + 10.0, 28.0), "at the header's end, on the toolbar's centre line");
    assert_eq!(find(&rt, "toolbar").rect[1] + 24.0, t[1] + 14.0);
    rt.set_system_reduced_motion(true);
    rt.update(|s, _| s.sidebar.set_open(false));
    rt.frame(1.0 / 60.0);
    let t = find(&rt, "sidebar-toggle").rect;
    assert_eq!(t[0], inset + 16.0, "past the lights");
    assert_eq!(find(&rt, "lead").rect[0], inset + 16.0 + 28.0 + 4.0, "the toolbar starts past the toggle");
}

/// The pointer rests at the window's edge; returns the runtime with the sidebar peeking.
fn peeked() -> Runtime<S> {
    let mut rt = runtime(closed());
    settle(&mut rt);
    rt.pointer_move(500.0, 300.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(12.0, 300.0);
    rt.frame(0.03);
    assert!(!rt.state.sidebar.is_peeking(), "not before 40 ms");
    rt.frame(0.02);
    assert!(rt.state.sidebar.is_peeking(), "after resting 40 ms");
    rt
}

#[test]
fn resting_at_the_edge_slides_the_sidebar_over_the_content_and_leaving_closes_it() {
    let mut rt = peeked();
    let card = find(&rt, "main-pane").rect;
    rt.frame(1.0 / 60.0);
    let x = find(&rt, "pane-content").rect[0];
    assert!(x > -276.0 && x < 0.0, "sliding in: {x}");
    settle(&mut rt);
    assert_eq!(find(&rt, "pane-content").rect[0], 0.0, "at the window's edge");
    assert_eq!(find(&rt, "main-pane").rect, card, "over the content, which stays put");
    assert_eq!(find(&rt, "pane-surface").opacity, 1.0);
    // It floats above the main pane: a press on it reaches its row.
    let (rx, ry) = rt.find("row2").unwrap();
    assert!(rx < 276.0);
    rt.pointer_move(rx, ry);
    rt.pointer_down(rx, ry);
    rt.pointer_up(rx, ry);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.picked, 2, "rows in the peek take presses");
    assert!(rt.state.sidebar.is_peeking());

    // Straying within the margin keeps it; past it, it closes after the grace.
    rt.pointer_move(276.0 + 10.0, 300.0);
    rt.frame(0.2);
    assert!(rt.state.sidebar.is_peeking(), "within 15 points");
    rt.pointer_move(600.0, 300.0);
    rt.frame(0.09);
    assert!(rt.state.sidebar.is_peeking(), "within the grace");
    rt.frame(0.02);
    assert!(!rt.state.sidebar.is_peeking() && !rt.state.sidebar.is_open());
    settle(&mut rt);
    assert!(find(&rt, "pane-content").rect[0] + 276.0 <= 0.5, "slid out");
    assert_eq!(find(&rt, "pane-surface").opacity, 0.0);
}

#[test]
fn the_title_bar_band_and_a_quick_pass_dont_peek() {
    let mut rt = runtime(closed());
    settle(&mut rt);
    rt.pointer_move(12.0, 40.0);
    rt.frame(0.2);
    assert!(!rt.state.sidebar.is_peeking(), "the band holds the lights and the toggle");
    rt.pointer_move(12.0, 300.0);
    rt.frame(0.02);
    rt.pointer_move(400.0, 300.0);
    rt.frame(0.2);
    assert!(!rt.state.sidebar.is_peeking(), "a pass shorter than the delay");
}

#[test]
fn resting_on_the_toggle_peeks_and_pressing_it_docks_in_place() {
    let mut rt = runtime(closed());
    settle(&mut rt);
    let (x, y) = rt.find("sidebar-toggle").unwrap();
    rt.pointer_move(x, y);
    rt.frame(0.07);
    assert!(!rt.state.sidebar.is_peeking(), "not before 80 ms");
    rt.frame(0.02);
    assert!(rt.state.sidebar.is_peeking());
    settle(&mut rt);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for i in 0..40 {
        rt.frame(1.0 / 120.0);
        if i == 0 {
            assert!(rt.state.sidebar.is_open() && !rt.state.sidebar.is_peeking(), "docked");
        }
        let x = find(&rt, "pane-content").rect[0];
        assert!(x.abs() < 0.5, "the contents hold still while the sidebar docks: {x}");
    }
    settle(&mut rt);
    assert_eq!(find(&rt, "main-pane").rect[0], 276.0, "and the main pane made room");
}

#[test]
fn escape_and_blur_end_a_peek() {
    let mut rt = peeked();
    rt.update(|s, _| s.sidebar.end_peek());
    assert!(!rt.state.sidebar.is_peeking());
    let mut rt = peeked();
    rt.window_focus(false);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.sidebar.is_peeking(), "the window losing focus ends it");
}

#[test]
fn under_reduce_motion_a_peek_fades_in_place() {
    let mut rt = runtime(S { reduced: true, ..closed() });
    rt.set_system_reduced_motion(true);
    settle(&mut rt);
    rt.pointer_move(12.0, 300.0);
    rt.frame(0.05);
    assert!(rt.state.sidebar.is_peeking());
    rt.frame(1.0 / 60.0);
    let c = find(&rt, "pane-content");
    assert_eq!(c.rect[0], 0.0, "no slide");
    assert!(c.opacity > 0.0 && c.opacity < 1.0, "a fade: {}", c.opacity);
}

#[test]
fn tall_rows_lay_out_two_lines_and_swap_status_for_close_on_hover() {
    let mut rt = runtime(state());
    settle(&mut rt);
    let row = find(&rt, "row1").rect;
    assert_eq!(row[3], 45.0);
    let avatar = inside(&rt, "row1", "avatar").rect;
    let label = inside(&rt, "row1", "label").rect;
    let sub = inside(&rt, "row1", "subtitle").rect;
    assert_eq!(label[0], avatar[0] + 16.0 + 6.0, "the label follows the avatar");
    assert_eq!(sub[0], label[0], "the subtitle sits under the label");
    assert!(sub[1] >= label[1] + label[3] - 0.5);
    let slot = inside(&rt, "row1", "status").rect;
    assert_eq!((slot[2], slot[3]), (20.0, 20.0));
    let mark = |rt: &Runtime<S>| (inside(rt, "row1", "status-mark").opacity, inside(rt, "row1", "close").opacity);
    assert_eq!(mark(&rt), (1.0, 0.0), "the status shows at rest");
    rt.pointer_move(row[0] + 40.0, row[1] + 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(mark(&rt), (0.0, 1.0), "the close button, at once");
    let (cx, cy) = (slot[0] + 10.0, slot[1] + 10.0);
    rt.pointer_move(cx, cy);
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.closed.as_slice(), rt.state.picked), (&[1][..], 0), "closes without selecting");
}

#[test]
fn dragging_a_row_lifts_it_reflows_the_others_and_escape_puts_it_back() {
    let mut rt = runtime(state());
    settle(&mut rt);
    let y0 = |rt: &Runtime<S>, k: &str| find(rt, k).rect[1];
    let (r0, r1, r2, r3) = (y0(&rt, "row0"), y0(&rt, "row1"), y0(&rt, "row2"), y0(&rt, "row3"));
    let pitch = r1 - r0;
    assert_eq!(pitch, 49.0);
    let (x, y) = rt.find("row0").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 2.0 * pitch + 4.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.target(), Some(2));
    assert_eq!(y0(&rt, "row0"), r0 + 2.0 * pitch + 4.0, "follows the pointer");
    rt.frame(1.0 / 60.0);
    let mid = y0(&rt, "row1");
    assert!(mid < r1 && mid > r0, "the others spring aside: {mid}");
    settle(&mut rt);
    assert_eq!((y0(&rt, "row1"), y0(&rt, "row2"), y0(&rt, "row3")), (r0, r1, r3));
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.reorder.dragging(), None);
    assert_eq!(rt.state.order, [0, 1, 2, 3, 4], "Escape cancels");
    settle(&mut rt);
    assert_eq!((y0(&rt, "row0"), y0(&rt, "row1"), y0(&rt, "row2")), (r0, r1, r2), "back in place");
    rt.pointer_up(x, y);

    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 2.0 * pitch);
    rt.frame(1.0 / 60.0);
    let drawn = y0(&rt, "row0");
    rt.pointer_up(x, y + 2.0 * pitch);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.order, [1, 2, 0, 3, 4], "a drop moves it");
    let mut last = drawn;
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
        let now = y0(&rt, "row0");
        assert!((now - last).abs() < 20.0 && now <= r2 + 0.5, "it settles into its slot without a jump: {last} -> {now}");
        last = now;
    }
    assert_eq!(last, r2);
    assert_eq!(rt.state.picked, 0, "a drag isn't a select");
}

#[test]
fn the_unread_dot_fades_on_a_tween_that_ends_with_it() {
    let mut rt = runtime(closed());
    settle(&mut rt);
    assert_eq!(find(&rt, "sidebar-toggle-unread").opacity, 1.0, "shown while the sidebar is closed");
    rt.update(|s, _| s.sidebar.set_open(true));
    for _ in 0..19 {
        rt.frame(1.0 / 120.0);
    }
    let dot = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("sidebar-toggle-unread"));
    assert!(dot.as_ref().is_none_or(|d| d.opacity == 0.0), "gone at 0.16 s, while the pane still moves: {dot:?}");
    rt.update(|s, _| s.sidebar.set_open(false));
    settle(&mut rt);
    assert_eq!(find(&rt, "sidebar-toggle-unread").opacity, 1.0, "and back");
}

#[test]
fn the_hidden_peek_surface_takes_a_new_width_at_once() {
    let mut rt = runtime(state());
    settle(&mut rt);
    rt.update(|s, _| s.sidebar.set_width(400.0));
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let content = inside(&rt, "sidebar-pane", "pane-content").rect[2];
    assert!(content < 399.0, "the contents spring to the new width: {content}");
    let surface = find(&rt, "pane-surface");
    assert_eq!(surface.opacity, 0.0);
    assert!((surface.rect[2] - 400.0).abs() < 0.5, "unseen, it doesn't animate: {}", surface.rect[2]);
}

#[test]
fn cancelling_a_resize_restores_the_width_and_open_state_without_peeking() {
    for blur in [false, true] {
        let mut rt = runtime(state());
        settle(&mut rt);
        let (x, y) = rt.find("sidebar-grip").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_move(12.0, y);
        rt.frame(0.1);
        assert!(!rt.state.sidebar.is_peeking(), "a held resize never peeks");
        if blur { rt.window_focus(false); } else { rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }); }
        rt.frame(0.0);
        assert_eq!(rt.state.sidebar.width(), 276.0);
        assert!(rt.state.sidebar.is_open(), "blur={blur}");
        assert!(!rt.state.sidebar.is_resizing());
    }
}

#[test]
fn a_pending_peek_does_not_open_after_blur() {
    let mut rt = runtime(closed());
    settle(&mut rt);
    rt.pointer_move(12.0, 300.0);
    rt.frame(0.02);
    rt.window_focus(false);
    rt.frame(0.2);
    assert!(!rt.state.sidebar.is_peeking());
}
