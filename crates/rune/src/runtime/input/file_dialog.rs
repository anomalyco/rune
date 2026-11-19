use super::*;
use crate::file_dialog::{DialogState, FileDialogError, FileDialogKind, FileDialogOptions, FileDialogResult, Pending, Request};

impl RuntimeCore {
    /// The file dialog showing, or about to show, from [`Tx::open_file`], [`Tx::save_file`] or
    /// [`Tx::open_folder`].
    pub fn file_dialog(&self) -> Option<(FileDialogKind, &FileDialogOptions)> {
        self.host.file_dialog.pending().map(|d| (d.kind, &d.options))
    }

    /// Answers the file dialog showing as if the user chose `path`, or cancelled with `None`.
    /// Its callback runs on the next frame. For tests and headless drivers; native dialogs
    /// answer themselves.
    pub fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), FileDialogError> {
        let id = self.host.file_dialog.pending().ok_or_else(|| FileDialogError("no file dialog is open".into()))?.id;
        self.answer_file_dialog(id, Ok(path));
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
        self.host.file_dialog = DialogState::Requested(Pending { id, kind: request.kind, options: request.options });
    }

    /// The dialog for the host to show, marked shown. Nothing while it shows.
    pub(crate) fn take_file_dialog(&mut self) -> Option<(u64, FileDialogKind, FileDialogOptions)> {
        let d = self.host.file_dialog.show()?;
        Some((d.id, d.kind, d.options.clone()))
    }

    /// Queues dialog `id`'s answer for the next frame, closing it.
    pub(crate) fn answer_file_dialog(&mut self, id: u64, result: FileDialogResult) {
        self.host.file_dialog.answered(id);
        self.frame.queue.push(Action::FileDialog(id, result));
    }

    /// Answers a dialog this host can't show with `why`.
    pub(crate) fn refuse_file_dialog(&mut self, why: &str) {
        if let Some((id, ..)) = self.take_file_dialog() {
            self.answer_file_dialog(id, Err(FileDialogError(why.into())));
        }
    }

    pub(in crate::runtime) fn run_file_dialog(&mut self, app: &mut dyn AppDriver, id: u64, result: FileDialogResult, one: &mut Tx) -> bool {
        let Some(i) = self.host.file_dialog_answers.iter().position(|(dialog, _)| *dialog == id) else { return false };
        let (_, f) = self.host.file_dialog_answers.remove(i);
        self.trace_event(format_args!(
            "file-dialog:answer:{}",
            if result.as_ref().is_ok_and(Option::is_some) { "path" } else if result.is_ok() { "cancel" } else { "error" }
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
                self.frame.queue.push(Action::FileDialog(d.id, Ok(None)));
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
