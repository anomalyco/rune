use super::selection::{
    self, CARET_HANDLE_TIMEOUT_SECS, Effect, Event, HANDLE_TARGET_DP, HandleKind, HandlePose,
    Mode, Presentation, State, TOOLBAR_COPY, TOOLBAR_CUT, TOOLBAR_SELECT_ALL, ToolbarActions,
    step,
};
use crate::runtime::selection::touch_ui::{
    HandleAnchor, TouchSelectionKind, TouchSelectionSnapshot,
};
use crate::{PopupItem, PopupMenu};

fn anchor(x: f32, top: f32, bottom: f32, visible: bool) -> HandleAnchor {
    HandleAnchor { x, top, bottom, visible }
}

fn range_snap(s: HandleAnchor, e: HandleAnchor, rect: [f32; 4]) -> TouchSelectionSnapshot {
    TouchSelectionSnapshot {
        kind: TouchSelectionKind::FieldRange,
        start: s,
        end: Some(e),
        content_rect: rect,
        editable: true,
        can_copy: true,
        can_select_all: true,
        revision: 1,
    }
}

fn caret_snap(c: HandleAnchor, rev: u64) -> TouchSelectionSnapshot {
    TouchSelectionSnapshot {
        kind: TouchSelectionKind::FieldCaret,
        start: c,
        end: None,
        content_rect: if c.visible { [c.x, c.top, c.x, c.bottom] } else { [0.0; 4] },
        editable: true,
        can_copy: false,
        can_select_all: true,
        revision: rev,
    }
}

#[test]
fn caret_handle_shows_for_four_seconds_after_tap_and_reappears_on_next_tap() {
    let mut st = State::default();

    // Revision 0 (initial auto-focus before any tap) stays hidden.
    let eff0: Vec<_> = step(&mut st, Event::Sync { now: 1.0, snapshot: Some(caret_snap(anchor(40.0, 10.0, 30.0, true), 0)), menu: None }).into_iter().collect();
    assert!(eff0.is_empty());
    assert_eq!(st.mode, Mode::Hidden);

    // Tap bumps revision to 1 -> caret handle appears with a 4.0s hide wake.
    let eff1: Vec<_> = step(&mut st, Event::Sync { now: 2.0, snapshot: Some(caret_snap(anchor(40.0, 10.0, 30.0, true), 1)), menu: None }).into_iter().collect();
    assert_eq!(st.wake_after_secs(2.0), Some(CARET_HANDLE_TIMEOUT_SECS));
    assert!(matches!(eff1.as_slice(), [Effect::Apply(p)] if p.caret == Some(HandlePose { x: 40.0, top: 10.0, bottom: 30.0 })));

    // Early tick at 5.9s keeps handle visible.
    assert!(step(&mut st, Event::Tick { now: 5.9 }).into_iter().next().is_none());
    assert!(matches!(st.mode, Mode::Caret { .. }));

    // Tick at 6.0s hides caret handle and remembers revision 1.
    let eff2: Vec<_> = step(&mut st, Event::Tick { now: 6.0 }).into_iter().collect();
    assert_eq!(eff2, [Effect::Apply(Presentation::default())]);
    assert_eq!(st.mode, Mode::Hidden);

    // Subsequent idle sync with same revision 1 stays hidden.
    assert!(step(&mut st, Event::Sync { now: 6.5, snapshot: Some(caret_snap(anchor(40.0, 10.0, 30.0, true), 1)), menu: None }).into_iter().next().is_none());

    // Fresh tap bumps revision to 2 -> caret handle reappears for 4s.
    let eff3: Vec<_> = step(&mut st, Event::Sync { now: 7.0, snapshot: Some(caret_snap(anchor(40.0, 10.0, 30.0, true), 2)), menu: None }).into_iter().collect();
    assert_eq!(st.wake_after_secs(7.0), Some(4.0));
    assert!(matches!(eff3.as_slice(), [Effect::Apply(p)] if p.caret.is_some()));
}

#[test]
fn toolbar_content_rect_clears_foot_of_48dp_handle_targets() {
    let mut st = State::default();
    let actions = ToolbarActions(ToolbarActions::CUT | ToolbarActions::COPY | ToolbarActions::PASTE);
    let snap = range_snap(anchor(20.0, 100.0, 120.0, true), anchor(110.0, 100.0, 120.0, true), [20.0, 100.0, 110.0, 120.0]);
    let eff: Vec<_> = step(&mut st, Event::Sync { now: 1.0, snapshot: Some(snap), menu: Some(actions) }).into_iter().collect();
    let [Effect::Apply(p)] = eff.as_slice() else { panic!("expected single Apply, got {eff:?}") };
    assert_eq!(p.toolbar_actions, actions);
    assert_eq!(p.toolbar_rect, [20.0, 100.0, 110.0, 120.0 + HANDLE_TARGET_DP]);
}

#[test]
fn dragging_handle_hides_toolbar_tracks_magnifier_and_reopens_toolbar_on_release() {
    let mut st = State::default();
    let actions = ToolbarActions(ToolbarActions::COPY);
    let snap = range_snap(anchor(20.0, 50.0, 70.0, true), anchor(90.0, 50.0, 70.0, true), [20.0, 50.0, 90.0, 70.0]);
    let _ = step(&mut st, Event::Sync { now: 1.0, snapshot: Some(snap), menu: Some(actions) });

    // Grabbing End dismisses open text menu, grabs runtime handle, hides toolbar, shows magnifier.
    let grab: Vec<_> = step(&mut st, Event::HandleGrab { handle: HandleKind::End, x: 90.0, y: 60.0 }).into_iter().collect();
    assert!(grab.contains(&Effect::ResolveTextMenu(None)));
    assert!(grab.contains(&Effect::GrabHandle { handle: HandleKind::End, x: 90.0, y: 60.0 }));
    let applied = grab.iter().find_map(|e| match e { Effect::Apply(p) => Some(*p), _ => None }).unwrap();
    assert!(applied.toolbar_actions.is_empty(), "toolbar hides during handle drag");
    assert_eq!(applied.magnifier, Some((90.0, 60.0)), "magnifier tracks active handle midpoint");

    // Move forwards to runtime; subsequent Sync moves magnifier to new End midpoint.
    let mv: Vec<_> = step(&mut st, Event::HandleMove { x: 140.0, y: 80.0 }).into_iter().collect();
    assert_eq!(mv, [Effect::MoveHandle { x: 140.0, y: 80.0 }]);
    let snap2 = range_snap(anchor(20.0, 50.0, 70.0, true), anchor(140.0, 70.0, 90.0, true), [20.0, 50.0, 140.0, 90.0]);
    let sync: Vec<_> = step(&mut st, Event::Sync { now: 1.1, snapshot: Some(snap2), menu: None }).into_iter().collect();
    assert!(matches!(sync.as_slice(), [Effect::Apply(p)] if p.magnifier == Some((140.0, 80.0)) && p.toolbar_actions.is_empty()));

    // Release hides magnifier and asks runtime for a fresh text menu.
    let rel: Vec<_> = step(&mut st, Event::HandleRelease { now: 1.2 }).into_iter().collect();
    assert!(rel.contains(&Effect::ReleaseHandle));
    assert!(rel.contains(&Effect::RequestTextMenu));
    let rel_p = rel.iter().find_map(|e| match e { Effect::Apply(p) => Some(*p), _ => None }).unwrap();
    assert_eq!(rel_p.magnifier, None);
}

#[test]
fn clipped_handles_hide_and_reappear_when_scrolled_back() {
    let mut st = State::default();
    let actions = ToolbarActions(ToolbarActions::COPY);
    let vis = range_snap(anchor(20.0, 50.0, 70.0, true), anchor(90.0, 50.0, 70.0, true), [20.0, 50.0, 90.0, 70.0]);
    let _ = step(&mut st, Event::Sync { now: 1.0, snapshot: Some(vis), menu: Some(actions) });

    // Scrolled completely offscreen: both handles and toolbar hide.
    let off = range_snap(anchor(20.0, -40.0, -20.0, false), anchor(90.0, -40.0, -20.0, false), [0.0; 4]);
    let eff_off: Vec<_> = step(&mut st, Event::Sync { now: 1.1, snapshot: Some(off), menu: None }).into_iter().collect();
    assert_eq!(eff_off, [Effect::Apply(Presentation::default())]);

    // Scrolled back into view: handles and toolbar reappear automatically.
    let back = range_snap(anchor(20.0, 60.0, 80.0, true), anchor(90.0, 60.0, 80.0, true), [20.0, 60.0, 90.0, 80.0]);
    let eff_back: Vec<_> = step(&mut st, Event::Sync { now: 1.2, snapshot: Some(back), menu: None }).into_iter().collect();
    let [Effect::Apply(p)] = eff_back.as_slice() else { panic!("expected Apply") };
    assert!(p.start.is_some() && p.end.is_some());
    assert_eq!(p.toolbar_actions, actions);
}

#[test]
fn suspend_and_resume_survive_home_and_rotation() {
    let mut st = State::default();
    let actions = ToolbarActions(ToolbarActions::CUT | ToolbarActions::COPY);
    let snap = range_snap(anchor(20.0, 50.0, 70.0, true), anchor(90.0, 50.0, 70.0, true), [20.0, 50.0, 90.0, 70.0]);
    let _ = step(&mut st, Event::Sync { now: 1.0, snapshot: Some(snap), menu: Some(actions) });

    // Home / rotation suspends native overlays without clearing selection.
    let susp: Vec<_> = step(&mut st, Event::Suspend).into_iter().collect();
    assert_eq!(susp, [Effect::Apply(Presentation::default())]);
    assert!(st.suspended);

    // Resume + post-rotation Sync restores handles at new coordinates and requests fresh toolbar.
    let _ = step(&mut st, Event::Resume);
    let rotated = range_snap(anchor(30.0, 80.0, 100.0, true), anchor(120.0, 80.0, 100.0, true), [30.0, 80.0, 120.0, 100.0]);
    let res: Vec<_> = step(&mut st, Event::Sync { now: 2.0, snapshot: Some(rotated), menu: None }).into_iter().collect();
    assert!(res.contains(&Effect::RequestTextMenu), "toolbar must restore after Home/rotation");
    let p = res.iter().find_map(|e| match e { Effect::Apply(p) => Some(*p), _ => None }).unwrap();
    assert_eq!(p.start, Some(HandlePose { x: 30.0, top: 80.0, bottom: 100.0 }));
    assert_eq!(p.end, Some(HandlePose { x: 120.0, top: 80.0, bottom: 100.0 }));
}

#[test]
fn caret_tap_toggles_toolbar_and_select_all_reopens_toolbar() {
    let mut st = State::default();
    let _ = step(&mut st, Event::Sync { now: 1.0, snapshot: Some(caret_snap(anchor(40.0, 10.0, 30.0, true), 1)), menu: None });
    let tap: Vec<_> = step(&mut st, Event::CaretHandleTapped { now: 1.5 }).into_iter().collect();
    assert_eq!(tap, [Effect::RequestTextMenu]);

    // Toolbar opens -> caret auto-hide timer suspends while toolbar is open.
    let paste_all = ToolbarActions(ToolbarActions::PASTE | ToolbarActions::SELECT_ALL);
    let _ = step(&mut st, Event::Sync { now: 1.6, snapshot: Some(caret_snap(anchor(40.0, 10.0, 30.0, true), 1)), menu: Some(paste_all) });
    assert_eq!(st.wake_after_secs(1.6), None);

    // Picking Select All resolves select-all and reopens toolbar on the next range Sync.
    let sa: Vec<_> = step(&mut st, Event::ToolbarAction { code: TOOLBAR_SELECT_ALL, now: 2.0 }).into_iter().collect();
    assert!(sa.contains(&Effect::ResolveTextMenu(Some("select-all"))));
    let full = range_snap(anchor(0.0, 10.0, 30.0, true), anchor(200.0, 10.0, 30.0, true), [0.0, 10.0, 200.0, 30.0]);
    let next: Vec<_> = step(&mut st, Event::Sync { now: 2.1, snapshot: Some(full), menu: None }).into_iter().collect();
    assert!(next.contains(&Effect::RequestTextMenu));
}

#[test]
fn toolbar_actions_respect_enabled_items_and_empty_field_select_all() {
    let menu = PopupMenu::new()
        .item(PopupItem::new("cut", "Cut").enabled(false))
        .item(PopupItem::new("copy", "Copy").enabled(true))
        .item(PopupItem::new("paste", "Paste").enabled(true))
        .item(PopupItem::new("select-all", "Select All").enabled(true));
    let nonempty = ToolbarActions::from_popup_menu(&menu, true);
    assert!(!nonempty.contains(ToolbarActions::CUT));
    assert!(nonempty.contains(ToolbarActions::COPY));
    assert!(nonempty.contains(ToolbarActions::PASTE));
    assert!(nonempty.contains(ToolbarActions::SELECT_ALL));

    let empty_field = ToolbarActions::from_popup_menu(&menu, false);
    assert!(!empty_field.contains(ToolbarActions::SELECT_ALL), "empty field suppresses Select All");
    assert_eq!(empty_field.bits(), (ToolbarActions::COPY | ToolbarActions::PASTE) as i32);
    assert_eq!(ToolbarActions::action_id(TOOLBAR_CUT), Some("cut"));
    assert_eq!(ToolbarActions::action_id(TOOLBAR_COPY), Some("copy"));
    assert_eq!(ToolbarActions::action_id(selection::TOOLBAR_PASTE), Some("paste"));
    assert_eq!(ToolbarActions::action_id(selection::TOOLBAR_DISMISSED), None);

    assert_eq!(HandleKind::from_jni(0), Some(HandleKind::Start));
    assert_eq!(HandleKind::from_jni(1), Some(HandleKind::End));
    assert_eq!(HandleKind::from_jni(2), Some(HandleKind::Caret));
    assert_eq!(HandleKind::from_jni(99), None);
    assert_eq!(HandleKind::Start.to_runtime(), crate::runtime::selection::touch_ui::Handle::Start);
    assert_eq!(HandleKind::End.to_runtime(), crate::runtime::selection::touch_ui::Handle::End);
    assert_eq!(HandleKind::Caret.to_runtime(), crate::runtime::selection::touch_ui::Handle::Caret);
}
