use winit::window::CursorGrabMode;

use super::{GrabOutcome, MotionPlatform, apply_grab_with, resolve_cursor, scale_motion};
use crate::el::Cursor;
use crate::runtime::LockEffect;

#[test]
fn grab_prefers_locked_and_stops_on_success() {
    let mut calls = Vec::new();
    let res: Result<GrabOutcome, (&str, &str)> = apply_grab_with(LockEffect::Grab, |mode| {
        calls.push(mode);
        Ok(())
    });
    assert_eq!(res, Ok(GrabOutcome::Locked));
    assert_eq!(calls, vec![CursorGrabMode::Locked]);
}

#[test]
fn grab_falls_back_to_confined_when_locked_refused() {
    let mut calls = Vec::new();
    let res = apply_grab_with(LockEffect::Grab, |mode| {
        calls.push(mode);
        match mode {
            CursorGrabMode::Locked => Err("not supported on X11"),
            CursorGrabMode::Confined => Ok(()),
            CursorGrabMode::None => unreachable!(),
        }
    });
    assert_eq!(res, Ok(GrabOutcome::Confined));
    assert_eq!(calls, vec![CursorGrabMode::Locked, CursorGrabMode::Confined]);
}

#[test]
fn grab_surfaces_both_errors_when_locked_and_confined_both_refused() {
    let mut calls = Vec::new();
    let res = apply_grab_with(LockEffect::Grab, |mode| {
        calls.push(mode);
        match mode {
            CursorGrabMode::Locked => Err("locked refused"),
            CursorGrabMode::Confined => Err("confined refused"),
            CursorGrabMode::None => unreachable!(),
        }
    });
    assert_eq!(res, Err(("locked refused", "confined refused")));
    assert_eq!(calls, vec![CursorGrabMode::Locked, CursorGrabMode::Confined]);
}

#[test]
fn ungrab_calls_none_and_ignores_platform_error() {
    let mut calls = Vec::new();
    let res = apply_grab_with(LockEffect::Ungrab, |mode| {
        calls.push(mode);
        Err::<(), _>("already ungrabbed")
    });
    assert_eq!(res, Ok(GrabOutcome::Released));
    assert_eq!(calls, vec![CursorGrabMode::None]);
}

#[test]
fn scale_motion_passes_logical_points_through_and_scales_device_counts() {
    assert_eq!(scale_motion(12.0, -6.0, 2.0, MotionPlatform::LogicalPoints), [12.0, -6.0]);
    assert_eq!(scale_motion(12.0, -6.0, 1.0, MotionPlatform::DeviceCounts), [12.0, -6.0]);
    assert_eq!(scale_motion(12.0, -6.0, 2.0, MotionPlatform::DeviceCounts), [6.0, -3.0]);
    assert_eq!(scale_motion(15.0, -9.0, 1.5, MotionPlatform::DeviceCounts), [10.0, -6.0]);
}

#[test]
fn resolve_cursor_hides_while_locked_and_focused_and_restores_otherwise() {
    assert_eq!(resolve_cursor(Cursor::Crosshair, true, true), Cursor::Hidden);
    assert_eq!(resolve_cursor(Cursor::Default, true, true), Cursor::Hidden);
    assert_eq!(resolve_cursor(Cursor::Crosshair, true, false), Cursor::Crosshair);
    assert_eq!(resolve_cursor(Cursor::Hidden, true, false), Cursor::Default);
    assert_eq!(resolve_cursor(Cursor::Pointer, false, true), Cursor::Pointer);
    assert_eq!(resolve_cursor(Cursor::Hidden, false, true), Cursor::Hidden);
    assert_eq!(resolve_cursor(Cursor::Hidden, false, false), Cursor::Default);
}
