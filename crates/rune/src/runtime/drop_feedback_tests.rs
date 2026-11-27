//! Tests for file-drag operations (`DragOperations`), modifier keys (`Tx::drag_mods`),
//! per-target `El::drop_feedback`, `Tx::set_drop_operation` / `Tx::set_drop_description`,
//! and backward-compatible copy-only defaults.

use std::path::PathBuf;

use crate::{
    Cx, DragOperation, DragOperations, DropDescription, DropDescriptionKind, DropFeedback, El, Env,
    Mods, Runtime, Tx, col, el,
};

#[test]
fn drag_operations_resolve_follows_desktop_modifier_rules() {
    let all = DragOperations::ALL;
    assert_eq!(all.resolve(Mods::default()), Some(DragOperation::Copy));
    assert_eq!(all.resolve(Mods { shift: true, ..Mods::default() }), Some(DragOperation::Move));
    assert_eq!(all.resolve(Mods { ctrl: true, cmd: true, ..Mods::default() }), Some(DragOperation::Copy));
    assert_eq!(all.resolve(Mods { alt: true, ..Mods::default() }), Some(DragOperation::Link));
    assert_eq!(all.resolve(Mods { shift: true, ctrl: true, cmd: true, ..Mods::default() }), Some(DragOperation::Link));

    // Constrained to allowed operations: Shift when Move isn't allowed falls back to Copy.
    let copy_only = DragOperations::COPY;
    assert_eq!(copy_only.resolve(Mods { shift: true, ..Mods::default() }), Some(DragOperation::Copy));
    assert_eq!(DragOperations::MOVE.resolve(Mods::default()), Some(DragOperation::Move));
    assert_eq!(DragOperations::LINK.resolve(Mods::default()), Some(DragOperation::Link));
    assert_eq!(DragOperations::NONE.resolve(Mods { shift: true, ..Mods::default() }), None);
}

#[test]
fn drop_feedback_updates_on_stationary_modifier_change_and_reports_operation_on_drop() {
    #[derive(Default)]
    struct App {
        folder: String,
        dropped: Vec<(Option<DragOperation>, Mods, DragOperations, Vec<PathBuf>)>,
    }

    fn view(_: &App, _: &mut Cx<App>) -> El {
        el().key("root").size(200.0, 200.0).child(
            el().key("folder")
                .size(120.0, 80.0)
                .on_drop_files(|s: &mut App, files, tx: &mut Tx| {
                    s.dropped.push((
                        tx.drop_operation(),
                        tx.drag_mods(),
                        tx.drag_operations(),
                        files.to_vec(),
                    ));
                })
                .drop_feedback(|s: &App, info| {
                    let Some(op) = info.default_operation() else {
                        return DropFeedback::none();
                    };
                    let desc = match op {
                        DragOperation::Move => DropDescription::move_to(&s.folder),
                        DragOperation::Link => DropDescription::link_in(&s.folder),
                        _ => DropDescription::copy_to(&s.folder),
                    };
                    DropFeedback::from(op).description(desc)
                }),
        )
    }

    let mut rt = Runtime::new(
        App { folder: "Documents".into(), ..App::default() },
        view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Enter over folder with no modifiers -> Copy to Documents.
    rt.drag_files_with(vec!["a.txt".into()], Some((40.0, 40.0)), Mods::default(), DragOperations::ALL);
    rt.frame(0.016);
    assert_eq!(
        rt.drop_feedback(),
        Some(&DropFeedback::copy().description(DropDescription::copy_to("Documents"))),
    );

    // Hold Shift while stationary -> queues re-evaluation and switches to Move to Documents.
    let shift = Mods { shift: true, ..Mods::default() };
    assert!(rt.drag_files_to_with(Some((40.0, 40.0)), shift, DragOperations::ALL));
    rt.frame(0.016);
    assert_eq!(
        rt.drop_feedback(),
        Some(&DropFeedback::move_files().description(DropDescription::move_to("Documents"))),
    );

    // Move outside folder to empty root -> feedback resets to None (no drop target).
    assert!(rt.drag_files_to_with(Some((180.0, 180.0)), shift, DragOperations::ALL));
    rt.frame(0.016);
    assert_eq!(rt.drop_feedback(), Some(&DropFeedback::none()));

    // Move back onto folder and drop with Shift -> on_drop_files receives DragOperation::Move.
    assert!(rt.drag_files_to_with(Some((40.0, 40.0)), shift, DragOperations::ALL));
    rt.frame(0.016);
    rt.drop_files_with(vec!["a.txt".into()], Some((40.0, 40.0)), shift, DragOperations::ALL, None);
    rt.frame(0.016);
    assert_eq!(rt.drop_feedback(), None);
    assert_eq!(rt.state.dropped.len(), 1);
    assert_eq!(rt.state.dropped[0].0, Some(DragOperation::Move));
    assert!(rt.state.dropped[0].1.shift);
    assert_eq!(rt.state.dropped[0].2, DragOperations::ALL);
}

#[test]
fn drop_feedback_returning_none_refuses_headless_drop_and_constrains_to_allowed() {
    #[derive(Default)]
    struct App {
        allow_drop: bool,
        drops: usize,
    }

    fn view(_: &App, _: &mut Cx<App>) -> El {
        el().key("zone")
            .size(100.0, 100.0)
            .on_drop_files(|s: &mut App, _, _| s.drops += 1)
            .drop_feedback(|s: &App, _| {
                if s.allow_drop {
                    DropFeedback::move_files()
                } else {
                    DropFeedback::none().description(DropDescription::none("Read-only folder"))
                }
            })
    }

    let mut rt = Runtime::new(App::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    rt.drag_files_with(vec!["x.txt".into()], Some((20.0, 20.0)), Mods::default(), DragOperations::ALL);
    rt.frame(0.016);
    let fb = rt.drop_feedback().expect("feedback evaluated");
    assert_eq!(fb.operation(), None);
    assert_eq!(fb.drop_description().map(DropDescription::kind), Some(DropDescriptionKind::None));

    // Dropping while drop_feedback returns None must not invoke on_drop_files.
    rt.drop_files_with(vec!["x.txt".into()], Some((20.0, 20.0)), Mods::default(), DragOperations::ALL, None);
    rt.frame(0.016);
    assert_eq!(rt.state.drops, 0);

    // Even when allow_drop is true and target asks for Move, if source only allows Copy the
    // operation is constrained to None.
    rt.state.allow_drop = true;
    rt.drag_files_with(vec!["x.txt".into()], Some((20.0, 20.0)), Mods::default(), DragOperations::COPY);
    rt.frame(0.016);
    assert_eq!(rt.drop_feedback().and_then(DropFeedback::operation), None);
}

#[test]
fn tx_set_drop_operation_and_description_work_from_hover_and_move_and_clear_on_target_change() {
    #[derive(Default)]
    struct App {
        seen_drop_op: Option<DragOperation>,
    }

    fn view(_: &App, _: &mut Cx<App>) -> El {
        col().size(200.0, 200.0).children([
            el().key("a")
                .size(200.0, 100.0)
                .on_drop_files(|s: &mut App, _, tx| s.seen_drop_op = tx.drop_operation())
                .on_drop_hover(|_: &mut App, over, tx| {
                    if over {
                        tx.set_drop_operation(Some(DragOperation::Move));
                        tx.set_drop_description(Some(DropDescription::move_to("Folder A")));
                    }
                }),
            el().key("b")
                .size(200.0, 100.0)
                .on_drop_files(|s: &mut App, _, tx| s.seen_drop_op = tx.drop_operation()),
        ])
    }

    let mut rt = Runtime::new(App::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Hover A -> Move to Folder A.
    rt.drag_files_with(vec!["f.txt".into()], Some((50.0, 25.0)), Mods::default(), DragOperations::ALL);
    rt.frame(0.016);
    assert_eq!(
        rt.drop_feedback(),
        Some(&DropFeedback::move_files().description(DropDescription::move_to("Folder A"))),
    );

    // Move onto B -> A's override and description clear automatically; B uses Copy default.
    assert!(rt.drag_files_to_with(Some((50.0, 150.0)), Mods::default(), DragOperations::ALL));
    rt.frame(0.016);
    assert_eq!(rt.drop_feedback(), Some(&DropFeedback::copy()));

    // Move back to A and drop -> on_drop_files receives Move.
    assert!(rt.drag_files_to_with(Some((50.0, 25.0)), Mods::default(), DragOperations::ALL));
    rt.frame(0.016);
    rt.drop_files_with(vec!["f.txt".into()], Some((50.0, 25.0)), Mods::default(), DragOperations::ALL, None);
    rt.frame(0.016);
    assert_eq!(rt.state.seen_drop_op, Some(DragOperation::Move));
}

#[test]
fn plain_on_drop_files_stays_copy_only_even_with_shift_held() {
    #[derive(Default)]
    struct App {
        op: Option<DragOperation>,
    }

    fn view(_: &App, _: &mut Cx<App>) -> El {
        el().key("plain")
            .size(100.0, 100.0)
            .on_drop_files(|s: &mut App, _, tx| s.op = tx.drop_operation())
    }

    let mut rt = Runtime::new(App::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let shift = Mods { shift: true, ..Mods::default() };
    rt.drag_files_with(vec!["f.txt".into()], Some((20.0, 20.0)), shift, DragOperations::ALL);
    rt.frame(0.016);
    // Unchanged default: plain on_drop_files never silently asks the OS to Move/delete sources.
    assert_eq!(rt.drop_feedback(), Some(&DropFeedback::copy()));

    rt.drop_files_with(vec!["f.txt".into()], Some((20.0, 20.0)), shift, DragOperations::ALL, None);
    rt.frame(0.016);
    assert_eq!(rt.state.op, Some(DragOperation::Copy));
}
