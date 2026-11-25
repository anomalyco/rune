use super::*;
use crate::titlebar::Caption;

use CaptionButton::{Close, Maximize, Minimize};

const FLOATING: WindowState = WindowState { maximized: false, full_screen: false, resizable: true, minimizable: true, maximizable: true, tiled: Sides::NONE, outer_frame: false };

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
    assert!(floating.edges == Sides::ALL && !floating.maximized && floating.enabled(Maximize));
    let maximized = Caption::own_frame(44.0, layout, WindowState { maximized: true, ..FLOATING });
    assert!(!maximized.edges.any() && maximized.maximized, "a maximized window offers Restore and no edges");
    let full = Caption::own_frame(44.0, layout, WindowState { full_screen: true, ..FLOATING });
    assert!(!full.edges.any() && full.buttons().is_empty());
    assert_eq!(full.width(1.0), 0.0);
}

#[test]
fn the_window_shows_only_the_buttons_it_can_act_on() {
    let layout = ButtonLayout::default();
    // A fixed-size window never maximizes: no button for it, as GTK's window controls show none.
    let fixed = Caption::own_frame(44.0, layout, WindowState { resizable: false, maximizable: false, ..FLOATING });
    assert!(!fixed.edges.any());
    assert_eq!(fixed.buttons().to_vec(), [Minimize, Close]);
    // A compositor that minimizes nothing (a tiling one) shows no minimize button.
    let tiling = Caption::own_frame(44.0, layout, WindowState { minimizable: false, ..FLOATING });
    assert_eq!(tiling.buttons().to_vec(), [Maximize, Close]);
    assert_eq!(tiling.width(1.0), 78.0, "two cells, 3 pt between them, 7 pt to the edge");
    // Close is the window's own to do.
    let neither = Caption::own_frame(44.0, layout, WindowState { minimizable: false, maximizable: false, ..FLOATING });
    assert_eq!(neither.buttons().to_vec(), [Close]);
}

#[test]
fn a_tiled_side_has_no_resize_edge() {
    let layout = ButtonLayout::default();
    // The left half of the screen: only its right side is free.
    let left_half = Sides { right: false, ..Sides::ALL };
    let tiled = Caption::own_frame(44.0, layout, WindowState { tiled: left_half, ..FLOATING });
    assert_eq!(tiled.edges, Sides { right: true, ..Sides::NONE });
    assert_eq!(Caption::own_frame(44.0, layout, WindowState { tiled: Sides::ALL, ..FLOATING }).edges, Sides::NONE);
    // Tiled and maximized: none, whatever is tiled.
    assert!(!Caption::own_frame(44.0, layout, WindowState { tiled: left_half, maximized: true, ..FLOATING }).edges.any());
}

#[test]
fn a_frame_around_the_window_takes_the_resize_edges_and_leaves_the_buttons() {
    let layout = ButtonLayout::default();
    // winit's frame, less its header bar, has the handles outside the window: no band inside.
    let framed = Caption::own_frame(44.0, layout, WindowState { outer_frame: true, ..FLOATING });
    assert_eq!(framed.edges, Sides::NONE);
    // Everything else is the same caption: the buttons, their room and what they can do.
    let bare = Caption::own_frame(44.0, layout, FLOATING);
    assert_eq!(framed, Caption { edges: Sides::NONE, ..bare });
    assert_eq!(framed.buttons().to_vec(), bare.buttons().to_vec());
    // Tiled on a side or not, the frame keeps the handles.
    let left_half = Sides { right: false, ..Sides::ALL };
    assert_eq!(Caption::own_frame(44.0, layout, WindowState { outer_frame: true, tiled: left_half, ..FLOATING }).edges, Sides::NONE);
}
