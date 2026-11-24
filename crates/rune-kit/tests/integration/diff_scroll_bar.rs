//! `DiffView::scroll_bar` and `DiffView::scroll_bar_styled`: opt-in vertical and dual-axis
//! scroll bars, sticky header clearance, `memo::layout` viewport-width tracking, wrap/split
//! switching, and zero view commits while scrolling.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rune::runtime::ElementInfo;
use rune::{Color, Cx, El, Env, Runtime, ScrollBars, col};
use rune_kit::diff::{DiffMode, DiffState, DiffView, FileDiff};
use rune_kit::{Kit, ScrollBarStyle};

enum BarCfg {
    Default,
    Off,
    Kit,
    Styled(ScrollBarStyle),
}

struct App {
    diff: DiffState,
    bar: BarCfg,
    runs: Arc<AtomicUsize>,
}

impl App {
    fn new(diff: DiffState, bar: BarCfg) -> Self {
        Self { diff, bar, runs: Arc::new(AtomicUsize::new(0)) }
    }
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    s.runs.fetch_add(1, Ordering::Relaxed);
    let k = Kit::neutral(true);
    let mut v = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff).key("diff");
    v = match s.bar {
        BarCfg::Default => v,
        BarCfg::Off => v.scroll_bar(false),
        BarCfg::Kit => v.scroll_bar(true),
        BarCfg::Styled(style) => v.scroll_bar_styled(style),
    };
    col().size(480.0, 260.0).child(v.build(s, &k))
}

fn wide_tall_diff() -> DiffState {
    let long = "let very_long_identifier_for_horizontal_overflow = compute_something_with_many_arguments(alpha, beta, gamma, delta, epsilon, zeta, eta);";
    let old: String = (1..=60).map(|i| format!("fn line_{i:02}() {{ {long} }}
")).collect();
    let new = old.replace("fn line_05()", "fn line_05_changed()").replace("fn line_40()", "fn line_40_changed()");
    let f1 = FileDiff::from_texts("src/wide.rs", &old, &new);
    let f2 = FileDiff::from_texts("src/second.rs", "a
b
c
", "a
B
c
");
    DiffState::new(vec![f1, f2])
}

fn frames<S>(rt: &mut Runtime<S>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

fn find_key<S>(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

fn find_suffix<S>(rt: &Runtime<S>, suffix: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.path.ends_with(suffix))
}

fn find_thumb<S>(rt: &Runtime<S>, track: &str) -> ElementInfo {
    let needle = format!("/{track}/thumb-slot/thumb");
    find_suffix(rt, &needle).unwrap_or_else(|| panic!("missing thumb under {track}"))
}

#[test]
fn default_and_disabled_diff_view_mount_no_scroll_bars() {
    for cfg in [BarCfg::Default, BarCfg::Off] {
        let mut rt = Runtime::new(App::new(wide_tall_diff(), cfg), view, (480.0, 260.0), Env::new(true, 1.0));
        rt.set_system_scroll_bars(ScrollBars::Always);
        frames(&mut rt, 3);
        assert!(find_key(&rt, "scroll-bar").is_none());
        assert!(find_key(&rt, "scroll-bar-x").is_none());
    }
}

#[test]
fn unwrapped_diff_view_scroll_bar_mounts_both_bars_clears_sticky_and_tracks_width() {
    let mut rt = Runtime::new(App::new(wide_tall_diff(), BarCfg::Kit), view, (480.0, 260.0), Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    rt.check_live_only(false);
    frames(&mut rt, 4);

    let v_track = find_key(&rt, "scroll-bar").expect("vertical bar mounted").rect;
    let h_track = find_key(&rt, "scroll-bar-x").expect("horizontal bar mounted").rect;
    assert!(v_track[3] > 100.0 && h_track[2] > 200.0, "both tracks overflow: v={v_track:?}, h={h_track:?}");
    // Vertical track starts below the 44 pt file header / sticky overlay and clears the bottom-right corner.
    assert!(v_track[1] >= 44.0, "vertical track starts below sticky header: {v_track:?}");
    assert!(v_track[1] + v_track[3] < h_track[1], "vertical clears horizontal: {v_track:?} vs {h_track:?}");
    assert!(h_track[0] + h_track[2] < v_track[0], "horizontal clears vertical: {h_track:?} vs {v_track:?}");

    // memo::layout still measured the wrapped scroll viewport's width, so the file header spans the viewport (480 pt).
    let header_before = find_key(&rt, "df0").expect("file 0 header").rect;
    assert!((header_before[2] - 480.0).abs() < 0.5, "unwrapped header spans viewport width: {header_before:?}");

    // Drag the horizontal thumb: scrolls code horizontally while keeping the header pinned at x = 0 and running zero view commits.
    let runs_before = rt.state.runs.load(Ordering::Relaxed);
    let ht = find_thumb(&rt, "scroll-bar-x").rect;
    let (hx, hy) = (ht[0] + ht[2] / 2.0, ht[1] + ht[3] / 2.0);
    rt.pointer_move(hx, hy);
    rt.pointer_down(hx, hy);
    rt.pointer_move(hx + 120.0, hy);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(hx + 120.0, hy);
    frames(&mut rt, 2);
    let header_after = find_key(&rt, "df0").expect("file 0 header after h-scroll").rect;
    assert!(header_after[0].abs() < 0.5, "header stays sticky_left(0) after horizontal thumb drag: {header_after:?}");
    assert_eq!(rt.state.runs.load(Ordering::Relaxed), runs_before, "horizontal thumb drag runs zero view commits");

    // Drag the vertical thumb down: scrolls down, mounts the sticky header at y = 0..44 above the vertical track.
    let vt = find_thumb(&rt, "scroll-bar").rect;
    let (vx, vy) = (vt[0] + vt[2] / 2.0, vt[1] + vt[3] / 2.0);
    rt.pointer_move(vx, vy);
    rt.pointer_down(vx, vy);
    rt.pointer_move(vx, vy + 80.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(vx, vy + 80.0);
    frames(&mut rt, 4);
    let sticky = find_key(&rt, "sticky").expect("sticky header mounted after vertical scroll").rect;
    assert!(sticky[1].abs() < 0.5 && (sticky[3] - 44.0).abs() < 0.5, "sticky at top: {sticky:?}");
    assert!(v_track[1] >= sticky[1] + sticky[3], "vertical track stays completely below sticky overlay");
    assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
}

#[test]
fn wrap_split_styled_and_empty_diff_view_scroll_bar_modes() {
    let mut rt = Runtime::new(App::new(wide_tall_diff(), BarCfg::Kit), view, (480.0, 260.0), Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 3);
    let v_dual_h = find_key(&rt, "scroll-bar").unwrap().rect[3];
    assert!(find_key(&rt, "scroll-bar-x").is_some());

    // Wrapping switches to vertical-only and reclaims the corner height (no stale corner clearance).
    rt.update(|s, _| s.diff.set_wrap(true));
    frames(&mut rt, 40);
    let v_wrap_h = find_key(&rt, "scroll-bar").expect("vertical bar in wrapped mode").rect[3];
    assert!(find_key(&rt, "scroll-bar-x").is_none(), "wrapped mode drops horizontal bar");
    assert!(v_wrap_h > v_dual_h + 10.0, "vertical bar reclaims corner clearance: dual={v_dual_h} wrap={v_wrap_h}");

    // Split mode always wraps and keeps vertical-only.
    rt.update(|s, _| {
        s.diff.set_wrap(false);
        s.diff.set_mode(DiffMode::Split);
    });
    frames(&mut rt, 40);
    assert!(find_key(&rt, "scroll-bar").is_some());
    assert!(find_key(&rt, "scroll-bar-x").is_none(), "split mode has no horizontal bar");

    // Back to unified unwrapped restores both bars with corner clearance.
    rt.update(|s, _| s.diff.set_mode(DiffMode::Unified));
    frames(&mut rt, 3);
    assert!(find_key(&rt, "scroll-bar").is_some());
    assert!(find_key(&rt, "scroll-bar-x").is_some());

    // scroll_bar_styled applies custom thumb width, trough fill, always_visible, and explicit track_offset.
    const TROUGH: Color = Color { r: 0.2, g: 0.25, b: 0.3, a: 1.0 };
    let custom = ScrollBarStyle::neutral(true)
        .always_visible()
        .width(8.0)
        .hover_width(8.0)
        .track(TROUGH)
        .track_offset(10.0, 0.0);
    let mut rt = Runtime::new(App::new(wide_tall_diff(), BarCfg::Styled(custom)), view, (480.0, 260.0), Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::WhenScrolling);
    frames(&mut rt, 3);
    let vt = find_key(&rt, "scroll-bar").unwrap();
    let ht = find_key(&rt, "scroll-bar-x").unwrap();
    assert_eq!((vt.opacity, ht.opacity), (1.0, 1.0), "always_visible keeps both bars shown under WhenScrolling");
    assert_eq!((vt.fill, ht.fill), (TROUGH, TROUGH));
    assert!((vt.rect[1] - 12.0).abs() < 0.5, "explicit track_offset(10, 0) + inset 2 is respected: {:?}", vt.rect);
    assert!((find_thumb(&rt, "scroll-bar").rect[2] - 8.0).abs() < 0.5);
    assert!((find_thumb(&rt, "scroll-bar-x").rect[3] - 8.0).abs() < 0.5);

    // Empty diff hides both thumbs cleanly.
    let mut rt = Runtime::new(App::new(DiffState::new(Vec::new()), BarCfg::Kit), view, (480.0, 260.0), Env::new(true, 1.0));
    rt.set_system_scroll_bars(ScrollBars::Always);
    frames(&mut rt, 3);
    assert_eq!(find_key(&rt, "scroll-bar").unwrap().rect[3], 0.0);
    assert_eq!(find_key(&rt, "scroll-bar-x").unwrap().rect[2], 0.0);
}
