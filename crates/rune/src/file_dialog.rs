//! Inject native dialogs in the shell, scripted responses in headless fixtures.
//! Native futures use rfd on macOS and Windows, and the XDG desktop portal's FileChooser
//! over zbus on Linux.
//!
//! A handler can also ask with [`Tx::open_file`], [`Tx::open_files`], [`Tx::save_file`] or
//! [`Tx::open_folder`]: the window shows the dialog once the handler's frame is done, and the
//! answer runs the callback as an action on a later frame. Headless runs keep the dialog open in
//! `--tree` (`fileDialog`) until a script's `{"type": "file-dialog", "path": …}`,
//! `{"type": "file-dialog", "paths": […]}` or `{"type": "file-dialog-cancel"}` answers it.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crate::Tx;
use crate::runtime::Waker;

#[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
mod portal;
#[cfg(any(all(feature = "native-file-dialog", target_os = "linux"), all(test, unix)))]
mod portal_format;
#[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
pub(crate) use portal::window_focus;
#[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
use portal::{show as native, show_files as native_files, show_outcome as portal_show_outcome};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDialogOptions {
    pub title: String,
    pub directory: Option<PathBuf>,
    pub file_name: Option<String>,
    pub filters: Vec<(String, Vec<String>)>,
}
impl FileDialogOptions {
    pub fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push((name.into(), extensions.iter().map(|s| (*s).into()).collect())); self
    }
}

/// Which dialog: Open picks an existing file, Save names a file to write, Folder picks an
/// existing folder (its options' filters and file name don't apply).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDialogKind { Open, Save, Folder }

impl FileDialogKind {
    fn name(self) -> &'static str { match self { Self::Open => "open", Self::Save => "save", Self::Folder => "folder" } }
}

/// Expected request and its response. `None` is cancellation, not a missing fixture.
#[derive(Clone, Debug)]
pub struct ScriptedFileDialog {
    pub kind: FileDialogKind,
    pub path: Option<PathBuf>,
}

#[derive(Clone)]
struct ScriptedEntry {
    kind: FileDialogKind,
    multiple: bool,
    paths: Option<Vec<PathBuf>>,
}

#[derive(Clone)]
pub struct FileDialogs(Backend);
#[derive(Clone)]
enum Backend {
    #[cfg(feature = "native-file-dialog")]
    Native,
    Scripted(Arc<Mutex<VecDeque<ScriptedEntry>>>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDialogError(pub String);
impl std::fmt::Display for FileDialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for FileDialogError {}

impl FileDialogs {
    /// Native dialogs are opt-in at the app's capability boundary. Headless apps inject
    /// `scripted` instead, so tests never depend on filesystem UI or a display server.
    /// Poll asynchronously: blocking the window's main thread (for example with
    /// `pollster::block_on`) deadlocks Cocoa's dialog presentation. On Linux, `open`,
    /// `open_files` and `save` return an error when the desktop has no XDG portal FileChooser.
    #[cfg(feature = "native-file-dialog")]
    pub fn native() -> Self { Self(Backend::Native) }
    pub fn scripted(responses: impl IntoIterator<Item = ScriptedFileDialog>) -> Self {
        let entries = responses.into_iter().map(|r| ScriptedEntry {
            kind: r.kind,
            multiple: false,
            paths: r.path.map(|p| vec![p]),
        }).collect();
        Self(Backend::Scripted(Arc::new(Mutex::new(entries))))
    }
    /// Headless fixture for [`FileDialogs::open_files`]: each entry is a list of chosen paths, or
    /// `None` for cancellation.
    ///
    /// ```
    /// # use std::path::PathBuf;
    /// # use rune::{FileDialogOptions, FileDialogs};
    /// let dialogs = FileDialogs::scripted_files([
    ///     Some(vec![PathBuf::from("/fixture/a.csv"), PathBuf::from("/fixture/b.csv")]),
    ///     None,
    /// ]);
    /// # let _ = (dialogs, FileDialogOptions::default());
    /// ```
    pub fn scripted_files(responses: impl IntoIterator<Item = Option<Vec<PathBuf>>>) -> Self {
        let entries = responses.into_iter().map(|paths| ScriptedEntry {
            kind: FileDialogKind::Open,
            multiple: true,
            paths,
        }).collect();
        Self(Backend::Scripted(Arc::new(Mutex::new(entries))))
    }
    pub async fn open(&self, options: FileDialogOptions) -> FileDialogResult {
        self.show(FileDialogKind::Open, options).await
    }
    /// Show an Open dialog that lets the user pick multiple files at once. Returns `Ok(None)`
    /// when cancelled.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use rune::{FileDialogError, FileDialogOptions, FileDialogs};
    /// # fn import(_: PathBuf) {}
    /// # async fn example(dialogs: FileDialogs) -> Result<(), FileDialogError> {
    /// let options = FileDialogOptions { title: "Import".into(), ..Default::default() }.filter("CSV", &["csv"]);
    /// if let Some(paths) = dialogs.open_files(options).await? {
    ///     for path in paths { import(path); }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn open_files(&self, options: FileDialogOptions) -> FileDialogFilesResult {
        match &self.0 {
            Backend::Scripted(responses) => {
                let mut responses = responses.lock().unwrap();
                let response = responses.front().ok_or_else(|| FileDialogError("file dialog fixture exhausted".into()))?;
                if response.kind != FileDialogKind::Open || !response.multiple {
                    return Err(FileDialogError(format!("expected {:?} dialog, received multi-file Open", response.kind)));
                }
                let _ = options;
                Ok(responses.pop_front().unwrap().paths)
            }
            #[cfg(feature = "native-file-dialog")]
            Backend::Native => native_files(options).await,
        }
    }
    pub async fn save(&self, options: FileDialogOptions) -> FileDialogResult {
        self.show(FileDialogKind::Save, options).await
    }
    pub async fn open_folder(&self, options: FileDialogOptions) -> FileDialogResult {
        self.show(FileDialogKind::Folder, options).await
    }
    async fn show(&self, kind: FileDialogKind, options: FileDialogOptions) -> FileDialogResult {
        match &self.0 {
            Backend::Scripted(responses) => {
                let mut responses = responses.lock().unwrap();
                let response = responses.front().ok_or_else(|| FileDialogError("file dialog fixture exhausted".into()))?;
                if response.multiple {
                    return Err(FileDialogError(format!("expected multi-file {:?} dialog, received {kind:?}", response.kind)));
                }
                if response.kind != kind {
                    return Err(FileDialogError(format!("expected {:?} dialog, received {kind:?}", response.kind)));
                }
                let _ = options;
                Ok(responses.pop_front().unwrap().paths.and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) }))
            }
            #[cfg(feature = "native-file-dialog")]
            Backend::Native => native(kind, options).await,
        }
    }
}

#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
async fn native(kind: FileDialogKind, options: FileDialogOptions) -> FileDialogResult {
    rfd_show(rfd_dialog(options), kind).await
}
#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
async fn native_files(options: FileDialogOptions) -> FileDialogFilesResult {
    rfd_show_files(rfd_dialog(options)).await
}
#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
fn rfd_dialog(options: FileDialogOptions) -> rfd::AsyncFileDialog {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(options.title);
    if let Some(directory) = options.directory { dialog = dialog.set_directory(directory); }
    if let Some(file_name) = options.file_name { dialog = dialog.set_file_name(file_name); }
    for (name, extensions) in options.filters { dialog = dialog.add_filter(name, &extensions); }
    dialog
}
/// Calling this starts the dialog (on macOS, a sheet over the parent); awaiting reads it.
#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
fn rfd_show(dialog: rfd::AsyncFileDialog, kind: FileDialogKind) -> impl Future<Output = FileDialogResult> {
    let file: Pin<Box<dyn Future<Output = Option<rfd::FileHandle>> + Send>> = match kind {
        FileDialogKind::Open => Box::pin(dialog.pick_file()),
        FileDialogKind::Save => Box::pin(dialog.save_file()),
        FileDialogKind::Folder => Box::pin(dialog.pick_folder()),
    };
    async move { Ok(file.await.map(|file| file.path().to_path_buf())) }
}
/// Calling this starts the multi-file Open dialog (on macOS, a sheet over the parent); awaiting reads it.
#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
fn rfd_show_files(dialog: rfd::AsyncFileDialog) -> impl Future<Output = FileDialogFilesResult> {
    let files = dialog.pick_files();
    async move { Ok(files.await.map(|list| list.into_iter().map(|file| file.path().to_path_buf()).collect())) }
}
#[cfg(all(feature = "native-file-dialog", not(any(target_os = "linux", target_os = "macos", target_os = "windows"))))]
async fn native(_: FileDialogKind, _: FileDialogOptions) -> FileDialogResult {
    Err(FileDialogError("native file dialogs aren't supported on this platform".into()))
}
#[cfg(all(feature = "native-file-dialog", not(any(target_os = "linux", target_os = "macos", target_os = "windows"))))]
async fn native_files(_: FileDialogOptions) -> FileDialogFilesResult {
    Err(FileDialogError("native file dialogs aren't supported on this platform".into()))
}

/// What a [`Tx::open_file`], [`Tx::save_file`] or [`Tx::open_folder`] callback gets: the chosen path, `Ok(None)`
/// when the user cancels, or why no dialog could show.
pub type FileDialogResult = Result<Option<PathBuf>, FileDialogError>;

/// What a [`Tx::open_files`] or [`FileDialogs::open_files`] call returns: the chosen paths,
/// `Ok(None)` when the user cancels, or why no dialog could show.
///
/// ```
/// # use std::path::PathBuf;
/// # use rune::FileDialogFilesResult;
/// let picked: FileDialogFilesResult = Ok(Some(vec![PathBuf::from("a.csv"), PathBuf::from("b.csv")]));
/// assert_eq!(picked.unwrap().unwrap().len(), 2);
/// ```
pub type FileDialogFilesResult = Result<Option<Vec<PathBuf>>, FileDialogError>;

/// Internal dialog answer carried from a host or headless resolver to the pending callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogAnswer {
    Single(Option<PathBuf>),
    Many(Option<Vec<PathBuf>>),
}

impl DialogAnswer {
    pub(crate) fn into_single(self) -> Option<PathBuf> {
        match self {
            Self::Single(path) => path,
            Self::Many(paths) => paths.and_then(|mut list| if list.is_empty() { None } else { Some(list.remove(0)) }),
        }
    }

    pub(crate) fn into_many(self) -> Option<Vec<PathBuf>> {
        match self {
            Self::Single(path) => path.map(|p| vec![p]),
            Self::Many(paths) => paths,
        }
    }

    pub(crate) fn is_some(&self) -> bool {
        match self {
            Self::Single(path) => path.is_some(),
            Self::Many(paths) => paths.is_some(),
        }
    }
}

pub(crate) type DialogOutcome = Result<DialogAnswer, FileDialogError>;

pub(crate) type AnswerFn = Box<dyn FnOnce(crate::states::States<'_>, DialogOutcome, &mut Tx)>;

pub(crate) struct Request {
    pub kind: FileDialogKind,
    pub multiple: bool,
    pub options: FileDialogOptions,
    pub on_answer: AnswerFn,
}

/// A `Tx`'s dialog request. `Tx` is `Clone`; clones share the request and the runtime takes it once.
#[derive(Clone)]
pub(crate) struct Call(Rc<RefCell<Option<Request>>>);

impl Call {
    pub(crate) fn take(&self) -> Option<Request> { self.0.borrow_mut().take() }
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let request = self.0.borrow();
        f.debug_tuple("FileDialogCall").field(&request.as_ref().map(|r| (r.kind, r.multiple, &r.options))).finish()
    }
}

/// The runtime's dialog, from the request until its answer is queued.
pub(crate) struct Pending {
    pub id: u64,
    pub kind: FileDialogKind,
    pub multiple: bool,
    pub options: FileDialogOptions,
}

/// The runtime's one file dialog. Its answer carries its `id`, so an answer to a dialog that
/// has since closed changes nothing (OpenCode Desktop's
/// `packages/desktop/src/main/files/attachment-picker-machine.ts` fences picks the same way).
#[derive(Default)]
pub(crate) enum DialogState {
    /// None asked for, or the last one's answer is queued.
    #[default]
    Closed,
    /// Asked for; a host shows it next.
    Requested(Pending),
    /// A host has it on screen.
    Showing(Pending),
}

impl DialogState {
    pub(crate) fn pending(&self) -> Option<&Pending> {
        match self {
            DialogState::Closed => None,
            DialogState::Requested(p) | DialogState::Showing(p) => Some(p),
        }
    }

    pub(crate) fn is_open(&self) -> bool { !matches!(self, DialogState::Closed) }

    /// The host takes a requested dialog to show; nothing while one shows.
    pub(crate) fn show(&mut self) -> Option<&Pending> {
        let DialogState::Requested(_) = self else { return None };
        let DialogState::Requested(p) = std::mem::take(self) else { unreachable!() };
        *self = DialogState::Showing(p);
        self.pending()
    }

    /// Dialog `id` was answered: it closes, if it's the one open.
    pub(crate) fn answered(&mut self, id: u64) {
        if self.pending().is_some_and(|p| p.id == id) { *self = DialogState::Closed; }
    }

    /// Closes whatever is open, for its `None` answer.
    pub(crate) fn take(&mut self) -> Option<Pending> {
        match std::mem::take(self) {
            DialogState::Closed => None,
            DialogState::Requested(p) | DialogState::Showing(p) => Some(p),
        }
    }
}

impl Pending {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let o = &self.options;
        let filters: Vec<_> = o.filters.iter().map(|(name, ext)| serde_json::json!({ "name": name, "extensions": ext })).collect();
        let mut v = serde_json::json!({ "kind": self.kind.name(), "title": o.title, "filters": filters });
        if self.multiple { v["multiple"] = true.into(); }
        if let Some(dir) = &o.directory { v["directory"] = dir.display().to_string().into(); }
        if let Some(name) = &o.file_name { v["fileName"] = name.clone().into(); }
        v
    }
}

impl Tx {
    /// Show an Open dialog once this handler's frame is done. `on_answer` runs, as an action on
    /// a later frame, with the chosen file, `Ok(None)` when the user cancels (or the window
    /// hides or closes), or an error when no dialog could show: a second dialog asked for while
    /// one is open, a window that stays hidden, an overlay, the web, or a build without the
    /// `native-file-dialog` feature. Asked for in the same turn that shows a hidden window, it
    /// shows with the window. The window keeps drawing and handling events while the dialog is
    /// up: it's a sheet on macOS, owned by the window on Windows, and the desktop portal's
    /// chooser on Linux.
    ///
    /// ```ignore
    /// .on_press_tx(|_: &mut App, tx: &mut Tx| {
    ///     let options = FileDialogOptions { title: "Import".into(), ..Default::default() }.filter("Export", &["json"]);
    ///     tx.open_file(options, |s: &mut App, result, _| match result {
    ///         Ok(Some(path)) => s.import(path),
    ///         Ok(None) => {}
    ///         Err(e) => s.error = Some(e.to_string()),
    ///     });
    /// })
    /// ```
    pub fn open_file<S: 'static>(&mut self, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        self.file_dialog(FileDialogKind::Open, options, on_answer);
    }

    /// [`Tx::open_file`] with multiple selection enabled: the user can pick several files at
    /// once. `on_answer` receives `Ok(Some(paths))` with every chosen file, `Ok(None)` on
    /// cancellation (or when the window hides or closes), or an error when no dialog could show.
    /// In headless `--tree` output `fileDialog` includes `"multiple": true`, and scripts answer
    /// it with `{"type": "file-dialog", "paths": [...]}` or `{"type": "file-dialog-cancel"}`.
    ///
    /// ```
    /// # use std::path::PathBuf;
    /// # use rune::{FileDialogOptions, Tx, el};
    /// # struct App { error: Option<String> }
    /// # impl App { fn import_all(&mut self, _: Vec<PathBuf>) {} }
    /// # let _ = el()
    /// .on_press_tx(|_: &mut App, tx: &mut Tx| {
    ///     let options = FileDialogOptions { title: "Import".into(), ..Default::default() }.filter("CSV", &["csv"]);
    ///     tx.open_files(options, |s: &mut App, result, _| match result {
    ///         Ok(Some(paths)) => s.import_all(paths),
    ///         Ok(None) => {}
    ///         Err(e) => s.error = Some(e.to_string()),
    ///     });
    /// })
    /// # ;
    /// ```
    pub fn open_files<S: 'static>(&mut self, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogFilesResult, &mut Tx) + 'static) {
        let on_answer: AnswerFn = Box::new(move |s, result, tx| {
            // None once the playground has dropped the stage that asked.
            if let Some(s) = s.cast::<S>() { on_answer(s, result.map(DialogAnswer::into_many), tx); }
        });
        self.file_dialogs.push(Call(Rc::new(RefCell::new(Some(Request {
            kind: FileDialogKind::Open,
            multiple: true,
            options,
            on_answer,
        })))));
    }

    /// [`Tx::open_file`] for a Save dialog: the path to write, which may not exist yet.
    pub fn save_file<S: 'static>(&mut self, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        self.file_dialog(FileDialogKind::Save, options, on_answer);
    }

    /// [`Tx::open_file`] for a folder: an Open dialog that picks an existing folder, as an app
    /// opening a project or choosing where exports go. `options`' title and directory apply;
    /// its filters and file name don't. A script answers it as it does a file dialog.
    pub fn open_folder<S: 'static>(&mut self, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        self.file_dialog(FileDialogKind::Folder, options, on_answer);
    }

    fn file_dialog<S: 'static>(&mut self, kind: FileDialogKind, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        let on_answer: AnswerFn = Box::new(move |s, result, tx| {
            // None once the playground has dropped the stage that asked.
            if let Some(s) = s.cast::<S>() { on_answer(s, result.map(DialogAnswer::into_single), tx); }
        });
        self.file_dialogs.push(Call(Rc::new(RefCell::new(Some(Request {
            kind,
            multiple: false,
            options,
            on_answer,
        })))));
    }
}

/// One window's native file dialog: shows what its runtime asks for, and queues the answer
/// after `wake` brings the host back to poll it. The dialog's future owns copies of the
/// options and the window's handle, never the app's state or the runtime.
pub(crate) struct Presenter {
    showing: Option<(u64, Pin<Box<dyn Future<Output = DialogOutcome>>>)>,
    #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
    sheet_parent: Option<objc2::rc::Retained<objc2_app_kit::NSWindow>>,
    wake: std::task::Waker,
}

struct Wake(Waker);
impl std::task::Wake for Wake {
    fn wake(self: Arc<Self>) { (self.0)() }
    fn wake_by_ref(self: &Arc<Self>) { (self.0)() }
}

impl Presenter {
    pub(crate) fn new(wake: Waker) -> Self {
        Self {
            showing: None,
            #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
            sheet_parent: None,
            wake: Arc::new(Wake(wake)).into(),
        }
    }

    /// Queues the answer of a dialog that closed. True when one did.
    pub(crate) fn answer_core(&mut self, rt: &mut crate::runtime::RuntimeCore) -> bool {
        let Some((id, future)) = &mut self.showing else { return false };
        let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(&self.wake)) else { return false };
        let id = *id;
        self.showing = None;
        #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
        {
            self.sheet_parent = None;
        }
        rt.answer_file_dialog(id, result);
        true
    }

    /// Shows the runtime's dialog over `window`, starting it before this returns, so nothing
    /// blocks the event loop. A dialog asked for while the window is hidden waits for it to
    /// show. True when an answer was queued.
    pub(crate) fn open_core(&mut self, rt: &mut crate::runtime::RuntimeCore, window: Option<&winit::window::Window>) -> bool {
        let Some(window) = window.filter(|_| self.showing.is_none()) else { return false };
        let Some((id, kind, multiple, options)) = rt.take_file_dialog() else { return false };
        #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
        {
            self.sheet_parent = crate::platform::macos::ns_window(window);
        }
        self.showing = Some((id, show_over(kind, multiple, options, window)));
        self.answer_core(rt)
    }

    /// The window is going away (hidden, closed or quitting): any attached sheet is ended and
    /// the open or waiting dialog is answered `Ok(None)` while its state is alive.
    pub(crate) fn cancel_core(&mut self, rt: &mut crate::runtime::RuntimeCore, app: &mut dyn crate::runtime::AppDriver) {
        self.stop();
        rt.cancel_file_dialogs(app);
    }

    fn stop(&mut self) {
        self.showing = None;
        #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
        if let Some(window) = self.sheet_parent.take() {
            if let Some(sheet) = window.attachedSheet() {
                window.endSheet(&sheet);
            }
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) { self.stop(); }
}

fn show_over(kind: FileDialogKind, multiple: bool, options: FileDialogOptions, window: &winit::window::Window) -> Pin<Box<dyn Future<Output = DialogOutcome>>> {
    #[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
    return if multiple && kind == FileDialogKind::Open {
        let files = rfd_show_files(rfd_dialog(options).set_parent(window));
        Box::pin(async move { files.await.map(DialogAnswer::Many) })
    } else {
        let file = rfd_show(rfd_dialog(options).set_parent(window), kind);
        Box::pin(async move { file.await.map(DialogAnswer::Single) })
    };
    // The portal's parent is the focused window (`window_focus`).
    #[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
    return { let _ = window; Box::pin(portal_show_outcome(kind, multiple, options)) };
    #[cfg(all(feature = "native-file-dialog", not(any(target_os = "linux", target_os = "macos", target_os = "windows"))))]
    return {
        let _ = window;
        if multiple && kind == FileDialogKind::Open {
            Box::pin(async move { native_files(options).await.map(DialogAnswer::Many) })
        } else {
            Box::pin(async move { native(kind, options).await.map(DialogAnswer::Single) })
        }
    };
    #[cfg(not(feature = "native-file-dialog"))]
    {
        let _ = (kind, multiple, options, window);
        Box::pin(std::future::ready(Err(FileDialogError("file dialogs need Rune's native-file-dialog feature".into()))))
    }
}

#[cfg(test)]
mod tests;
