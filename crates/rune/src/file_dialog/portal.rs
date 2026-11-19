//! xdg-desktop-portal FileChooser over zbus. Dialog tasks run on the shared portal worker
//! runtime (`platform::portal::spawn`), so any executor can poll them, the event loop never
//! blocks, and no thread is leaked per call. Dropping the future closes the portal request.
use super::{FileDialogError, FileDialogKind, FileDialogOptions};
use super::portal_format::{filters, outcome, parent_window, path_bytes};
use crate::platform::portal::{self, DEST, PATH, RequestError, RequestTimeouts};
use std::{collections::HashMap, path::PathBuf, sync::atomic::{AtomicU32, Ordering}, time::Duration};
use tokio::sync::oneshot;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use zbus::{Connection, OwnedVariant, Proxy, Variant};

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
    else { let _ = PARENT.compare_exchange(id, 0, Ordering::Relaxed, Ordering::Relaxed); }
}

fn unavailable() -> FileDialogError { FileDialogError("no XDG desktop portal FileChooser is available".into()) }
fn dbus_error(e: zbus::Error) -> FileDialogError {
    if portal::is_unavailable(&e) {
        return unavailable();
    }
    FileDialogError(e.to_string())
}

fn request_options<'a>(kind: FileDialogKind, options: &'a FileDialogOptions, token: &'a str) -> HashMap<&'static str, Variant<'a>> {
    let mut map = HashMap::from([("handle_token", Variant::from(token)), ("modal", Variant::from(true))]);
    let filters = filters(options);
    if !filters.is_empty() && kind != FileDialogKind::Folder { map.insert("filters", Variant::from(filters)); }
    if let Some(bytes) = options.directory.as_deref().and_then(path_bytes) {
        map.insert("current_folder", Variant::from(bytes));
    }
    if kind == FileDialogKind::Folder { map.insert("directory", Variant::from(true)); }
    if kind == FileDialogKind::Save && let Some(name) = options.file_name.as_deref().filter(|s| !s.is_empty()) {
        map.insert("current_name", Variant::from(name));
    }
    map
}
/// A reply's `uris`, empty when absent or mistyped.
fn uris(results: &HashMap<String, OwnedVariant>) -> Vec<String> {
    results.get("uris").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).unwrap_or_default()
}

pub(super) async fn show(kind: FileDialogKind, options: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
    let (reply, result) = oneshot::channel();
    let parent = parent_window(PARENT.load(Ordering::Relaxed));
    portal::spawn(move || run(kind, options, parent, reply)).map_err(FileDialogError)?;
    result.await.unwrap_or_else(|_| Err(FileDialogError("file dialog worker stopped".into())))
}

async fn run(kind: FileDialogKind, options: FileDialogOptions, parent: String, mut reply: oneshot::Sender<Result<Option<PathBuf>, FileDialogError>>) {
    let token = portal::next_token("file_dialog");
    let conn = match tokio::time::timeout(RPC_TIMEOUT, async {
        // No session bus means this desktop cannot supply the portal.
        tokio::select! {
            conn = Connection::session() => conn.map(Some).map_err(|_| unavailable()),
            _ = reply.closed() => Ok(None),
        }
    }).await {
        Ok(Ok(Some(conn))) => conn,
        Ok(Ok(None)) => return,
        Ok(Err(e)) => { let _ = reply.send(Err(e)); return; }
        Err(_) => { let _ = reply.send(Err(FileDialogError("file dialog portal timed out".into()))); return; }
    };
    let portal_proxy = match tokio::time::timeout(RPC_TIMEOUT, async {
        tokio::select! {
            proxy = Proxy::new(&conn, DEST, PATH, INTERFACE) => proxy.map(Some).map_err(dbus_error),
            _ = reply.closed() => Ok(None),
        }
    }).await {
        Ok(Ok(Some(proxy))) => proxy,
        Ok(Ok(None)) => { let _ = conn.close().await; return; }
        Ok(Err(e)) => { let _ = conn.close().await; let _ = reply.send(Err(e)); return; }
        Err(_) => { let _ = conn.close().await; let _ = reply.send(Err(FileDialogError("file dialog portal timed out".into()))); return; }
    };
    // A folder is OpenFile's `directory` choice (portal version 3).
    let method = match kind { FileDialogKind::Open | FileDialogKind::Folder => "OpenFile", FileDialogKind::Save => "SaveFile" };
    let body = (parent.as_str(), options.title.as_str(), request_options(kind, &options, &token));
    let response = portal::request(
        &conn,
        &portal_proxy,
        &token,
        method,
        &body,
        RequestTimeouts { rpc: RPC_TIMEOUT, response: None },
        reply.closed(),
    ).await;
    let _ = conn.close().await;
    match response {
        Ok((status, results)) => { let _ = reply.send(outcome(status, &uris(&results))); }
        Err(RequestError::Cancelled) => {}
        Err(RequestError::Dbus(e)) => { let _ = reply.send(Err(dbus_error(e))); }
        Err(RequestError::Unavailable) => { let _ = reply.send(Err(unavailable())); }
        Err(RequestError::TimedOut) => { let _ = reply.send(Err(FileDialogError("file dialog portal timed out".into()))); }
        Err(RequestError::Disconnected) => { let _ = reply.send(Err(FileDialogError("file dialog portal disconnected".into()))); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::fdo;
    #[test]
    fn reply_uris_are_read_from_the_results_map() {
        let list = vec!["file:///tmp/a%20b".to_string(), "file:///tmp/c".to_string()];
        let results = HashMap::from([("uris".to_string(), OwnedVariant::try_from(Variant::from(list.clone())).unwrap())]);
        assert_eq!(uris(&results), list);
        assert_eq!(outcome(0, &uris(&results)).unwrap(), Some(PathBuf::from("/tmp/a b")));
        assert!(uris(&HashMap::new()).is_empty());
        let mistyped = HashMap::from([("uris".to_string(), OwnedVariant::try_from(Variant::from(7u32)).unwrap())]);
        assert!(uris(&mistyped).is_empty());
    }
    #[test]
    fn options_carry_token_filters_folder_and_save_name() {
        let options = FileDialogOptions { title: "Import".into(), directory: Some("/home/a/Downloads".into()),
            file_name: Some("export.csv".into()), filters: vec![] }.filter("Exports", &["csv"]);
        let open = request_options(FileDialogKind::Open, &options, "t1");
        assert_eq!(open.get("handle_token"), Some(&Variant::from("t1")));
        assert_eq!(open.get("filters"), Some(&Variant::from(vec![("Exports".to_string(), vec![(0u32, "*.csv".to_string())])])));
        assert_eq!(open.get("current_folder"), Some(&Variant::from(b"/home/a/Downloads\0".to_vec())));
        assert!(!open.contains_key("current_name"));
        assert!(!open.contains_key("directory"));
        let folder = request_options(FileDialogKind::Folder, &options, "t4");
        assert_eq!(folder.get("directory"), Some(&Variant::from(true)));
        assert!(!folder.contains_key("filters"));
        assert_eq!(folder.get("current_folder"), Some(&Variant::from(b"/home/a/Downloads\0".to_vec())));
        let save = request_options(FileDialogKind::Save, &options, "t2");
        assert_eq!(save.get("current_name"), Some(&Variant::from("export.csv")));
        let empty = FileDialogOptions { directory: Some(PathBuf::new()), file_name: Some(String::new()), ..Default::default() }
            .filter("Empty", &[]);
        let empty_save = request_options(FileDialogKind::Save, &empty, "t3");
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
