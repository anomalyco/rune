use windows_sys::Win32::Foundation::{POINT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT};

use crate::titlebar::{Caption, CaptionButton, CaptionState};

use crate::runtime::DragHit;

use super::{
    DragHitSync, HitWindow, adjust_track_height, client_hit, custom_borders,
    dpi_scaled_outer_size, drag_hit_code, nccalcsize_restored, outer_size, owned_button,
};

fn caption() -> Caption {
    Caption { height: 36.0, state: CaptionState::Idle, maximized: false, minimizable: true, maximizable: true, hidden: false, mirrored: false }
}

/// An 800 px wide restored window at 96 dpi with an 8 px top resize edge, the point in client pixels.
fn at(caption: Caption, x: i32, y: i32) -> HitWindow {
    HitWindow { caption, width: 800, dpi: 96, edge: 8, zoomed: false, resizable: true, point: POINT { x, y } }
}

fn owned(w: &HitWindow, hit: u32) -> Option<CaptionButton> { owned_button(w, hit as WPARAM) }

#[test]
fn a_system_caption_button_above_the_client_area_is_not_rune_s() {
    // A native caption band above the client area: the default procedure reports its own
    // buttons there. The press is the system's, not a press on Rune's drawn Close that the
    // release over no drawn button then cancels.
    for hit in [HTMINBUTTON, HTMAXBUTTON, HTCLOSE] {
        assert_eq!(owned(&at(caption(), 780, -12), hit), None, "hit {hit} above the client area");
    }
    assert_eq!(owned(&at(caption(), 200, 18), HTCLOSE), None, "over the app's titlebar, clear of the buttons");
    assert_eq!(owned(&at(caption(), 740, 18), HTCLOSE), None, "over Maximize");
    assert_eq!(owned(&at(caption(), 790, 2), HTCLOSE), None, "in the top resize edge");
    let fixed = Caption { maximizable: false, ..caption() };
    assert_eq!(owned(&at(fixed, 740, 18), HTMAXBUTTON), None, "a dimmed Maximize");
}

#[test]
fn a_press_on_a_drawn_caption_button_is_rune_s() {
    // 46 px buttons at 96 dpi: Minimize [662, 708), Maximize [708, 754), Close [754, 800).
    assert_eq!(owned(&at(caption(), 775, 18), HTCLOSE), Some(CaptionButton::Close));
    assert_eq!(owned(&at(caption(), 730, 18), HTMAXBUTTON), Some(CaptionButton::Maximize));
    assert_eq!(owned(&at(caption(), 685, 18), HTMINBUTTON), Some(CaptionButton::Minimize));
    assert_eq!(owned(&at(caption(), 775, 18), HTCAPTION), None);
    let zoomed = HitWindow { zoomed: true, ..at(caption(), 799, 0) };
    assert_eq!(owned(&zoomed, HTCLOSE), Some(CaptionButton::Close), "maximized: no top resize edge");
    let scaled = HitWindow { width: 1200, dpi: 144, edge: 12, ..at(caption(), 1160, 24) };
    assert_eq!(owned(&scaled, HTCLOSE), Some(CaptionButton::Close), "150%: Close is [1131, 1200)");
    let rtl = Caption { mirrored: true, ..caption() };
    assert_eq!(owned(&at(rtl, 20, 18), HTCLOSE), Some(CaptionButton::Close), "mirrored: Close is leftmost");
    assert_eq!(owned(&at(rtl, 775, 18), HTCLOSE), None);
}

#[test]
fn the_client_hit_test_finds_the_top_edge_and_the_buttons() {
    assert_eq!(client_hit(&at(caption(), 4, 2), &[]), HTTOPLEFT);
    assert_eq!(client_hit(&at(caption(), 400, 2), &[]), HTTOP);
    assert_eq!(client_hit(&at(caption(), 796, 2), &[]), HTTOPRIGHT);
    assert_eq!(client_hit(&at(caption(), 775, 18), &[]), HTCLOSE);
    assert_eq!(client_hit(&at(caption(), 400, 18), &[]), HTCLIENT);
    let fixed = HitWindow { resizable: false, ..at(caption(), 775, 2) };
    assert_eq!(client_hit(&fixed, &[]), HTCLOSE, "no top resize edge on a window that doesn't resize");
}

fn standard_96() -> RECT {
    RECT { left: 8, top: 31, right: 8, bottom: 8 }
}

fn standard_144() -> RECT {
    RECT { left: 11, top: 45, right: 11, bottom: 11 }
}

#[test]
fn restored_custom_frame_leaves_zero_top_inset() {
    for standard in [standard_96(), standard_144()] {
        let b = custom_borders(standard);
        assert_eq!((b.left, b.top, b.right, b.bottom), (standard.left, 0, standard.right, standard.bottom));

        let outer = RECT { left: 100, top: 50, right: 916, bottom: 658 };
        let def_client = RECT {
            left: outer.left + standard.left,
            top: outer.top + standard.top,
            right: outer.right - standard.right,
            bottom: outer.bottom - standard.bottom,
        };
        let client = nccalcsize_restored(outer, def_client);
        assert_eq!(client.top, outer.top, "WM_NCCALCSIZE must leave a 0 top inset on restored windows");
        assert_eq!((client.left, client.right, client.bottom), (def_client.left, def_client.right, def_client.bottom));
    }
}

#[test]
fn restored_border_math_is_consistent_across_size_placement_and_dpi() {
    let std_96 = standard_96();
    let std_144 = standard_144();

    // request_inner_size -> WM_NCCALCSIZE -> restored_borders (placement) round-trip at 96 dpi.
    let (ow, oh) = outer_size(800, 600, std_96);
    assert_eq!((ow, oh), (816, 608));
    let outer = RECT { left: 120, top: 80, right: 120 + ow, bottom: 80 + oh };
    let def_client = RECT {
        left: outer.left + std_96.left,
        top: outer.top + std_96.top,
        right: outer.right - std_96.right,
        bottom: outer.bottom - std_96.bottom,
    };
    let client = nccalcsize_restored(outer, def_client);
    assert_eq!((client.right - client.left, client.bottom - client.top), (800, 600));
    let b = custom_borders(std_96);
    assert_eq!((ow - b.left - b.right, oh - b.top - b.bottom), (800, 600));

    // WM_GETMINMAXINFO removes the entire standard top border from winit's min/max track heights.
    assert_eq!(adjust_track_height(0, 200 + std_96.top + std_96.bottom, std_96), outer_size(800, 200, std_96).1);
    assert_eq!(adjust_track_height(0, 900 + std_144.top + std_144.bottom, std_144), outer_size(1200, 900, std_144).1);
    assert_eq!(adjust_track_height(300, 300, std_96), 300, "untouched track height stays unchanged");
    assert_eq!(adjust_track_height(0, 10, std_96), 1, "adjusted track height clamps at 1");

    // WM_GETDPISCALEDSIZE scales client size and adds only sides + bottom (0 top inset).
    let scaled_up = dpi_scaled_outer_size(800, 600, 96, 144, std_144);
    assert_eq!((scaled_up.cx, scaled_up.cy), (1222, 911));
    let scaled_down = dpi_scaled_outer_size(1200, 900, 144, 96, std_96);
    assert_eq!((scaled_down.cx, scaled_down.cy), (816, 608));
    let zero_old = dpi_scaled_outer_size(100, 100, 0, 96, std_96);
    assert_eq!((zero_old.cx, zero_old.cy), (9600 + 16, 9600 + 8));
}

#[test]
fn window_drag_regions_answer_htcaption_after_edges_caption_buttons_and_interactive_children() {
    // Full-width titlebar [0, 0, 800, 36] with a button [20, 4, 100, 32] painted above it.
    let hits = [
        DragHit { rect: [20.0, 4.0, 100.0, 32.0], drag: false },
        DragHit { rect: [0.0, 0.0, 800.0, 36.0], drag: true },
    ];
    // Bare titlebar returns HTCAPTION.
    assert_eq!(client_hit(&at(caption(), 400, 18), &hits), HTCAPTION);
    // Interactive child inside .window_drag() returns HTCLIENT.
    assert_eq!(client_hit(&at(caption(), 50, 18), &hits), HTCLIENT);
    // Body below titlebar returns HTCLIENT.
    assert_eq!(client_hit(&at(caption(), 400, 100), &hits), HTCLIENT);
    // Top resize edge still wins over .window_drag() when restored and resizable.
    assert_eq!(client_hit(&at(caption(), 4, 2), &hits), HTTOPLEFT);
    assert_eq!(client_hit(&at(caption(), 400, 2), &hits), HTTOP);
    assert_eq!(client_hit(&at(caption(), 796, 2), &hits), HTTOPRIGHT);
    // Maximized window has no top resize edge, so y=2 in the bare band returns HTCAPTION.
    let zoomed = HitWindow { zoomed: true, ..at(caption(), 400, 2) };
    assert_eq!(client_hit(&zoomed, &hits), HTCAPTION);
    // Caption buttons still win over full-width .window_drag(), keeping mouse/Snap Layouts intact.
    assert_eq!(client_hit(&at(caption(), 685, 18), &hits), HTMINBUTTON);
    assert_eq!(client_hit(&at(caption(), 730, 18), &hits), HTMAXBUTTON);
    assert_eq!(client_hit(&at(caption(), 775, 18), &hits), HTCLOSE);
    // A dimmed Maximize button stays HTCLIENT rather than turning into HTCAPTION.
    let no_max = Caption { maximizable: false, ..caption() };
    assert_eq!(client_hit(&at(no_max, 730, 18), &hits), HTCLIENT);
    // 150% DPI scaling (144 dpi): 400 pt -> 600 px, 18 pt -> 27 px.
    let scaled_bare = HitWindow { width: 1200, dpi: 144, edge: 12, ..at(caption(), 600, 27) };
    assert_eq!(client_hit(&scaled_bare, &hits), HTCAPTION);
    let scaled_btn = HitWindow { width: 1200, dpi: 144, edge: 12, ..at(caption(), 75, 27) };
    assert_eq!(client_hit(&scaled_btn, &hits), HTCLIENT);
    // Borderless drag subclass helper uses the same bare-drag hit test.
    assert_eq!(drag_hit_code(96, POINT { x: 400, y: 18 }, &hits), HTCAPTION);
    assert_eq!(drag_hit_code(96, POINT { x: 50, y: 18 }, &hits), HTCLIENT);
    assert_eq!(drag_hit_code(96, POINT { x: 400, y: 100 }, &hits), HTCLIENT);
}

#[test]
fn runtime_window_drag_hits_respect_children_modals_tooltips_and_zoom() {
    use crate::{Cx, El, Env, Runtime, TextEdit, TextStyle, col, el, row, text, text_input};

    #[derive(Default)]
    struct S {
        modal: bool,
        zoom: f32,
        note: TextEdit,
    }

    fn view(s: &S, cx: &mut Cx<S>) -> El {
        if s.zoom > 0.0 { cx.zoom(s.zoom); }
        let bar = row().key("bar").size(800.0, 36.0).window_drag()
            .child(el().key("btn").size(60.0, 36.0).on_press(|_: &mut S| {}))
            .child(el().key("nodrag").size(60.0, 36.0).no_window_drag())
            .child(el().key("badge").size(40.0, 36.0).help("Status"))
            .child(text_input(&s.note, TextStyle::new(14.0, 400.0)).key("input").label("Search").size(100.0, 28.0).on_edit(|s: &mut S| &mut s.note))
            .child(text("Title"));
        let modal = s.modal.then(|| el().key("sheet").size(200.0, 100.0).modal(true, |s: &mut S| s.modal = false));
        col().size(800.0, 400.0).child(bar).children(modal)
    }

    let mut rt = Runtime::new(S { modal: false, zoom: 1.0, note: TextEdit::new("") }, view, (800.0, 400.0), Env::default());
    rt.frame(0.0);

    let hits = rt.core().window_drag_hits();
    // Interactive button [0..60], .no_window_drag() [60..120], tooltip badge [120..160],
    // text input [160..260] all stay HTCLIENT.
    assert_eq!(client_hit(&at(caption(), 30, 18), &hits), HTCLIENT, "button inside .window_drag()");
    assert_eq!(client_hit(&at(caption(), 90, 18), &hits), HTCLIENT, ".no_window_drag() child");
    assert_eq!(client_hit(&at(caption(), 140, 18), &hits), HTCLIENT, "tooltip-only badge");
    assert_eq!(client_hit(&at(caption(), 210, 18), &hits), HTCLIENT, "text input inside .window_drag()");
    // Non-selectable plain text and bare band [260..662] return HTCAPTION.
    assert_eq!(client_hit(&at(caption(), 280, 18), &hits), HTCAPTION, "plain title text inside .window_drag()");
    assert_eq!(client_hit(&at(caption(), 450, 18), &hits), HTCAPTION, "bare drag region");
    // Caption buttons and top resize edge stay intact.
    assert_eq!(client_hit(&at(caption(), 450, 2), &hits), HTTOP);
    assert_eq!(client_hit(&at(caption(), 775, 18), &hits), HTCLOSE);

    // Active modal empties drag hits so titlebar clicks reach Rune to dismiss the modal,
    // while top resize edge and caption buttons still work.
    rt.update(|s, _| s.modal = true);
    rt.frame(1.0 / 60.0);
    let modal_hits = rt.core().window_drag_hits();
    assert!(modal_hits.is_empty(), "active modal must clear HTCAPTION regions");
    assert_eq!(client_hit(&at(caption(), 450, 18), &modal_hits), HTCLIENT);
    assert_eq!(client_hit(&at(caption(), 450, 2), &modal_hits), HTTOP);
    assert_eq!(client_hit(&at(caption(), 775, 18), &modal_hits), HTCLOSE);

    // Zoom 1.25 scales view rects into window points: button [0..60] becomes [0..75] window pt,
    // titlebar height 36 becomes 45 window pt.
    rt.update(|s, _| { s.modal = false; s.zoom = 1.25; });
    rt.frame(1.0 / 60.0);
    let zoomed_hits = rt.core().window_drag_hits();
    assert_eq!(drag_hit_code(96, POINT { x: 70, y: 20 }, &zoomed_hits), HTCLIENT, "zoomed button covers x=70");
    assert_eq!(drag_hit_code(96, POINT { x: 450, y: 40 }, &zoomed_hits), HTCAPTION, "zoomed 36 pt bar reaches y=40");
    assert_eq!(drag_hit_code(96, POINT { x: 450, y: 50 }, &zoomed_hits), HTCLIENT, "below zoomed 45 pt bar");
}

#[test]
fn window_resize_strip_above_window_drag_stays_htclient_so_press_reaches_rune() {
    use crate::{Cx, El, Env, Runtime, col, el};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(400.0, 200.0)
            .child(el().key("bar").size(400.0, 40.0).window_drag())
            .child(el().key("resize-n").pin(Some(0.0), Some(0.0), Some(0.0), None).h(6.0).window_resize(crate::ResizeEdge::North))
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::default());
    rt.frame(0.0);
    let hits = rt.core().window_drag_hits();
    assert_eq!(drag_hit_code(96, POINT { x: 200, y: 3 }, &hits), HTCLIENT, "resize strip wins over .window_drag()");
    assert_eq!(drag_hit_code(96, POINT { x: 200, y: 20 }, &hits), HTCAPTION, "uncovered titlebar still drags");
}

#[test]
fn drag_hit_sync_skips_idle_and_non_committing_frames() {
    let mut sync = DragHitSync::default();
    assert!(sync.needs_update(1, 1.0, false, false), "initial frame syncs");
    assert!(!sync.needs_update(1, 1.0, false, false), "unchanged frame does zero work");
    assert!(sync.needs_update(2, 1.0, false, false), "layout commit updates");
    assert!(!sync.needs_update(2, 1.0, false, false));
    assert!(sync.needs_update(2, 1.25, false, false), "zoom change updates");
    assert!(sync.needs_update(2, 1.25, true, false), "fullscreen toggle updates");
    assert!(!sync.needs_update(2, 1.25, true, false));
    assert!(sync.needs_update(2, 1.25, false, true), "motion updates");
    assert!(sync.needs_update(2, 1.25, false, false), "first settled frame after motion updates");
    assert!(!sync.needs_update(2, 1.25, false, false), "settled idle frame does zero work");
}

#[test]
fn hidden_caption_keeps_top_resize_edge_and_releases_button_area_to_client_or_drag() {
    let hidden = Caption { hidden: true, ..caption() };
    // Top resize edges and corners stay active when controls are hidden.
    assert_eq!(client_hit(&at(hidden, 4, 2), &[]), HTTOPLEFT, "top-left resize corner");
    assert_eq!(client_hit(&at(hidden, 400, 2), &[]), HTTOP, "top resize edge");
    assert_eq!(client_hit(&at(hidden, 796, 2), &[]), HTTOPRIGHT, "top-right resize corner");

    // Without drag hits, the entire caption button strip answers HTCLIENT and owns no button.
    for (x, hit) in [(685, HTMINBUTTON), (730, HTMAXBUTTON), (775, HTCLOSE)] {
        assert_eq!(client_hit(&at(hidden, x, 18), &[]), HTCLIENT, "hidden button at x={x}");
        assert_eq!(owned(&at(hidden, x, 18), hit), None, "hidden caption owns no button at x={x}");
    }

    // Mirrored (RTL) and 150% DPI also release the button strip to HTCLIENT.
    let hidden_rtl = Caption { hidden: true, mirrored: true, ..caption() };
    for (x, hit) in [(20, HTCLOSE), (70, HTMAXBUTTON), (115, HTMINBUTTON)] {
        assert_eq!(client_hit(&at(hidden_rtl, x, 18), &[]), HTCLIENT, "hidden RTL button at x={x}");
        assert_eq!(owned(&at(hidden_rtl, x, 18), hit), None, "hidden RTL caption owns no button at x={x}");
    }
    let scaled = HitWindow { width: 1200, dpi: 144, edge: 12, ..at(hidden, 1160, 24) };
    assert_eq!(client_hit(&scaled, &[]), HTCLIENT, "150% DPI hidden Close area");
    assert_eq!(owned(&scaled, HTCLOSE), None, "150% DPI hidden Close owned_button");

    // With a full-width .window_drag() bar and an app-drawn Close button [754..800],
    // top edge still resizes, the app button stays HTCLIENT, and bare space answers HTCAPTION.
    let hits = [
        DragHit { rect: [754.0, 0.0, 800.0, 36.0], drag: false },
        DragHit { rect: [0.0, 0.0, 800.0, 36.0], drag: true },
    ];
    assert_eq!(client_hit(&at(hidden, 796, 2), &hits), HTTOPRIGHT, "top-right resize beats drag");
    assert_eq!(client_hit(&at(hidden, 775, 18), &hits), HTCLIENT, "app-drawn close button stays HTCLIENT");
    assert_eq!(client_hit(&at(hidden, 685, 18), &hits), HTCAPTION, "bare titlebar over hidden min slot drags");
    let zoomed = HitWindow { zoomed: true, ..at(hidden, 775, 2) };
    assert_eq!(client_hit(&zoomed, &hits), HTCLIENT, "maximized app-drawn close button at y=2");
    assert_eq!(owned(&zoomed, HTCLOSE), None, "maximized hidden caption owns no Close button");
}
