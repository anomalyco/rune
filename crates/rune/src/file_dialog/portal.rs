//! xdg-desktop-portal FileChooser over zbus. Dialog tasks run on the shared portal worker
//! (`platform::portal::spawn`), so any executor can poll them, the event loop never blocks,
//! and no thread is leaked per call. Dropping the future closes the portal request.
use super::{DialogAnswer, DialogOutcome, FileDialogError, FileDialogFilesResult, FileDialogKind, FileDialogOptions, FileDialogResult};
use super::portal_format::{filters, outcome, outcome_many, parent_window, path_bytes};
use crate::platform::portal::{self, DEST, PATH, RequestError, RequestTimeouts};
use futures_lite::FutureExt;
use std::{collections::HashMap, sync::atomic::{AtomicU32, Ordering}, time::Duration};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use zbus::{Connection, Proxy, zvariant::{OwnedValue, Value}};

const INTERFACE: &str = "org.freedesktop.portal.FileChooser";
/// Connecting, D-Bus activation of the portal and the method call. The user's choice itself
/// has no timeout.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// The focused X11 window, the dialog's parent. 0 is none, and Wayland windows stay 0.
static PARENT: AtomicU32 = AtomicU32::new(0);

pub(crate) fn window_focus(window: &winit::window::Window, focused: bool) {
    let Ok(handle) = window.window_handle() else { return };
    let id = match handle.as_raw() {
        RawWindowHandle::Xlib(w) => u32::try_from(w.window).unwrap_or(0),
        RawWindowHandle::Xcb(w) => w.window.get(),
        _ => 0,
    };
    if id == 0 { return; }
    if focused { PARENT.store(id, Ordering::Relaxed); }
    else { PARENT.compare_exchange(id, 0, Ordering::Relaxed, Ordering::Relaxed).ok(); }
}

fn unavailable() -> FileDialogError { FileDialogError("no XDG desktop portal FileChooser is available".into()) }
fn dbus_error(e: zbus::Error) -> FileDialogError {
    if portal::is_unavailable(&e) {
        return unavailable();
    }
    FileDialogError(e.to_string())
}

fn request_options<'a>(kind: FileDialogKind, multiple: bool, options: &'a FileDialogOptions, token: &'a str) -> HashMap<&'static str, Value<'a>> {
    let mut map = HashMap::from([("handle_token", Value::from(token)), ("modal", Value::from(true))]);
    let filters = filters(options);
    if !filters.is_empty() && kind != FileDialogKind::Folder { map.insert("filters", Value::from(filters)); }
    if let Some(bytes) = options.directory.as_deref().and_then(path_bytes) {
        map.insert("current_folder", Value::from(bytes));
    }
    if multiple && kind == FileDialogKind::Open { map.insert("multiple", Value::from(true)); }
    if kind == FileDialogKind::Folder { map.insert("directory", Value::from(true)); }
    if kind == FileDialogKind::Save && let Some(name) = options.file_name.as_deref().filter(|s| !s.is_empty()) {
        map.insert("current_name", Value::from(name));
    }
    map
}
/// A reply's `uris`, empty when absent or mistyped.
fn uris(results: &HashMap<String, OwnedValue>) -> Vec<String> {
    results.get("uris").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).unwrap_or_default()
}

pub(super) async fn show(kind: FileDialogKind, options: FileDialogOptions) -> FileDialogResult {
    show_outcome(kind, false, options).await.map(DialogAnswer::into_single)
}

pub(super) async fn show_files(options: FileDialogOptions) -> FileDialogFilesResult {
    show_outcome(FileDialogKind::Open, true, options).await.map(DialogAnswer::into_many)
}

pub(super) async fn show_outcome(kind: FileDialogKind, multiple: bool, options: FileDialogOptions) -> DialogOutcome {
    let (reply, result) = async_channel::bounded(1);
    let parent = parent_window(PARENT.load(Ordering::Relaxed));
    portal::spawn(move || run(kind, multiple, options, parent, reply)).map_err(FileDialogError)?;
    result.recv().await.unwrap_or_else(|_| Err(FileDialogError("file dialog worker stopped".into())))
}

async fn run(kind: FileDialogKind, multiple: bool, options: FileDialogOptions, parent: String, reply: async_channel::Sender<DialogOutcome>) {
    let token = portal::next_token("file_dialog");
    let conn = match portal::timeout(RPC_TIMEOUT, async {
        reply.closed().await;
        Ok(None)
    }.or(async {
        // No session bus means this desktop cannot supply the portal.
        Connection::session().await.map(Some).map_err(|_| unavailable())
    })).await {
        Some(Ok(Some(conn))) => conn,
        Some(Ok(None)) => return,
        Some(Err(e)) => { reply.try_send(Err(e)).ok(); return; }
        None => { reply.try_send(Err(FileDialogError("file dialog portal timed out".into()))).ok(); return; }
    };
    let portal_proxy = match portal::timeout(RPC_TIMEOUT, async {
        reply.closed().await;
        Ok(None)
    }.or(async {
        portal::register_app_id(&conn).await;
        Proxy::new(&conn, DEST, PATH, INTERFACE).await.map(Some).map_err(dbus_error)
    })).await {
        Some(Ok(Some(proxy))) => proxy,
        Some(Ok(None)) => { conn.close().await.ok(); return; }
        Some(Err(e)) => { conn.close().await.ok(); reply.try_send(Err(e)).ok(); return; }
        None => { conn.close().await.ok(); reply.try_send(Err(FileDialogError("file dialog portal timed out".into()))).ok(); return; }
    };
    // A folder is OpenFile's `directory` choice (portal version 3).
    let method = match kind { FileDialogKind::Open | FileDialogKind::Folder => "OpenFile", FileDialogKind::Save => "SaveFile" };
    let body = (parent.as_str(), options.title.as_str(), request_options(kind, multiple, &options, &token));
    let response = portal::request(
        &conn,
        &portal_proxy,
        &token,
        method,
        &body,
        RequestTimeouts { rpc: RPC_TIMEOUT, response: None },
        reply.closed(),
    ).await;
    conn.close().await.ok();
    match response {
        Ok((status, results)) => {
            let reply_uris = uris(&results);
            let answer = if multiple && kind == FileDialogKind::Open {
                outcome_many(status, &reply_uris).map(DialogAnswer::Many)
            } else {
                outcome(status, &reply_uris).map(DialogAnswer::Single)
            };
            reply.try_send(answer).ok();
        }
        Err(RequestError::Cancelled) => {}
        Err(RequestError::Dbus(e)) => { reply.try_send(Err(dbus_error(e))).ok(); }
        Err(RequestError::Unavailable) => { reply.try_send(Err(unavailable())).ok(); }
        Err(RequestError::TimedOut) => { reply.try_send(Err(FileDialogError("file dialog portal timed out".into()))).ok(); }
        Err(RequestError::Disconnected) => { reply.try_send(Err(FileDialogError("file dialog portal disconnected".into()))).ok(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use zbus::fdo;
    #[test]
    fn reply_uris_are_read_from_the_results_map() {
        let list = vec!["file:///tmp/a%20b".to_string(), "file:///tmp/c".to_string()];
        let results = HashMap::from([("uris".to_string(), OwnedValue::try_from(Value::from(list.clone())).unwrap())]);
        assert_eq!(uris(&results), list);
        assert_eq!(outcome(0, &uris(&results)).unwrap(), Some(PathBuf::from("/tmp/a b")));
        assert_eq!(
            outcome_many(0, &uris(&results)).unwrap(),
            Some(vec![PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c")])
        );
        assert!(uris(&HashMap::new()).is_empty());
        let mistyped = HashMap::from([("uris".to_string(), OwnedValue::try_from(Value::from(7u32)).unwrap())]);
        assert!(uris(&mistyped).is_empty());
    }
    #[test]
    fn options_carry_token_filters_folder_multiple_and_save_name() {
        let options = FileDialogOptions { title: "Import".into(), directory: Some("/home/a/Downloads".into()),
            file_name: Some("export.csv".into()), filters: vec![] }.filter("Exports", &["csv"]);
        let open = request_options(FileDialogKind::Open, false, &options, "t1");
        assert_eq!(open.get("handle_token"), Some(&Value::from("t1")));
        assert_eq!(open.get("filters"), Some(&Value::from(vec![("Exports".to_string(), vec![(0u32, "*.csv".to_string())])])));
        assert_eq!(open.get("current_folder"), Some(&Value::from(b"/home/a/Downloads\0".to_vec())));
        assert!(!open.contains_key("current_name"));
        assert!(!open.contains_key("directory"));
        assert!(!open.contains_key("multiple"));
        let open_many = request_options(FileDialogKind::Open, true, &options, "t5");
        assert_eq!(open_many.get("multiple"), Some(&Value::from(true)));
        assert_eq!(open_many.get("filters"), Some(&Value::from(vec![("Exports".to_string(), vec![(0u32, "*.csv".to_string())])])));
        assert_eq!(open_many.get("current_folder"), Some(&Value::from(b"/home/a/Downloads\0".to_vec())));
        assert!(!open_many.contains_key("current_name"));
        assert!(!open_many.contains_key("directory"));
        let folder = request_options(FileDialogKind::Folder, true, &options, "t4");
        assert_eq!(folder.get("directory"), Some(&Value::from(true)));
        assert!(!folder.contains_key("filters"));
        assert!(!folder.contains_key("multiple"));
        assert_eq!(folder.get("current_folder"), Some(&Value::from(b"/home/a/Downloads\0".to_vec())));
        let save = request_options(FileDialogKind::Save, true, &options, "t2");
        assert_eq!(save.get("current_name"), Some(&Value::from("export.csv")));
        assert!(!save.contains_key("multiple"));
        let empty = FileDialogOptions { directory: Some(PathBuf::new()), file_name: Some(String::new()), ..Default::default() }
            .filter("Empty", &[]);
        let empty_save = request_options(FileDialogKind::Save, false, &empty, "t3");
        assert!(!empty_save.contains_key("filters"));
        assert!(!empty_save.contains_key("current_folder"));
        assert!(!empty_save.contains_key("current_name"));
    }
    #[test]
    fn missing_portal_is_a_clear_error() {
        let fdo = |e| zbus::Error::FDO(Box::new(e));
        assert_eq!(dbus_error(fdo(fdo::Error::ServiceUnknown("portal".into()))), unavailable());
        assert_eq!(dbus_error(fdo(fdo::Error::UnknownMethod("OpenFile".into()))), unavailable());
        assert_eq!(dbus_error(fdo(fdo::Error::SpawnChildExited("portal".into()))), unavailable());
        assert_ne!(dbus_error(fdo(fdo::Error::Failed("x".into()))), unavailable());
    }
}
