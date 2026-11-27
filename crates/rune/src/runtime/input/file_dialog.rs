use super::*;
use crate::file_dialog::{DialogAnswer, DialogOutcome, DialogState, FileDialogError, FileDialogKind, FileDialogOptions, FileHandle, Pending, Request};

impl RuntimeCore {
    /// The file dialog showing, or about to show, from [`Tx::open_file`], [`Tx::open_files`],
    /// [`Tx::save_file`] or [`Tx::open_folder`].
    pub fn file_dialog(&self) -> Option<(FileDialogKind, &FileDialogOptions)> {
        self.host.file_dialog.pending().map(|d| (d.kind, &d.options))
    }

    /// True when the pending file dialog allows selecting multiple files ([`Tx::open_files`]).
    ///
    /// ```no_run
    /// # fn check(rt: &rune::Runtime<()>) {
    /// assert!(rt.file_dialog_multiple());
    /// # }
    /// ```
    pub fn file_dialog_multiple(&self) -> bool {
        self.host.file_dialog.pending().is_some_and(|d| d.multiple)
    }

    /// True when the pending file dialog accepts platform-neutral [`FileHandle`]s
    /// ([`Tx::pick_file`], [`Tx::pick_files`], [`Tx::pick_save_file`]), so a host may answer
    /// it with documents that have no local filesystem path.
    pub fn file_dialog_handles(&self) -> bool {
        self.host.file_dialog.pending().is_some_and(|d| d.handles)
    }

    /// Answers the file dialog showing as if the user chose `path`, or cancelled with `None`.
    /// Its callback runs on the next frame. For tests and headless drivers; native dialogs
    /// answer themselves.
    pub fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), FileDialogError> {
        let id = self.host.file_dialog.pending().ok_or_else(|| FileDialogError("no file dialog is open".into()))?.id;
        self.answer_file_dialog(id, Ok(DialogAnswer::Single(path)));
        Ok(())
    }

    /// Answers the open file dialog as if the user chose `paths`, or cancelled with `None`.
    /// For a single-file dialog, the first path (if any) is delivered. Its callback runs on
    /// the next frame. For tests and headless drivers; native dialogs answer themselves.
    ///
    /// ```no_run
    /// # fn answer(rt: &mut rune::Runtime<()>) {
    /// rt.resolve_file_dialog_files(Some(vec!["/a.csv".into(), "/b.csv".into()])).unwrap();
    /// rt.frame(1.0 / 60.0);
    /// # }
    /// ```
    pub fn resolve_file_dialog_files(&mut self, paths: Option<Vec<std::path::PathBuf>>) -> Result<(), FileDialogError> {
        let id = self.host.file_dialog.pending().ok_or_else(|| FileDialogError("no file dialog is open".into()))?.id;
        self.answer_file_dialog(id, Ok(DialogAnswer::Many(paths)));
        Ok(())
    }

    /// Answers the pending file dialog with a single [`FileHandle`], or cancels with `None`.
    /// Returns an error if no dialog is open, or if the dialog requires local paths and the
    /// handle has `path().is_none()`.
    pub fn resolve_file_dialog_handle(&mut self, handle: Option<FileHandle>) -> Result<(), FileDialogError> {
        self.resolve_file_dialog_handles(handle.map(|h| vec![h]))
    }

    /// Answers the pending file dialog with one or more [`FileHandle`]s, or cancels with `None`.
    /// Returns an error if no dialog is open, or if the dialog requires local paths (`open_file`,
    /// `open_files`, `save_file`, `open_folder`) and any supplied handle has `path().is_none()`.
    pub fn resolve_file_dialog_handles(&mut self, handles: Option<Vec<FileHandle>>) -> Result<(), FileDialogError> {
        let pending = self.host.file_dialog.pending().ok_or_else(|| FileDialogError("no file dialog is open".into()))?;
        if !pending.handles && let Some(list) = &handles && list.iter().any(|h| h.path().is_none()) {
            return Err(FileDialogError("this file dialog requires local filesystem paths".into()));
        }
        let id = pending.id;
        self.answer_file_dialog(id, Ok(DialogAnswer::Handles(handles)));
        Ok(())
    }

    /// Takes a handler's request. One dialog at a time: a request while one is open is
    /// answered with an error rather than queued, so a double click opens one dialog.
    pub(crate) fn request_file_dialog(&mut self, request: Request) {
        self.host.next_file_dialog += 1;
        let id = self.host.next_file_dialog;
        self.host.file_dialog_answers.push((id, request.on_answer));
        if self.host.native_overlay {
            self.trace_event("file-dialog:overlay");
            self.frame.queue.push(Action::FileDialog(id, Err(FileDialogError("overlays cannot open file dialogs".into()))));
            return;
        }
        if self.host.file_dialog.is_open() {
            self.trace_event("file-dialog:busy");
            self.frame.queue.push(Action::FileDialog(id, Err(FileDialogError("a file dialog is already open".into()))));
            return;
        }
        self.trace_event(format_args!("file-dialog:open:{:?}", request.kind));
        self.host.file_dialog = DialogState::Requested(Pending {
            id,
            kind: request.kind,
            multiple: request.multiple,
            handles: request.handles,
            options: request.options,
        });
    }

    /// The dialog for the host to show, marked shown. Nothing while it shows.
    pub(crate) fn take_file_dialog(&mut self) -> Option<(u64, FileDialogKind, bool, FileDialogOptions)> {
        let d = self.host.file_dialog.show()?;
        Some((d.id, d.kind, d.multiple, d.options.clone()))
    }

    /// Queues dialog `id`'s answer for the next frame, closing it.
    pub(crate) fn answer_file_dialog(&mut self, id: u64, result: DialogOutcome) {
        self.host.file_dialog.answered(id);
        self.frame.queue.push(Action::FileDialog(id, result));
    }

    /// Answers a dialog this host can't show with `why`.
    pub(crate) fn refuse_file_dialog(&mut self, why: &str) {
        if let Some((id, ..)) = self.take_file_dialog() {
            self.answer_file_dialog(id, Err(FileDialogError(why.into())));
        }
    }

    pub(in crate::runtime) fn run_file_dialog(&mut self, app: &mut dyn AppDriver, id: u64, result: DialogOutcome, one: &mut Tx) -> bool {
        let Some(i) = self.host.file_dialog_answers.iter().position(|(dialog, _)| *dialog == id) else { return false };
        let (_, f) = self.host.file_dialog_answers.remove(i);
        self.trace_event(format_args!(
            "file-dialog:answer:{}",
            if result.as_ref().is_ok_and(DialogAnswer::is_some) { "path" } else if result.is_ok() { "cancel" } else { "error" }
        ));
        f(app.states(), result, one);
        true
    }

    /// Answers the open or waiting dialog with `Ok(None)`, and runs any queued file dialog
    /// callbacks now while the window's state is still alive. A callback that asks for another
    /// dialog is answered too, up to a bound against one that always asks again.
    pub(crate) fn cancel_file_dialogs(&mut self, app: &mut dyn AppDriver) {
        for _ in 0..64 {
            if let Some(d) = self.host.file_dialog.take() {
                self.frame.queue.push(Action::FileDialog(d.id, Ok(DialogAnswer::Single(None))));
            }
            let Some(pos) = self.frame.queue.iter().position(|a| matches!(a, Action::FileDialog(..))) else { return };
            let Action::FileDialog(id, result) = self.frame.queue.remove(pos) else { unreachable!() };
            let mut one = self.transaction(app);
            self.run_file_dialog(app, id, result, &mut one);
            self.latch_requests(app, &mut one);
            merge(&mut self.frame.pending, one);
            self.commit.dirty = true;
        }
        eprintln!(
            "rune: file dialog callbacks kept asking for dialogs while their window went away; dropping {}",
            self.host.file_dialog_answers.len()
        );
        self.host.file_dialog = DialogState::Closed;
        self.host.file_dialog_answers.clear();
        self.frame.queue.retain(|a| !matches!(a, Action::FileDialog(..)));
    }

    pub(crate) fn file_dialog_json(&self) -> Option<serde_json::Value> {
        self.host.file_dialog.pending().map(Pending::to_json)
    }
}
