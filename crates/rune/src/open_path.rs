//! Open a local file or folder in its default application ([`Tx::open_path`]) or reveal it
//! selected in the system file manager ([`Tx::reveal_path`]).
//!
//! Only syntax is checked in the handler: empty paths return [`OpenPathError::Empty`], paths with
//! embedded NUL bytes return [`OpenPathError::Invalid`], and non-desktop targets (`wasm32`, iOS,
//! Android) return [`OpenPathError::Unsupported`]. No filesystem calls run on the UI thread during
//! a frame. Once the frame finishes, desktop hosts execute the request off the UI thread:
//!
//! - macOS: `NSWorkspace` (`openURL:configuration:completionHandler:` / `openURL:` and
//!   `activateFileViewerSelectingURLs:`), resolving and checking the path on a background worker
//!   thread first so the UI thread never stats the filesystem.
//! - Windows: a background STA worker thread running `ShellExecuteW` for open and
//!   `ILCreateFromPathW` + `SHOpenFolderAndSelectItems` for reveal.
//! - Linux: the shared portal worker (`org.freedesktop.portal.OpenURI.OpenFile` for open,
//!   `org.freedesktop.FileManager1.ShowItems` then `OpenURI.OpenDirectory` for reveal), falling
//!   back to `xdg-open` when D-Bus or the portal is unavailable.
//!
//! Headless runs and tests never launch external apps: they record each request for
//! [`crate::Runtime::take_path_requests`].

use std::ffi::OsStr;
#[cfg(any(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))), test))]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Why a local path was refused or couldn't be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenPathError {
    /// Nothing to open: the path was empty.
    Empty,
    /// Not a valid path (for example, contains an embedded NUL byte).
    Invalid,
    /// This platform cannot open or reveal local paths (for example, `wasm32` or mobile).
    Unsupported,
    /// The system couldn't open or reveal the path.
    Failed(String),
}

impl std::fmt::Display for OpenPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("empty path"),
            Self::Invalid => f.write_str("not a valid path"),
            Self::Unsupported => f.write_str("opening local paths is not supported on this platform"),
            Self::Failed(why) => write!(f, "couldn't open the path: {why}"),
        }
    }
}
impl std::error::Error for OpenPathError {}

/// What a handler asked the system to do with a local path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathAction {
    /// Open the file or folder in its default application ([`crate::Tx::open_path`]).
    Open,
    /// Show the item selected in the system file manager ([`crate::Tx::reveal_path`]).
    Reveal,
}

impl PathAction {
    pub(crate) fn verb(self) -> &'static str {
        match self {
            Self::Open => "open_path",
            Self::Reveal => "reveal_path",
        }
    }
}

/// A local path request queued by [`crate::Tx::open_path`] or [`crate::Tx::reveal_path`],
/// captured by headless runs and tests via [`crate::Runtime::take_path_requests`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRequest {
    /// The path passed to `open_path` or `reveal_path`.
    pub path: PathBuf,
    /// Whether to open the path or reveal it in the file manager.
    pub action: PathAction,
}

const SUPPORTED_PLATFORM: bool = cfg!(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(any(target_os = "ios", target_os = "android"))),
));

impl crate::Tx {
    /// Open a local file or folder in the system's default application once this handler's frame
    /// is done.
    ///
    /// Unlike [`crate::Tx::open_url`], which accepts only `http:`, `https:` and `mailto:` links, this
    /// handles local filesystem paths on desktop platforms (macOS, Windows, Linux). Only syntax is
    /// checked here (`Empty`, embedded NUL -> `Invalid`, web/mobile -> `Unsupported`); the
    /// filesystem is never touched on the UI thread. Headless runs and tests open nothing; read
    /// the requests with [`crate::Runtime::take_path_requests`].
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use rune::{el, Tx};
    ///
    /// let _ = el().on_press_tx(|_: &mut (), tx: &mut Tx| {
    ///     let _ = tx.open_path(Path::new("/tmp/notes.txt"));
    /// });
    /// ```
    pub fn open_path(&mut self, path: impl AsRef<Path>) -> Result<(), OpenPathError> {
        let path = check(path.as_ref())?;
        self.open_paths.push(PathRequest { path, action: PathAction::Open });
        Ok(())
    }

    /// Reveal a local file or folder selected in the system file manager (Finder on macOS,
    /// Explorer on Windows, the session file manager or OpenURI portal on Linux) once this
    /// handler's frame is done.
    ///
    /// Only syntax is checked here (`Empty`, embedded NUL -> `Invalid`, web/mobile ->
    /// `Unsupported`); the filesystem is never touched on the UI thread. Headless runs and tests
    /// launch nothing; read the requests with [`crate::Runtime::take_path_requests`].
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use rune::{el, Tx};
    ///
    /// let _ = el().on_press_tx(|_: &mut (), tx: &mut Tx| {
    ///     let _ = tx.reveal_path(Path::new("/tmp/notes.txt"));
    /// });
    /// ```
    pub fn reveal_path(&mut self, path: impl AsRef<Path>) -> Result<(), OpenPathError> {
        let path = check(path.as_ref())?;
        self.open_paths.push(PathRequest { path, action: PathAction::Reveal });
        Ok(())
    }
}

/// Validates `path` syntactically without performing any filesystem I/O.
pub(crate) fn check(path: &Path) -> Result<PathBuf, OpenPathError> {
    check_for_platform(path, SUPPORTED_PLATFORM)
}

fn check_for_platform(path: &Path, supported: bool) -> Result<PathBuf, OpenPathError> {
    if !supported {
        return Err(OpenPathError::Unsupported);
    }
    let os = path.as_os_str();
    if os.is_empty() {
        return Err(OpenPathError::Empty);
    }
    if has_embedded_nul(os) {
        return Err(OpenPathError::Invalid);
    }
    Ok(path.to_path_buf())
}

fn has_embedded_nul(os: &OsStr) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        os.as_bytes().contains(&0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        os.encode_wide().any(|u| u == 0)
    }
    #[cfg(not(any(unix, windows)))]
    {
        os.to_string_lossy().contains('\0')
    }
}

impl crate::runtime::HostState {
    pub(crate) fn open_paths(&mut self, requests: Vec<PathRequest>) {
        self.dispatch_paths(requests, dispatch_native);
    }

    pub(crate) fn dispatch_paths(
        &mut self,
        requests: Vec<PathRequest>,
        mut launch: impl FnMut(PathRequest) -> Result<(), OpenPathError>,
    ) {
        for request in requests {
            if self.open_urls_natively {
                let verb = request.action.verb();
                let display = request.path.display().to_string();
                if let Err(e) = launch(request) {
                    eprintln!("rune: {verb}({display}): {e}");
                }
            } else {
                self.push_path_request(request);
            }
        }
    }
}

impl crate::runtime::RuntimeCore {
    #[inline]
    pub(crate) fn open_paths(&mut self, requests: Vec<PathRequest>) {
        self.host.open_paths(requests);
    }
}

/// Encodes an absolute Unix path's raw bytes as an RFC 8089 `file://` URI for
/// `org.freedesktop.FileManager1.ShowItems`. Returns `None` if `bytes` is empty, does not
/// start with `/`, or contains an interior NUL byte.
#[cfg(any(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")), feature = "dbus"), test))]
fn file_uri_from_bytes(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(b"/") || bytes.contains(&0) {
        return None;
    }
    let mut out = String::with_capacity("file://".len() + bytes.len() + 16);
    out.push_str("file://");
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            write!(out, "%{b:02X}").ok();
        }
    }
    Some(out)
}

/// The directory to open with `xdg-open` when `reveal_path` falls back without `FileManager1`
/// or the `OpenURI` portal.
#[cfg(any(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))), test))]
fn reveal_fallback_target(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) => PathBuf::from("."),
        None => path.to_path_buf(),
    }
}

/// Guards a path passed to `xdg-open` so it never starts with `-` (which `xdg-open` parses as
/// an option flag and cannot escape with `--`).
#[cfg(any(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))), test))]
fn xdg_open_arg(path: &Path) -> OsString {
    #[cfg(unix)]
    let starts_with_dash = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().starts_with(b"-")
    };
    #[cfg(not(unix))]
    let starts_with_dash = path.to_string_lossy().starts_with('-');
    if starts_with_dash {
        let mut out = OsString::from("./");
        out.push(path.as_os_str());
        out
    } else {
        path.as_os_str().to_os_string()
    }
}

/// Converts UTF-16 path units for Windows Shell APIs (`ShellExecuteW`, `ILCreateFromPathW`):
/// strips `\\?\` from drive paths, turns `\\?\UNC\` into `\\`, normalizes `/` to `\`,
/// and appends a terminating NUL.
#[cfg(any(target_os = "windows", test))]
fn windows_shell_wide_from_units(units: &[u16]) -> Vec<u16> {
    const SLASH: u16 = b'/' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    const QUESTION: u16 = b'?' as u16;
    const COLON: u16 = b':' as u16;

    let norm = |u: u16| if u == SLASH { BACKSLASH } else { u };
    let eq_ascii_ci = |u: u16, ch: u8| u < 0x80 && (u as u8).eq_ignore_ascii_case(&ch);

    let has_verbatim = units.len() >= 4
        && units[0] == BACKSLASH
        && units[1] == BACKSLASH
        && units[2] == QUESTION
        && units[3] == BACKSLASH;

    let mut out = Vec::with_capacity(units.len() + 1);
    if has_verbatim
        && units.len() >= 8
        && eq_ascii_ci(units[4], b'U')
        && eq_ascii_ci(units[5], b'N')
        && eq_ascii_ci(units[6], b'C')
        && norm(units[7]) == BACKSLASH
    {
        out.push(BACKSLASH);
        out.push(BACKSLASH);
        out.extend(units[8..].iter().map(|&u| norm(u)));
    } else if has_verbatim
        && units.len() >= 6
        && units[4] < 0x80
        && (units[4] as u8).is_ascii_alphabetic()
        && units[5] == COLON
    {
        out.extend(units[4..].iter().map(|&u| norm(u)));
    } else {
        out.extend(units.iter().map(|&u| norm(u)));
    }
    out.push(0);
    out
}

/// Resolves relative paths on a background worker thread without touching the UI thread.
#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(any(target_os = "ios", target_os = "android"))),
))]
fn resolve_path(path: &Path) -> PathBuf {
    if path.is_relative() {
        std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn dispatch_native(request: PathRequest) -> Result<(), OpenPathError> {
    use std::sync::{OnceLock, mpsc};
    static WORKER: OnceLock<Result<mpsc::Sender<PathRequest>, String>> = OnceLock::new();
    let tx = WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<PathRequest>();
            std::thread::Builder::new()
                .name("rune-open-path".into())
                .spawn(move || {
                    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
                    // SAFETY: STA COM initialization for this dedicated worker thread; Shell APIs
                    // (`SHOpenFolderAndSelectItems`, `ShellExecuteW`) expect an STA apartment.
                    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok().ok();
                    while let Ok(req) = rx.recv() {
                        let verb = req.action.verb();
                        let display = req.path.display().to_string();
                        if let Err(e) = run_windows(&req) {
                            eprintln!("rune: {verb}({display}): {e}");
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(tx)
        })
        .as_ref()
        .map_err(|e| OpenPathError::Failed(e.clone()))?;
    tx.send(request).map_err(|_| OpenPathError::Failed("open_path worker stopped".into()))
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn run_windows(request: &PathRequest) -> Result<(), OpenPathError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};
    use windows::core::PCWSTR;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let resolved = resolve_path(&request.path);
    let units: Vec<u16> = resolved.as_os_str().encode_wide().collect();
    let wide = windows_shell_wide_from_units(&units);
    match request.action {
        PathAction::Open => {
            // SAFETY: `wide` is NUL-terminated and outlives the call; NULL verb uses the file's
            // default Shell verb (falling back to "open" for folders and standard associations).
            let result = unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    wide.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                )
            };
            if result as usize > 32 {
                Ok(())
            } else {
                Err(OpenPathError::Failed(format!("ShellExecuteW returned {}", result as usize)))
            }
        }
        PathAction::Reveal => {
            // SAFETY: `wide` is NUL-terminated and outlives `ILCreateFromPathW`; non-null PIDL is
            // freed with `ILFree` right after `SHOpenFolderAndSelectItems`. Passing `None` with a
            // full item PIDL opens its parent folder and selects the item.
            let pidl = unsafe { ILCreateFromPathW(PCWSTR(wide.as_ptr())) };
            if pidl.is_null() {
                return Err(OpenPathError::Failed("path could not be resolved by the shell".into()));
            }
            // SAFETY: `pidl` is non-null from `ILCreateFromPathW` and remains valid until `ILFree` below.
            let hr = unsafe { SHOpenFolderAndSelectItems(pidl, None, 0) };
            // SAFETY: `pidl` was allocated by `ILCreateFromPathW` above and is freed once here.
            unsafe { ILFree(Some(pidl)) };
            hr.map_err(|e| OpenPathError::Failed(format!("SHOpenFolderAndSelectItems failed: {e}")))
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn dispatch_native(request: PathRequest) -> Result<(), OpenPathError> {
    use std::sync::{OnceLock, mpsc};
    static WORKER: OnceLock<Result<mpsc::Sender<PathRequest>, String>> = OnceLock::new();
    let tx = WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<PathRequest>();
            std::thread::Builder::new()
                .name("rune-open-path".into())
                .spawn(move || {
                    while let Ok(req) = rx.recv() {
                        let verb = req.action.verb();
                        let display = req.path.display().to_string();
                        if let Err(e) = run_macos(req) {
                            eprintln!("rune: {verb}({display}): {e}");
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(tx)
        })
        .as_ref()
        .map_err(|e| OpenPathError::Failed(e.clone()))?;
    tx.send(request).map_err(|_| OpenPathError::Failed("open_path worker stopped".into()))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn build_macos_file_url(path: &str, is_dir: bool) -> objc2::rc::Retained<objc2_foundation::NSURL> {
    use objc2_foundation::{NSString, NSURL};
    // `fileURLWithPath:isDirectory:` skips the synchronous `stat()` that `fileURLWithPath:` does.
    NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path), is_dir)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn run_macos(request: PathRequest) -> Result<(), OpenPathError> {
    use objc2::{class, msg_send, rc::{Retained, autoreleasepool}, runtime::{AnyClass, AnyObject, Bool}};
    use objc2_foundation::{NSArray, NSError};

    // Resolve and stat off the main thread so the UI thread never performs filesystem I/O.
    let resolved = resolve_path(&request.path);
    let meta = match request.action {
        PathAction::Open => std::fs::metadata(&resolved),
        PathAction::Reveal => std::fs::symlink_metadata(&resolved),
    }
    .map_err(|e| OpenPathError::Failed(e.to_string()))?;
    let is_dir = meta.is_dir();
    let path_str = resolved
        .to_str()
        .ok_or_else(|| OpenPathError::Failed("path is not valid UTF-8".into()))?
        .to_owned();
    let action = request.action;

    // Hop to the main queue before calling AppKit's `NSWorkspace` (`activateFileViewerSelectingURLs:`
    // and `openURL:configuration:completionHandler:` require the main thread on recent macOS).
    autoreleasepool(|_| {
        // SAFETY: dispatches an Objective-C block onto NSOperationQueue.mainQueue so NSWorkspace/NSURL calls run on the main thread.
        unsafe {
        let block = block2::RcBlock::new(move || {
            autoreleasepool(|_| {
                let url = build_macos_file_url(&path_str, is_dir);
                let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
                if workspace.is_null() {
                    eprintln!("rune: {}({path_str}): NSWorkspace unavailable", action.verb());
                    return;
                }
                match action {
                    PathAction::Open => {
                        if let Some(cfg_cls) = AnyClass::get(c"NSWorkspaceOpenConfiguration") {
                            let config: *mut AnyObject = msg_send![cfg_cls, configuration];
                            if !config.is_null() {
                                let label_path = path_str.clone();
                                let done = block2::RcBlock::new(move |_app: *mut AnyObject, error: *mut NSError| {
                                    if let Some(err) = error.as_ref() {
                                        eprintln!("rune: open_path({label_path}): {}", err.localizedDescription());
                                    }
                                });
                                let _: () = msg_send![
                                    workspace,
                                    openURL: &*url,
                                    configuration: config,
                                    completionHandler: &*done
                                ];
                                return;
                            }
                        }
                        let opened: Bool = msg_send![workspace, openURL: &*url];
                        if !opened.as_bool() {
                            eprintln!("rune: open_path({path_str}): no app opened it");
                        }
                    }
                    PathAction::Reveal => {
                        let urls = NSArray::from_retained_slice(&[url]);
                        let _: () = msg_send![workspace, activateFileViewerSelectingURLs: &*urls];
                    }
                }
            });
        });
        let main: Retained<AnyObject> = msg_send![class!(NSOperationQueue), mainQueue];
        let _: () = msg_send![&main, addOperationWithBlock: &*block];
        }
    });
    Ok(())
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
fn dispatch_native(request: PathRequest) -> Result<(), OpenPathError> {
    crate::platform::portal::spawn(move || run_linux_portal(request))
        .map_err(OpenPathError::Failed)
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
async fn run_linux_portal(request: PathRequest) {
    use std::os::unix::ffi::OsStrExt;
    use std::time::Duration;
    use zbus::Connection;

    const RPC_TIMEOUT: Duration = Duration::from_secs(30);
    let verb = request.action.verb();
    let resolved = resolve_path(&request.path);
    let display = resolved.display().to_string();

    let conn = crate::platform::portal::timeout(RPC_TIMEOUT, Connection::session())
        .await
        .and_then(Result::ok);

    match request.action {
        PathAction::Open => {
            if let Some(conn) = conn {
                crate::platform::portal::register_app_id(&conn).await;
                match open_uri_portal(&conn, &resolved, "OpenFile", RPC_TIMEOUT).await {
                    PortalStep::Done => { conn.close().await.ok(); return; }
                    PortalStep::Failed(msg) => {
                        conn.close().await.ok();
                        eprintln!("rune: {verb}({display}): {msg}");
                        return;
                    }
                    PortalStep::Fallback => { conn.close().await.ok(); }
                }
            }
            if let Err(e) = spawn_xdg_open(&resolved) {
                eprintln!("rune: {verb}({display}): {e}");
            }
        }
        PathAction::Reveal => {
            if let Some(conn) = conn {
                if let Some(uri) = file_uri_from_bytes(resolved.as_os_str().as_bytes()) {
                    let uris = [uri.as_str()];
                    let showed = crate::platform::portal::timeout(
                        RPC_TIMEOUT,
                        conn.call_method(
                            Some("org.freedesktop.FileManager1"),
                            "/org/freedesktop/FileManager1",
                            Some("org.freedesktop.FileManager1"),
                            "ShowItems",
                            &(&uris[..], ""),
                        ),
                    )
                    .await;
                    if matches!(showed, Some(Ok(_))) {
                        conn.close().await.ok();
                        return;
                    }
                }
                crate::platform::portal::register_app_id(&conn).await;
                match open_uri_portal(&conn, &resolved, "OpenDirectory", RPC_TIMEOUT).await {
                    PortalStep::Done => { conn.close().await.ok(); return; }
                    PortalStep::Failed(msg) => {
                        conn.close().await.ok();
                        eprintln!("rune: {verb}({display}): {msg}");
                        return;
                    }
                    PortalStep::Fallback => { conn.close().await.ok(); }
                }
            }
            let target = reveal_fallback_target(&resolved);
            if let Err(e) = spawn_xdg_open(&target) {
                eprintln!("rune: {verb}({display}): {e}");
            }
        }
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
enum PortalStep {
    Done,
    Fallback,
    Failed(String),
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
async fn open_uri_portal(
    conn: &zbus::Connection,
    path: &Path,
    method: &str,
    rpc_timeout: std::time::Duration,
) -> PortalStep {
    use std::collections::HashMap;
    use zbus::{Proxy, zvariant::{Fd, Value}};
    use crate::platform::portal::{self, DEST, PATH, RequestError, RequestTimeouts};

    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PortalStep::Failed(e.to_string()),
        Err(_) => return PortalStep::Fallback,
    };
    let proxy = match portal::timeout(
        rpc_timeout,
        Proxy::new(conn, DEST, PATH, "org.freedesktop.portal.OpenURI"),
    )
    .await
    {
        Some(Ok(proxy)) => proxy,
        Some(Err(e)) if portal::is_unavailable(&e) => return PortalStep::Fallback,
        Some(Err(e)) => return PortalStep::Failed(e.to_string()),
        None => return PortalStep::Fallback,
    };
    let token = portal::next_token("open_path");
    let options = HashMap::from([("handle_token", Value::from(token.as_str()))]);
    let fd = Fd::from(&file);
    let body = ("", fd, options);
    match portal::request(
        conn,
        &proxy,
        &token,
        method,
        &body,
        RequestTimeouts { rpc: rpc_timeout, response: None },
        std::future::pending::<()>(),
    )
    .await
    {
        // Status 0 = opened, 1 = user cancelled app chooser, 2 = ended otherwise.
        Ok((0 | 1 | 2, _)) => PortalStep::Done,
        Ok((status, _)) => PortalStep::Failed(format!("OpenURI portal response {status}")),
        Err(RequestError::Unavailable) => PortalStep::Fallback,
        Err(RequestError::Dbus(e)) if portal::is_unavailable(&e) => PortalStep::Fallback,
        Err(RequestError::Dbus(e)) => PortalStep::Failed(e.to_string()),
        Err(RequestError::TimedOut) => PortalStep::Failed("OpenURI portal timed out".into()),
        Err(RequestError::Disconnected) => PortalStep::Failed("OpenURI portal disconnected".into()),
        Err(RequestError::Cancelled) => PortalStep::Done,
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android")),
    not(all(target_os = "linux", feature = "dbus")),
))]
fn dispatch_native(request: PathRequest) -> Result<(), OpenPathError> {
    use std::sync::{OnceLock, mpsc};
    static WORKER: OnceLock<Result<mpsc::Sender<PathRequest>, String>> = OnceLock::new();
    let tx = WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<PathRequest>();
            std::thread::Builder::new()
                .name("rune-open-path".into())
                .spawn(move || {
                    while let Ok(req) = rx.recv() {
                        let verb = req.action.verb();
                        let resolved = resolve_path(&req.path);
                        let target = match req.action {
                            PathAction::Open => resolved.clone(),
                            PathAction::Reveal => reveal_fallback_target(&resolved),
                        };
                        if let Err(e) = spawn_xdg_open(&target) {
                            eprintln!("rune: {verb}({}): {e}", resolved.display());
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(tx)
        })
        .as_ref()
        .map_err(|e| OpenPathError::Failed(e.clone()))?;
    tx.send(request).map_err(|_| OpenPathError::Failed("open_path worker stopped".into()))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn spawn_xdg_open(path: &Path) -> Result<(), OpenPathError> {
    let arg = xdg_open_arg(path);
    let mut child = std::process::Command::new("xdg-open")
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| OpenPathError::Failed(e.to_string()))?;
    // Reap the child so it does not linger as a zombie.
    std::thread::spawn(move || { child.wait().ok(); });
    Ok(())
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(any(target_os = "ios", target_os = "android"))),
)))]
fn dispatch_native(_: PathRequest) -> Result<(), OpenPathError> {
    Err(OpenPathError::Unsupported)
}

#[cfg(test)]
#[path = "open_path_tests.rs"]
mod tests;
