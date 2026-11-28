use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
};

use super::{outer_size_for_content, plan_window_pos};
use crate::ResizeAnchor;

const WORK_AREA: [i32; 4] = [0, 0, 1920, 1040]; // 1080p display with 40 px bottom taskbar
const BASE_FLAGS: u32 = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER;

#[test]
fn bottom_anchored_resize_moves_and_resizes_atomically_in_one_set_window_pos() {
    let current = [660, 740, 600, 260]; // bottom = 1000
    // Growing by 140 px moves y up by 140 px and sets neither SWP_NOMOVE nor SWP_NOSIZE.
    let grow = plan_window_pos(current, Some((600, 400)), false, WORK_AREA, ResizeAnchor::Bottom);
    assert_eq!((grow.x, grow.y, grow.cx, grow.cy), (660, 600, 600, 400));
    assert_eq!(grow.y + grow.cy, 1000, "bottom edge stays pinned");
    assert!(grow.moved && grow.resized);
    assert_eq!(grow.flags, BASE_FLAGS, "atomic move+resize omits both SWP_NOMOVE and SWP_NOSIZE");

    // Shrinking moves y down while keeping bottom at 1000.
    let shrink = plan_window_pos(current, Some((600, 120)), false, WORK_AREA, ResizeAnchor::Bottom);
    assert_eq!((shrink.x, shrink.y, shrink.cx, shrink.cy), (660, 880, 600, 120));
    assert_eq!(shrink.y + shrink.cy, 1000);
    assert_eq!(shrink.flags, BASE_FLAGS);
}

#[test]
fn top_left_default_keeps_swp_nomove_when_window_stays_inside_work_area() {
    let current = [100, 120, 500, 300];
    let grow = plan_window_pos(current, Some((500, 450)), false, WORK_AREA, ResizeAnchor::TopLeft);
    assert_eq!((grow.x, grow.y, grow.cx, grow.cy), (100, 120, 500, 450));
    assert!(!grow.moved && grow.resized);
    assert_eq!(grow.flags, BASE_FLAGS | SWP_NOMOVE);

    // Growing past the work-area bottom lifts the window atomically in the same SetWindowPos.
    let clamped = plan_window_pos(current, Some((500, 980)), false, WORK_AREA, ResizeAnchor::TopLeft);
    assert_eq!((clamped.x, clamped.y, clamped.cx, clamped.cy), (100, 60, 500, 980));
    assert!(clamped.moved && clamped.resized);
    assert_eq!(clamped.flags, BASE_FLAGS);
}

#[test]
fn unchanged_size_and_position_is_a_noop_and_center_only_sets_swp_nosize() {
    let current = [100, 120, 500, 300];
    let same = plan_window_pos(current, Some((500, 300)), false, WORK_AREA, ResizeAnchor::Bottom);
    assert!(!same.moved && !same.resized);
    assert_eq!(same.flags, BASE_FLAGS | SWP_NOMOVE | SWP_NOSIZE);

    let centered = plan_window_pos(current, None, true, WORK_AREA, ResizeAnchor::Bottom);
    assert!(centered.moved && !centered.resized);
    assert_eq!((centered.x, centered.y, centered.cx, centered.cy), (710, 185, 500, 300));
    assert_eq!(centered.flags, BASE_FLAGS | SWP_NOSIZE);
}

#[test]
fn outer_size_for_content_adds_non_client_borders_at_dpi_scale() {
    // Borderless window (0 borders) at 150% DPI.
    let bare = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    assert_eq!(outer_size_for_content((600.0, 200.0), 1.5, bare), (900, 300));

    // Custom titlebar frame (0 top inset, 11 px sides/bottom at 150% DPI).
    let custom = RECT { left: 11, top: 0, right: 11, bottom: 11 };
    assert_eq!(outer_size_for_content((600.0, 200.0), 1.5, custom), (922, 311));

    // Standard Win32 frame at 100% DPI.
    let standard = RECT { left: 8, top: 31, right: 8, bottom: 8 };
    assert_eq!(outer_size_for_content((600.0, 200.0), 1.0, standard), (616, 239));
}
