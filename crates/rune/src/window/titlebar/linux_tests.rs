use super::*;
use crate::titlebar::Caption;

use CaptionButton::{Close, Maximize, Minimize};

const FLOATING: WindowState = WindowState { maximized: false, full_screen: false, resizable: true };

#[test]
fn the_layout_follows_gnomes_setting() {
    // GNOME's default: close alone, at the right.
    assert_eq!(ButtonLayout::parse("appmenu:close"), ButtonLayout { buttons: [Some(Close), None, None], leading: false });
    assert_eq!(ButtonLayout::parse("appmenu:minimize,maximize,close"), ButtonLayout::default());
    assert_eq!(ButtonLayout::parse(":minimize,maximize,close"), ButtonLayout::default());
    // At the left, listed towards the window's edge: drawn mirrored, close leftmost.
    assert_eq!(ButtonLayout::parse("close,minimize,maximize:appmenu"), ButtonLayout { buttons: [Some(Maximize), Some(Minimize), Some(Close)], leading: true });
    // Split across both sides: the side that holds close.
    assert_eq!(ButtonLayout::parse("close:minimize,maximize"), ButtonLayout { buttons: [Some(Close), None, None], leading: true });
    assert_eq!(ButtonLayout::parse("minimize:close"), ButtonLayout { buttons: [Some(Close), None, None], leading: false });
    // No close anywhere: the side with more buttons.
    assert_eq!(ButtonLayout::parse("minimize,maximize:"), ButtonLayout { buttons: [Some(Maximize), Some(Minimize), None], leading: true });
    // Nothing named, a button named twice, spaces and names that aren't buttons.
    assert_eq!(ButtonLayout::parse(""), ButtonLayout { buttons: [None; 3], leading: false });
    assert_eq!(ButtonLayout::parse("appmenu:"), ButtonLayout { buttons: [None; 3], leading: false });
    assert_eq!(ButtonLayout::parse(":close,close, minimize ,icon,spacer"), ButtonLayout { buttons: [Some(Close), Some(Minimize), None], leading: false });
    // Without a colon it is the left side.
    assert_eq!(ButtonLayout::parse("close"), ButtonLayout { buttons: [Some(Close), None, None], leading: true });
}

#[test]
fn the_frame_can_be_forced_either_way() {
    assert_eq!(forced(None), None);
    assert_eq!(forced(Some("0")), Some(false));
    assert_eq!(forced(Some("1")), Some(true));
    assert_eq!(forced(Some("")), None);
    assert_eq!(forced(Some("yes")), None);
}

#[test]
fn the_desktops_layout_is_kept_and_a_new_one_is_reported() {
    assert_eq!(desktop_layout(), ButtonLayout::default(), "three at the right until the desktop says");
    assert!(set_desktop_layout("appmenu:close"));
    assert!(!set_desktop_layout("appmenu:close"), "the same layout again is no change");
    assert_eq!(desktop_layout().buttons, [Some(Close), None, None]);
    assert!(set_desktop_layout("close,minimize,maximize:"));
    assert!(desktop_layout().leading);
}

#[test]
fn the_buttons_take_their_room_in_whole_pixels() {
    let close = Caption::own_frame(44.0, ButtonLayout::parse("appmenu:close"), FLOATING);
    assert_eq!(close.buttons().to_vec(), [Close]);
    // A 34 pt cell and 7 pt to the window's edge.
    assert_eq!(close.width(1.0), 41.0);
    let all = Caption::own_frame(44.0, ButtonLayout::default(), FLOATING);
    assert_eq!(all.buttons().to_vec(), [Minimize, Maximize, Close]);
    // Three cells, 3 pt between two of them.
    assert_eq!(all.width(1.0), 115.0);
    for scale in [1.25, 1.5, 1.75, 2.0] {
        let pixels = all.width(scale) * scale;
        assert!((pixels - pixels.round()).abs() < 1e-3, "{pixels} px at {scale}");
    }
    // A layout that names no button takes no room.
    assert_eq!(Caption::own_frame(44.0, ButtonLayout::parse("appmenu:"), FLOATING).width(1.0), 0.0);
}

#[test]
fn only_a_floating_resizable_window_has_edges_and_full_screen_has_no_buttons() {
    let layout = ButtonLayout::default();
    let floating = Caption::own_frame(44.0, layout, FLOATING);
    assert!(floating.edges && !floating.maximized && floating.enabled(Maximize));
    let maximized = Caption::own_frame(44.0, layout, WindowState { maximized: true, ..FLOATING });
    assert!(!maximized.edges && maximized.maximized, "a maximized window offers Restore and no edges");
    let fixed = Caption::own_frame(44.0, layout, WindowState { resizable: false, ..FLOATING });
    assert!(!fixed.edges && !fixed.enabled(Maximize) && fixed.enabled(Minimize) && fixed.enabled(Close));
    let full = Caption::own_frame(44.0, layout, WindowState { full_screen: true, ..FLOATING });
    assert!(!full.edges && full.buttons().is_empty());
    assert_eq!(full.width(1.0), 0.0);
}
