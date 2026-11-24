//! SplitView::flat_main, Page::flat_main, and localized SidebarToggle::labels.

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el};
use rune_kit::Kit;
use rune_kit::shell::{Page, PaneState, SidebarToggle, SplitView, Toolbar, is_sidebar_toggle};

struct S {
    sidebar: PaneState,
    inspector: PaneState,
    with_sidebar: bool,
    with_inspector: bool,
    with_toolbar: bool,
    in_panel: bool,
    flat: bool,
    frame_inset: Option<f32>,
    inset_first: bool,
    rtl: bool,
    toggle: SidebarToggle,
    overflow_probe: bool,
}

impl Default for S {
    fn default() -> Self {
        Self {
            sidebar: PaneState::new(240.0).range(180.0, 420.0),
            inspector: PaneState::new(220.0).range(180.0, 400.0),
            with_sidebar: true,
            with_inspector: true,
            with_toolbar: true,
            in_panel: false,
            flat: false,
            frame_inset: None,
            inset_first: false,
            rtl: false,
            toggle: SidebarToggle::new(),
            overflow_probe: false,
        }
    }
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let mut main = col().key("body").w_full().h_full();
    if s.overflow_probe {
        // Sits 10 points outside main-pane's top-left corner to detect rounded clipping on main-pane.
        main = main.child(el().key("probe").absolute(-10.0, -10.0).size(20.0, 20.0));
    }
    let mut split = SplitView::new(main).sidebar_toggle(s.toggle).reduced_motion(true);
    if s.with_sidebar {
        split = split.sidebar(&s.sidebar, |s: &mut S| &mut s.sidebar, col().key("side-body"));
    }
    if s.with_inspector {
        split = split.inspector(&s.inspector, |s: &mut S| &mut s.inspector, col().key("insp-body"));
    }
    if s.with_toolbar {
        split = split.toolbar(Toolbar::new().title("Workspace"));
    }
    if s.in_panel {
        split = split.toolbar_in_panel();
    }
    if s.inset_first {
        if let Some(inset) = s.frame_inset {
            split = split.frame_inset(inset);
        }
        split = split.flat_main(s.flat);
    } else {
        split = split.flat_main(s.flat);
        if let Some(inset) = s.frame_inset {
            split = split.frame_inset(inset);
        }
    }
    let root = split.build(&k);
    if s.rtl { root.dir(rune::Direction::Rtl) } else { root }
}

fn runtime(s: S) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (1000.0, 600.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.on_key = Some(|s, ev, _| {
        let hit = is_sidebar_toggle(ev);
        if hit {
            s.sidebar.toggle();
        }
        hit
    });
    rt.frame(0.0);
    rt
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
        .unwrap_or_else(|| panic!("no {key}"))
}

fn premul(srgb: bool, c: rune::Color) -> [f32; 4] {
    if srgb { c.premul_srgb(1.0) } else { c.premul(1.0) }
}

fn main_pane_instance(rt: &Runtime<S>, k: &Kit) -> (f32, [f32; 4], [f32; 4], [f32; 4]) {
    let r = find(rt, "main-pane").rect;
    let fill = premul(rt.srgb_blend(), k.c.bg_base);
    let inst = rt
        .instances
        .iter()
        .find(|i| i.fill == fill && (0..4).all(|idx| (i.rect[idx] - r[idx]).abs() < 0.5))
        .unwrap_or_else(|| panic!("no painted instance for main-pane at {r:?}"));
    (inst.params[0], inst.uv, inst.radii, inst.border)
}

#[test]
fn default_split_view_keeps_rounded_card_inset_and_full_border() {
    let k = Kit::neutral(true);
    let mut s = S::default();
    s.sidebar = PaneState::new(240.0).collapsed();
    s.inspector = PaneState::new(220.0).collapsed();
    s.overflow_probe = true;
    let rt = runtime(s);

    let pane = find(&rt, "main-pane");
    assert_eq!(pane.radius, k.radius.lg, "default main pane stays rounded");
    assert_eq!(pane.rect, [8.0, 48.0, 984.0, 544.0], "default main pane keeps 8pt frame inset");

    let (bw, uv, radii, border) = main_pane_instance(&rt, &k);
    assert_eq!(bw, 1.0, "default main pane draws a 1pt border");
    assert_eq!(uv, [0.0; 4], "uniform 4-sided border has zero per-side uv override");
    assert_eq!(radii, [k.radius.lg; 4], "all four corners use k.radius.lg");
    assert_eq!(border, premul(rt.srgb_blend(), k.c.border_muted));

    let probe = find(&rt, "probe");
    assert!(probe.visible[0] >= pane.rect[0] - 0.01, "default rounded card clips children at its edge");
}

#[test]
fn flat_main_removes_radius_clip_and_inset_and_borders_only_pane_edges() {
    let k = Kit::neutral(true);
    let mut rt = runtime(S { flat: true, overflow_probe: true, ..S::default() });
    let border_color = premul(rt.srgb_blend(), k.c.border_muted);

    // Both sidebar and inspector open + outer toolbar: hairlines on top, right, left; none on bottom.
    let pane = find(&rt, "main-pane");
    assert_eq!(pane.radius, 0.0, "flat_main removes corner radius");
    assert_eq!(pane.rect, [240.0, 48.0, 540.0, 552.0], "flat_main defaults frame_inset to 0");
    let (bw, uv, radii, border) = main_pane_instance(&rt, &k);
    assert_eq!((bw, uv, radii, border), (1.0, [1.0, 1.0, 0.0, 1.0], [0.0; 4], border_color));

    // No rounded clip on main-pane: the probe extending 10pt above/left of main-pane stays unclipped by main-pane.
    let probe = find(&rt, "probe");
    assert!(probe.visible[0] < pane.rect[0] - 5.0, "flat_main does not clip children to main-pane: {probe:?}");

    // Collapse sidebar: left hairline disappears and main-pane sits flush at x = 0.
    rt.update(|s, _| s.sidebar.set_open(false));
    rt.frame(1.0 / 60.0);
    let pane = find(&rt, "main-pane");
    assert_eq!(pane.rect, [0.0, 48.0, 780.0, 552.0]);
    let (bw, uv, _, _) = main_pane_instance(&rt, &k);
    assert_eq!((bw, uv), (1.0, [1.0, 1.0, 0.0, 0.0]), "closing sidebar removes left hairline");

    // Collapse inspector too: only the top hairline under the toolbar stays.
    rt.update(|s, _| s.inspector.set_open(false));
    rt.frame(1.0 / 60.0);
    let pane = find(&rt, "main-pane");
    assert_eq!(pane.rect, [0.0, 48.0, 1000.0, 552.0]);
    let (bw, uv, _, _) = main_pane_instance(&rt, &k);
    assert_eq!((bw, uv), (1.0, [1.0, 0.0, 0.0, 0.0]), "closing inspector leaves only top hairline");

    // Without a toolbar and with both side panes closed: no border at all.
    rt.update(|s, _| s.with_toolbar = false);
    rt.frame(1.0 / 60.0);
    let (bw, uv, _, _) = main_pane_instance(&rt, &k);
    assert_eq!((bw, uv), (0.0, [0.0; 4]), "no open panes and no toolbar draws no borders");

    // With toolbar_in_panel and both panes open: left and right hairlines only, no top hairline.
    rt.update(|s, _| {
        s.with_toolbar = true;
        s.in_panel = true;
        s.sidebar.set_open(true);
        s.inspector.set_open(true);
    });
    rt.frame(1.0 / 60.0);
    let pane = find(&rt, "main-pane");
    assert_eq!(pane.rect, [240.0, 0.0, 540.0, 600.0], "toolbar_in_panel with flat_main is flush to top and bottom");
    let (bw, uv, _, _) = main_pane_instance(&rt, &k);
    assert_eq!((bw, uv), (1.0, [0.0, 1.0, 0.0, 1.0]), "toolbar_in_panel omits top hairline");

    // With toolbar_in_panel and no open panes: full window, zero borders.
    rt.update(|s, _| {
        s.sidebar.set_open(false);
        s.inspector.set_open(false);
    });
    rt.frame(1.0 / 60.0);
    let pane = find(&rt, "main-pane");
    assert_eq!(pane.rect, [0.0, 0.0, 1000.0, 600.0]);
    let (bw, uv, _, _) = main_pane_instance(&rt, &k);
    assert_eq!((bw, uv), (0.0, [0.0; 4]));
}

#[test]
fn flat_main_respects_explicit_frame_inset_in_either_builder_order() {
    let k = Kit::neutral(true);
    for inset_first in [false, true] {
        let mut s = S { flat: true, frame_inset: Some(6.0), inset_first, ..S::default() };
        s.sidebar = PaneState::new(240.0).collapsed();
        s.inspector = PaneState::new(220.0).collapsed();
        let rt = runtime(s);
        let pane = find(&rt, "main-pane");
        assert_eq!(pane.radius, 0.0, "stays flat when inset is explicit (inset_first={inset_first})");
        assert_eq!(pane.rect, [6.0, 48.0, 988.0, 546.0], "explicit frame_inset wins (inset_first={inset_first})");
    }

    // Passing flat_main(false) restores the default rounded card and 8pt frame inset.
    let mut s = S { flat: false, ..S::default() };
    s.sidebar = PaneState::new(240.0).collapsed();
    s.inspector = PaneState::new(220.0).collapsed();
    let rt = runtime(s);
    let pane = find(&rt, "main-pane");
    assert_eq!(pane.radius, k.radius.lg);
    assert_eq!(pane.rect, [8.0, 48.0, 984.0, 544.0]);
}

#[test]
fn flat_main_mirrors_pane_hairlines_in_rtl() {
    let k = Kit::neutral(true);
    let mut s = S { flat: true, in_panel: true, rtl: true, ..S::default() };
    s.inspector = PaneState::new(220.0).collapsed();
    let rt = runtime(s);

    let sidebar = find(&rt, "sidebar-pane");
    let pane = find(&rt, "main-pane");
    assert!(sidebar.rect[0] > pane.rect[0], "RTL places sidebar on the right: sidebar={:?} main={:?}", sidebar.rect, pane.rect);
    let (bw, uv, radii, _) = main_pane_instance(&rt, &k);
    assert_eq!(radii, [0.0; 4]);
    assert_eq!((bw, uv), (1.0, [0.0, 1.0, 0.0, 0.0]), "RTL mirrors the sidebar hairline to the right edge of main-pane");
}

#[test]
fn page_flat_main_forwards_flat_styling_and_frame_inset() {
    struct P {
        flat: bool,
        in_panel: bool,
        frame_inset: Option<f32>,
    }
    fn page_view(s: &P, _: &mut Cx<P>) -> El {
        let k = Kit::neutral(true);
        let mut page = Page::new(col().key("body").w_full().h_full()).title("Settings").flat_main(s.flat);
        if let Some(inset) = s.frame_inset {
            page = page.frame_inset(inset);
        }
        if s.in_panel {
            page = page.toolbar_in_panel();
        }
        page.build(&k)
    }
    let k = Kit::neutral(true);
    let mut rt = Runtime::new(P { flat: true, in_panel: false, frame_inset: None }, page_view, (960.0, 600.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);

    let pane = |rt: &Runtime<P>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("main-pane")).unwrap();
    let inst = |rt: &Runtime<P>| {
        let r = pane(rt).rect;
        let fill = premul(rt.srgb_blend(), k.c.bg_base);
        *rt.instances.iter().find(|i| i.fill == fill && (0..4).all(|idx| (i.rect[idx] - r[idx]).abs() < 0.5)).unwrap()
    };

    assert_eq!(pane(&rt).radius, 0.0);
    assert_eq!(pane(&rt).rect, [0.0, 48.0, 960.0, 552.0]);
    let i = inst(&rt);
    assert_eq!((i.params[0], i.uv, i.radii), (1.0, [1.0, 0.0, 0.0, 0.0], [0.0; 4]));

    // With toolbar_in_panel: full window, no borders.
    rt.update(|s, _| s.in_panel = true);
    rt.frame(1.0 / 60.0);
    assert_eq!(pane(&rt).rect, [0.0, 0.0, 960.0, 600.0]);
    let i = inst(&rt);
    assert_eq!((i.params[0], i.uv), (0.0, [0.0; 4]));

    // Explicit frame_inset still applies with flat_main(true).
    rt.update(|s, _| {
        s.in_panel = false;
        s.frame_inset = Some(4.0);
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(pane(&rt).radius, 0.0);
    assert_eq!(pane(&rt).rect, [4.0, 48.0, 952.0, 548.0]);

    // Default flat_main(false) keeps the rounded 8pt framed card.
    rt.update(|s, _| {
        s.flat = false;
        s.frame_inset = None;
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(pane(&rt).radius, k.radius.lg);
    assert_eq!(pane(&rt).rect, [8.0, 48.0, 944.0, 544.0]);
}

#[test]
fn sidebar_toggle_labels_localize_name_and_tooltip_and_keep_copy() {
    fn assert_copy<T: Copy>(_: T) {}
    let custom = SidebarToggle::new().labels("显示侧边栏", "隐藏侧边栏");
    assert_copy(custom);

    let sc = if cfg!(target_os = "macos") { "⇧⌘B" } else { "Ctrl+Shift+B" };

    // Default English labels remain unchanged.
    let mut rt = runtime(S::default());
    let tg = find(&rt, "sidebar-toggle");
    assert_eq!(tg.label.as_deref(), Some("Hide sidebar"));
    assert_eq!(tg.help.as_deref(), Some(format!("Hide sidebar  {sc}").as_str()));
    assert_eq!(tg.expanded, Some(true));

    rt.key(KeyEvent { key: Key::Command('b'), mods: Mods { shift: true, cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    let tg = find(&rt, "sidebar-toggle");
    assert_eq!(tg.label.as_deref(), Some("Show sidebar"));
    assert_eq!(tg.help.as_deref(), Some(format!("Show sidebar  {sc}").as_str()));
    assert_eq!(tg.expanded, Some(false));

    // Localized CJK labels appear in accessible name, help text, and hover tooltip in both states.
    let mut rt = runtime(S { toggle: custom, ..S::default() });
    let tg = find(&rt, "sidebar-toggle");
    assert_eq!(tg.label.as_deref(), Some("隐藏侧边栏"));
    let expected_hide_tip = format!("隐藏侧边栏  {sc}");
    assert_eq!(tg.help.as_deref(), Some(expected_hide_tip.as_str()));
    assert_eq!(tg.expanded, Some(true));

    let (x, y) = rt.find("sidebar-toggle").expect("toggle");
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(rt.help_delay + 0.05);
    assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some(expected_hide_tip.as_str()));

    // Clicking the toggle closes the sidebar and switches both name and tooltip to the show label.
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    let tg = find(&rt, "sidebar-toggle");
    assert_eq!(tg.label.as_deref(), Some("显示侧边栏"));
    let expected_show_tip = format!("显示侧边栏  {sc}");
    assert_eq!(tg.help.as_deref(), Some(expected_show_tip.as_str()));
    assert_eq!(tg.expanded, Some(false));

    // RTL Arabic labels with shortcut omitted.
    let rtl_toggle = SidebarToggle::new().labels("إظهار الشريط الجانبي", "إخفاء الشريط الجانبي").shortcut("");
    let mut rt = runtime(S { toggle: rtl_toggle, ..S::default() });
    assert_eq!(find(&rt, "sidebar-toggle").label.as_deref(), Some("إخفاء الشريط الجانبي"));
    assert_eq!(find(&rt, "sidebar-toggle").help.as_deref(), Some("إخفاء الشريط الجانبي"));
    rt.update(|s, _| s.sidebar.set_open(false));
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "sidebar-toggle").label.as_deref(), Some("إظهار الشريط الجانبي"));
    assert_eq!(find(&rt, "sidebar-toggle").help.as_deref(), Some("إظهار الشريط الجانبي"));

    // Empty and whitespace-only strings fall back to the defaults per state.
    let partial = SidebarToggle::new().labels("", "隐藏侧边栏").shortcut("");
    let mut rt = runtime(S { toggle: partial, ..S::default() });
    assert_eq!(find(&rt, "sidebar-toggle").label.as_deref(), Some("隐藏侧边栏"));
    rt.update(|s, _| s.sidebar.set_open(false));
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "sidebar-toggle").label.as_deref(), Some("Show sidebar"));

    let blank = SidebarToggle::new().labels("   ", "   ").shortcut("");
    let mut rt = runtime(S { toggle: blank, ..S::default() });
    assert_eq!(find(&rt, "sidebar-toggle").label.as_deref(), Some("Hide sidebar"));
    rt.update(|s, _| s.sidebar.set_open(false));
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "sidebar-toggle").label.as_deref(), Some("Show sidebar"));
}
