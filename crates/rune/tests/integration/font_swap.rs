//! `Tx::set_font_families` swaps the families behind the sans and mono slots while the app
//! runs: text measures as a fresh launch with those families would, memoized subtrees build
//! again, and asking for the families already there rebuilds nothing.
use std::cell::Cell;

use rune::text::TextSystem;
use rune::{App, AppSpec, El, Env, Runtime, TextStyle, col, memo, row, text};

const INTER: &[u8] = include_bytes!("../../assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
const LAUNCH: (&str, &str) = ("Inter", "IBM Plex Mono");
static APP_FONTS: &[&[u8]] = &[INTER, PLEX];

thread_local! {
    static MEMO_BUILDS: Cell<u32> = const { Cell::new(0) };
}

const STYLE: TextStyle = TextStyle::new(14.0, 400.0);

fn view(_: &(), _: &mut rune::Cx<()>) -> El {
    let style = STYLE;
    col().children([
        row().child(text("Hamburgefonstiv 0123").key("label").font(style)),
        row().child(memo("card", 0, || {
            MEMO_BUILDS.set(MEMO_BUILDS.get() + 1);
            row().child(text("Memoized card title").key("memo-label").font(style))
        })),
        row().child(text("let mono = 42;").key("code").font(style.mono())),
    ])
}

fn launch(families: (&str, &str)) -> Runtime<()> {
    let text = TextSystem::new(&[INTER, PLEX], families);
    let mut rt = Runtime::with_text((), view, (600.0, 400.0), Env::new(true, 2.0), text);
    rt.frame(0.0);
    rt
}

fn widths(rt: &Runtime<()>) -> [f32; 3] {
    let elements = rt.elements();
    ["label", "memo-label", "code"].map(|key| {
        elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).target[2]
    })
}

fn switch(rt: &mut Runtime<()>, sans: &str, mono: &str) -> bool {
    let stale = rt.update_if(|_, tx| {
        tx.set_font_families(sans, mono);
        false
    });
    rt.frame(1.0 / 120.0);
    stale
}

#[test]
fn a_switch_measures_like_a_fresh_launch_and_rebuilds_memoized_subtrees() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let fresh = widths(&launch(("IBM Plex Mono", "Inter")));
    let mut rt = launch(LAUNCH);
    let before = widths(&rt);
    let builds = MEMO_BUILDS.get();
    assert!(switch(&mut rt, "IBM Plex Mono", "Inter"), "a switch asks for a frame");
    let after = widths(&rt);
    assert_eq!(rt.font_families(), ("IBM Plex Mono", "Inter"));
    for i in 0..3 {
        assert!((after[i] - before[i]).abs() > 1.0, "width {i} changed: {before:?} -> {after:?}");
    }
    assert_eq!(after, fresh, "matches a fresh launch with those families");
    assert_eq!(MEMO_BUILDS.get(), builds + 1, "the memoized subtree built again, once");
    assert!(rt.stats.shaped > 0, "text shaped again in the new families");
    let (w, _) = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None);
    assert!((w - after[0]).abs() < 0.51, "measure follows: {w} vs {}", after[0]);

    // And back again.
    switch(&mut rt, LAUNCH.0, LAUNCH.1);
    assert_eq!(widths(&rt), before);
}

#[test]
fn asking_for_the_families_already_there_rebuilds_nothing() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let mut rt = launch(LAUNCH);
    rt.frame(1.0 / 120.0);
    let (builds, commits) = (MEMO_BUILDS.get(), rt.stats.commits);
    assert!(!switch(&mut rt, LAUNCH.0, LAUNCH.1), "no frame asked for");
    assert_eq!(rt.stats.commits, commits, "no commit");
    assert_eq!(MEMO_BUILDS.get(), builds, "no memo rebuild");
    assert_eq!(rt.stats.shaped, 0, "nothing shaped");
}

#[test]
fn every_runtime_on_the_thread_follows() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let mut a = launch(LAUNCH);
    let mut b = launch(LAUNCH);
    let fresh = widths(&launch(("IBM Plex Mono", "IBM Plex Mono")));
    switch(&mut a, "IBM Plex Mono", "IBM Plex Mono");
    b.frame(1.0 / 120.0);
    assert_eq!(widths(&a), fresh);
    assert_eq!(widths(&b), fresh, "another window follows on its next frame");
}

#[test]
fn a_missing_family_falls_back_as_at_launch() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let fresh = widths(&launch(("Rune Missing Family", "Rune Missing Mono")));
    let mut rt = launch(LAUNCH);
    switch(&mut rt, "Rune Missing Family", "Rune Missing Mono");
    assert_eq!(widths(&rt), fresh);
    assert!(widths(&rt).iter().all(|w| *w > 0.0));
}

/// A window of an app launched in `LAUNCH`, as a window opens it, before its first frame.
fn open_window() -> Runtime<()> {
    let mut spec = AppSpec::new("fonts", (600.0, 400.0), || (), view);
    spec.fonts = APP_FONTS;
    spec.families = LAUNCH;
    App::new(spec).headless_runtime((600.0, 400.0), Env::new(true, 2.0))
}

#[test]
fn a_window_opened_after_a_switch_starts_in_the_new_families() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let switched = ("IBM Plex Mono", "Inter");
    let fresh = widths(&launch(switched));
    let switched_width = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None).0;
    let mut first = open_window();
    first.frame(0.0);
    let launch_width = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None).0;
    assert!((launch_width - switched_width).abs() > 1.0, "the families measure apart");
    switch(&mut first, switched.0, switched.1);
    let mut second = open_window();
    let (w, _) = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None);
    assert_eq!(w, switched_width, "measure stays in the chosen families before its first frame");
    assert_eq!(second.font_families(), switched, "the new window starts in the chosen families");
    second.frame(0.0);
    assert_eq!(widths(&second), fresh);
    assert_eq!(rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None).0, switched_width);
}

#[derive(Default)]
struct InspectorState {
    trunc_size: Option<[f32; 2]>,
    trunc_cut: bool,
    fade_size: Option<[f32; 2]>,
    fade_cut: bool,
}

fn inspector_view(_: &InspectorState, _: &mut rune::Cx<InspectorState>) -> El {
    col().gap(8.0).children([
        row().w(90.0).child(text("Long label that truncates cleanly").key("sample-trunc").font(STYLE).truncate()),
        row().w(90.0).child(text("Long label that fades cleanly").key("sample-fade").font(STYLE).truncate_fade()),
        row().child(text("iiiiiiiiii 0123456789\niiiiiiiiii 0123456789").key("by-line").font(STYLE).by_line()),
        row().key("inspect-btn").size(120.0, 28.0)
            .on_press_tx(|_: &mut InspectorState, tx| {
                tx.read_elements::<InspectorState>(|s, elements, _| {
                    if let Some(e) = elements.iter().find(|e| e.key.as_deref() == Some("sample-trunc")) {
                        s.trunc_size = e.text_size;
                        s.trunc_cut = e.text_cut;
                    }
                    if let Some(e) = elements.iter().find(|e| e.key.as_deref() == Some("sample-fade")) {
                        s.fade_size = e.text_size;
                        s.fade_cut = e.text_cut;
                    }
                });
            })
            .child(text("Inspect").font(STYLE)),
    ])
}

#[test]
fn pre_commit_read_elements_stays_consistent_and_by_line_reshapes_after_font_switch() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let mut peer = launch(LAUNCH);
    let mut b = Runtime::with_text(
        InspectorState::default(),
        inspector_view,
        (600.0, 400.0),
        Env::new(true, 2.0),
        TextSystem::new(&[INTER, PLEX], LAUNCH),
    );
    b.frame(0.0);
    let by_line_w = |rt: &Runtime<InspectorState>| {
        rt.elements().iter().find(|e| e.key.as_deref() == Some("by-line")).unwrap().target[2]
    };
    let before_by_line = by_line_w(&b);

    // Queue a press that reads the presented frame via Tx::read_elements, then switch families in
    // another runtime before b's next frame runs.
    let (x, y) = b.find("inspect-btn").unwrap();
    b.pointer_down(x, y);
    b.pointer_up(x, y);
    switch(&mut peer, "IBM Plex Mono", "Inter");

    // b's frame follows the font switch at frame start, runs the press + read_elements against
    // the presented tree, and then commits the new font.
    b.frame(1.0 / 60.0);
    assert!(b.state.trunc_size.is_some(), "pre-commit read_elements must keep presented text_size for truncated text");
    assert!(b.state.trunc_cut, "pre-commit read_elements must keep presented text_cut for truncated text");
    assert!(b.state.fade_size.is_some(), "pre-commit read_elements must keep presented text_size for truncate_fade text");
    assert!(b.state.fade_cut, "pre-commit read_elements must keep presented text_cut for truncate_fade text");

    // After the commit, by_line (ParagraphLayout) must have re-shaped in the new font.
    let mut fresh = Runtime::with_text(
        InspectorState::default(),
        inspector_view,
        (600.0, 400.0),
        Env::new(true, 2.0),
        TextSystem::new(&[INTER, PLEX], ("IBM Plex Mono", "Inter")),
    );
    fresh.frame(0.0);
    let after_by_line = by_line_w(&b);
    assert_eq!(after_by_line, by_line_w(&fresh), "by_line must re-shape to match fresh launch");
    assert!((after_by_line - before_by_line).abs() > 10.0, "by_line width changed: {before_by_line} -> {after_by_line}");

    // Latching a family switch via update_if must also keep presented elements consistent before the next frame.
    assert!(b.update_if(|_, tx| {
        tx.set_font_families(LAUNCH.0, LAUNCH.1);
        false
    }));
    let pre = b.elements();
    let fade = pre.iter().find(|e| e.key.as_deref() == Some("sample-fade")).unwrap();
    assert!(fade.text_size.is_some() && fade.text_cut, "elements() before next frame must not see wiped text_layout");
}
