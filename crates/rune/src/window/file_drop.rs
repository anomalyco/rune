//! A file drag from another app: what the host gathers between passes.

use std::path::PathBuf;
use std::time::Duration;

/// A file drag from another app over the window. winit reports one event per file; the host
/// gathers them and hands the runtime one drag and one drop (`Host::flush_files`).
#[derive(Default)]
pub(super) struct FileDrop {
    /// What winit reported since the last pass, in order.
    pub(super) steps: Vec<FileStep>,
    /// Samples the pointer while the drag is over the window, and where it last was.
    pub(super) pointer: Option<crate::input::drop_pointer::DropPointer>,
    pub(super) at: Option<(f32, f32)>,
}

/// One step of a file drag over a window. Native drop targets (AppKit, Win32 OLE, Wayland
/// `wl_data_device`) supply the pointer's place in window points (`Some(at)`); winit's fallback
/// events (`HoveredFile`, `DroppedFile`) omit it (`None`), so the host samples the pointer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FileStep {
    Enter(Vec<PathBuf>, Option<(f32, f32)>),
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    EnterWith {
        paths: Vec<PathBuf>,
        at: (f32, f32),
        mods: crate::Mods,
        allowed: crate::DragOperations,
    },
    #[cfg_attr(not(any(test, target_os = "macos", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    Move((f32, f32)),
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    MoveWith {
        at: (f32, f32),
        mods: crate::Mods,
        allowed: crate::DragOperations,
    },
    Drop(Vec<PathBuf>, Option<(f32, f32)>),
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    DropWith {
        paths: Vec<PathBuf>,
        at: (f32, f32),
        mods: crate::Mods,
        allowed: crate::DragOperations,
        operation: Option<crate::DragOperation>,
    },
    Leave,
}

impl FileDrop {
    /// One file of a drag that entered (`drop` false) or dropped: the run of files it belongs
    /// to grows, or a new run starts.
    pub(super) fn file(&mut self, path: PathBuf, drop: bool) {
        match self.steps.last_mut() {
            Some(FileStep::Enter(files, None)) if !drop => files.push(path),
            Some(FileStep::Drop(files, None)) if drop => files.push(path),
            _ => self.steps.push(if drop { FileStep::Drop(vec![path], None) } else { FileStep::Enter(vec![path], None) }),
        }
    }

    /// Feeds gathered live file drag/drop steps into `rt` inside a live arrival-time scope so
    /// drop handlers see `Tx::input_time()` and `last_input()` updates. Returns whether any step
    /// queued work for the next frame.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(in crate::window) fn dispatch_steps(
        &mut self,
        rt: &mut crate::runtime::RuntimeCore,
        steps: Vec<FileStep>,
        mut enter_at: impl FnMut(&mut Self) -> Option<(f32, f32)>,
        mut drop_at: impl FnMut(&mut Self) -> Option<(f32, f32)>,
    ) -> bool {
        if steps.is_empty() { return false; }
        let _input = crate::input::time::arrived_now();
        let zoom = rt.zoom();
        let view_at = |(x, y): (f32, f32)| (x / zoom, y / zoom);
        let mut queued = false;
        for step in steps {
            queued |= match step {
                FileStep::Enter(paths, Some(p)) => { rt.drag_files(paths, Some(view_at(p))); true }
                FileStep::Enter(paths, None) => {
                    let at = enter_at(self).or_else(|| rt.pointer());
                    rt.drag_files(paths, at);
                    true
                }
                FileStep::EnterWith { paths, at, mods, allowed } => {
                    rt.drag_files_with(paths, Some(view_at(at)), mods, allowed);
                    true
                }
                FileStep::Move(p) => rt.drag_files_to(Some(view_at(p))),
                FileStep::MoveWith { at, mods, allowed } => rt.drag_files_to_with(Some(view_at(at)), mods, allowed),
                FileStep::Drop(paths, Some(p)) => { rt.drop_files(paths, Some(view_at(p))); true }
                FileStep::Drop(paths, None) => {
                    let at = drop_at(self).or_else(|| rt.pointer());
                    rt.drop_files(paths, at);
                    true
                }
                FileStep::DropWith { paths, at, mods, allowed, operation } => {
                    rt.drop_files_with(paths, Some(view_at(at)), mods, allowed, operation);
                    true
                }
                FileStep::Leave => {
                    self.pointer = None;
                    self.at = None;
                    rt.drag_files_leave();
                    true
                }
            };
        }
        queued
    }
}

/// How often a file drag's position is sampled while it's over the window.
pub(super) const DROP_SAMPLE: Duration = Duration::from_millis(16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_winit_files_coalesce_per_phase() {
        let mut drop = FileDrop::default();
        drop.file("a.txt".into(), false);
        drop.file("b.txt".into(), false);
        drop.file("a.txt".into(), true);
        drop.file("b.txt".into(), true);
        drop.steps.push(FileStep::Leave);
        assert_eq!(drop.steps, [
            FileStep::Enter(vec!["a.txt".into(), "b.txt".into()], None),
            FileStep::Drop(vec!["a.txt".into(), "b.txt".into()], None),
            FileStep::Leave,
        ]);
    }

    #[test]
    fn dispatch_steps_stamps_input_time_for_live_file_drag_and_drop() {
        use crate::{Cx, El, Env, Runtime, Tx, el};
        use crate::time::Instant;

        #[derive(Default)]
        struct DropSeen {
            hovers: Vec<(bool, Option<Instant>)>,
            moves: Vec<Option<Instant>>,
            drops: Vec<Option<Instant>>,
        }

        fn view(_: &DropSeen, _: &mut Cx<DropSeen>) -> El {
            el().key("zone")
                .size(100.0, 100.0)
                .on_drop_hover(|s: &mut DropSeen, on, tx: &mut Tx| s.hovers.push((on, tx.input_time())))
                .on_drop_move(|s: &mut DropSeen, _, tx: &mut Tx| s.moves.push(tx.input_time()))
                .on_drop_files(|s: &mut DropSeen, _, tx: &mut Tx| s.drops.push(tx.input_time()))
        }

        let mut rt = Runtime::new(DropSeen::default(), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);

        let mut drop = FileDrop::default();
        assert!(drop.dispatch_steps(
            rt.core_mut(),
            vec![
                FileStep::Enter(vec!["a.txt".into()], Some((20.0, 20.0))),
                FileStep::Move((30.0, 30.0)),
            ],
            |_| None,
            |_| None,
        ));
        assert_eq!(crate::input::time::current(), None, "dispatch_steps must restore scope");
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.hovers.len(), 1);
        assert!(rt.state.hovers[0].0 && rt.state.hovers[0].1.is_some(), "enter must stamp input_time");
        assert!(!rt.state.moves.is_empty(), "on_drop_move must run");
        assert!(rt.state.moves.iter().all(Option::is_some), "move must stamp input_time");

        assert!(drop.dispatch_steps(
            rt.core_mut(),
            vec![FileStep::Drop(vec!["a.txt".into()], Some((30.0, 30.0)))],
            |_| None,
            |_| None,
        ));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.drops.len(), 1);
        assert!(rt.state.drops[0].is_some(), "drop must stamp input_time");
        assert!(crate::input::time::last_input().is_some(), "live file drop records last_input");

        // Enter then leave also stamps the leave handler.
        assert!(drop.dispatch_steps(
            rt.core_mut(),
            vec![FileStep::Enter(vec!["b.txt".into()], None)],
            |_| Some((20.0, 20.0)),
            |_| None,
        ));
        rt.frame(1.0 / 60.0);
        assert!(drop.dispatch_steps(
            rt.core_mut(),
            vec![FileStep::Leave],
            |_| None,
            |_| None,
        ));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.hovers.last().is_some_and(|&(on, at)| !on && at.is_some()), "leave must stamp input_time");
    }
}
