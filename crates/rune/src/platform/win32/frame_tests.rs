use windows_sys::Win32::Foundation::{POINT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT};

use crate::titlebar::{Caption, CaptionButton, CaptionState};

use super::{
    HitWindow, adjust_track_height, client_hit, custom_borders, dpi_scaled_outer_size,
    nccalcsize_restored, outer_size, owned_button,
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
    assert_eq!(client_hit(&at(caption(), 4, 2)), HTTOPLEFT);
    assert_eq!(client_hit(&at(caption(), 400, 2)), HTTOP);
    assert_eq!(client_hit(&at(caption(), 796, 2)), HTTOPRIGHT);
    assert_eq!(client_hit(&at(caption(), 775, 18)), HTCLOSE);
    assert_eq!(client_hit(&at(caption(), 400, 18)), HTCLIENT);
    let fixed = HitWindow { resizable: false, ..at(caption(), 775, 2) };
    assert_eq!(client_hit(&fixed), HTCLOSE, "no top resize edge on a window that doesn't resize");
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
