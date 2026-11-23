use windows_sys::Win32::Foundation::{POINT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT};

use crate::titlebar::{Caption, CaptionButton, CaptionState};

use super::{HitWindow, client_hit, owned_button};

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
