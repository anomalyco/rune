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

/// One step of a file drag over a window. Native drop targets (AppKit, Win32 OLE) supply the
/// pointer's place in window points (`Some(at)`); winit's fallback events (`HoveredFile`,
/// `DroppedFile`) omit it (`None`), so the host samples the pointer.
#[derive(Debug, PartialEq)]
pub(crate) enum FileStep {
    Enter(Vec<PathBuf>, Option<(f32, f32)>),
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    Move((f32, f32)),
    Drop(Vec<PathBuf>, Option<(f32, f32)>),
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
}
