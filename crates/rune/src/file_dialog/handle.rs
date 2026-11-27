//! Platform-neutral file handles returned by [`Tx::pick_file`](crate::Tx::pick_file),
//! [`Tx::pick_files`](crate::Tx::pick_files) and [`Tx::pick_save_file`](crate::Tx::pick_save_file).

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Backend for a platform- or host-supplied [`FileHandle`] that has no local filesystem path
/// (for example an Android `content://` document opened through a detached file descriptor or
/// an iOS security-scoped bookmark).
pub trait FileHandleProvider: Send + Sync {
    /// Opens a fresh stream positioned at the start of the file.
    fn reader(&self) -> io::Result<Box<dyn Read + Send>>;

    /// Opens a fresh stream that replaces the file's contents (truncating write, `"wt"`).
    /// Read-only providers inherit the default `Unsupported` error.
    fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "this file handle is read-only"))
    }
}

/// A platform-neutral handle to a file chosen through [`Tx::pick_file`](crate::Tx::pick_file),
/// [`Tx::pick_files`](crate::Tx::pick_files), [`Tx::pick_save_file`](crate::Tx::pick_save_file),
/// or [`FileDialogs::pick_file`](crate::FileDialogs::pick_file) /
/// [`FileDialogs::pick_files`](crate::FileDialogs::pick_files) /
/// [`FileDialogs::pick_save_file`](crate::FileDialogs::pick_save_file).
///
/// Desktop hosts and headless path fixtures return path-backed handles whose [`path`](Self::path)
/// is `Some`. Document providers (Android Storage Access Framework `content://` URIs) and web
/// pickers (`<input type="file">`, File System Access handles) return handles whose
/// [`path`](Self::path) is `None` without copying into app cache.
///
/// Handles are cheaply [`Clone`]able, [`Send`] + [`Sync`], and scoped to the current session/task
/// (not persisted across launches). Run blocking I/O ([`reader`](Self::reader),
/// [`writer`](Self::writer), [`read_bytes`](Self::read_bytes), [`write_bytes`](Self::write_bytes))
/// on a worker thread ([`Latest`](crate::Latest) or [`task::spawn`](crate::task::spawn)), never
/// on the UI thread. Async [`read`](Self::read) and [`write`](Self::write) work on every target,
/// including browsers without JSPI.
#[derive(Clone)]
pub struct FileHandle(Arc<Inner>);

struct Inner {
    name: String,
    backend: Backend,
}

enum Backend {
    Path(PathBuf),
    Memory(Arc<Mutex<Vec<u8>>>),
    Provider(Arc<dyn FileHandleProvider>),
    #[cfg(target_arch = "wasm32")]
    Web { id: u32 },
}

impl Drop for Inner {
    fn drop(&mut self) {
        #[cfg(target_arch = "wasm32")]
        if let Backend::Web { id } = self.backend {
            js::release_picked_file(id);
        }
    }
}

impl FileHandle {
    /// Creates a path-backed handle for a local filesystem path.
    ///
    /// `name()` returns the path's final component (lossy UTF-8), falling back to the path's
    /// display string when it has no file name component.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Self(Arc::new(Inner { name, backend: Backend::Path(path) }))
    }

    /// Creates an in-memory handle with no filesystem path, for tests and headless fixtures.
    /// Clones share the same underlying buffer, so writing through one clone is visible to reads
    /// from another.
    pub fn from_bytes(name: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self(Arc::new(Inner {
            name: name.into(),
            backend: Backend::Memory(Arc::new(Mutex::new(bytes.into()))),
        }))
    }

    /// Creates a handle backed by a custom [`FileHandleProvider`] (for example an Android
    /// `content://` document opened through a detached file descriptor).
    pub fn from_provider(name: impl Into<String>, provider: impl FileHandleProvider + 'static) -> Self {
        Self(Arc::new(Inner {
            name: name.into(),
            backend: Backend::Provider(Arc::new(provider)),
        }))
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_web(id: u32, name: String) -> Self {
        Self(Arc::new(Inner { name, backend: Backend::Web { id } }))
    }

    /// The file's display name (for example `"export.csv"`), without any directory prefix when
    /// known.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The local filesystem path when this handle is path-backed (desktop hosts, headless path
    /// fixtures), or `None` for document-provider, browser, and in-memory handles.
    pub fn path(&self) -> Option<&Path> {
        match &self.0.backend {
            Backend::Path(path) => Some(path.as_path()),
            Backend::Memory(_) | Backend::Provider(_) => None,
            #[cfg(target_arch = "wasm32")]
            Backend::Web { .. } => None,
        }
    }

    /// On web (`wasm32`), returns the underlying browser `File` or `FileSystemFileHandle`
    /// without copying it into Wasm memory, if this handle was created by the web file picker.
    #[cfg(target_arch = "wasm32")]
    pub fn js_file(&self) -> Option<wasm_bindgen::JsValue> {
        match self.0.backend {
            Backend::Web { id } => Some(js::picked_file(id)).filter(|v| !v.is_undefined()),
            _ => None,
        }
    }

    /// Opens a fresh blocking stream for reading from byte 0. Run on a worker thread
    /// ([`Latest`](crate::Latest) or [`task::spawn`](crate::task::spawn)), never on the UI thread.
    ///
    /// On `wasm32`, browser-picked handles require JSPI task work (`rune_core::task::can_wait`)
    /// for blocking I/O; use async [`read`](Self::read) on non-JSPI browsers.
    pub fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        match &self.0.backend {
            Backend::Path(path) => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    Ok(Box::new(std::fs::File::open(path)?))
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = path;
                    Err(io::Error::new(io::ErrorKind::Unsupported, "local filesystem paths are unsupported on web"))
                }
            }
            Backend::Memory(buf) => {
                let bytes = buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
                Ok(Box::new(io::Cursor::new(bytes)))
            }
            Backend::Provider(provider) => provider.reader(),
            #[cfg(target_arch = "wasm32")]
            Backend::Web { id } => Ok(Box::new(io::Cursor::new(web_read_blocking(*id)?))),
        }
    }

    /// Opens a fresh blocking stream that replaces the file's contents (truncating write,
    /// matching `std::fs::File::create`, Android `"wt"`, and
    /// `FileSystemFileHandle.createWritable()`). Run on a worker thread, never on the UI thread.
    ///
    /// On `wasm32`, browser save handles require JSPI task work for blocking writes; use async
    /// [`write`](Self::write) on non-JSPI browsers.
    pub fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        match &self.0.backend {
            Backend::Path(path) => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    Ok(Box::new(std::fs::File::create(path)?))
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = path;
                    Err(io::Error::new(io::ErrorKind::Unsupported, "local filesystem paths are unsupported on web"))
                }
            }
            Backend::Memory(buf) => {
                buf.lock().unwrap_or_else(|e| e.into_inner()).clear();
                Ok(Box::new(MemoryWriter(Arc::clone(buf))))
            }
            Backend::Provider(provider) => provider.writer(),
            #[cfg(target_arch = "wasm32")]
            Backend::Web { id } => {
                if !rune_core::task::can_wait() {
                    return Err(web_non_jspi_error());
                }
                Ok(Box::new(WebWriter { id: *id, buf: Vec::new(), dirty: true }))
            }
        }
    }

    /// Reads the entire file into memory, blocking the calling worker thread.
    ///
    /// On `wasm32`, browser-picked handles require JSPI task work; use async [`read`](Self::read)
    /// when JSPI is unavailable.
    pub fn read_bytes(&self) -> io::Result<Vec<u8>> {
        match &self.0.backend {
            Backend::Path(path) => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    std::fs::read(path)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = path;
                    Err(io::Error::new(io::ErrorKind::Unsupported, "local filesystem paths are unsupported on web"))
                }
            }
            Backend::Memory(buf) => Ok(buf.lock().unwrap_or_else(|e| e.into_inner()).clone()),
            Backend::Provider(provider) => {
                let mut r = provider.reader()?;
                let mut out = Vec::new();
                r.read_to_end(&mut out)?;
                Ok(out)
            }
            #[cfg(target_arch = "wasm32")]
            Backend::Web { id } => web_read_blocking(*id),
        }
    }

    /// Replaces the file's entire contents with `bytes`, blocking the calling worker thread.
    ///
    /// On `wasm32`, browser save handles require JSPI task work; use async [`write`](Self::write)
    /// when JSPI is unavailable.
    pub fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        match &self.0.backend {
            Backend::Path(path) => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    std::fs::write(path, bytes)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = (path, bytes);
                    Err(io::Error::new(io::ErrorKind::Unsupported, "local filesystem paths are unsupported on web"))
                }
            }
            Backend::Memory(buf) => {
                let mut guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                guard.clear();
                guard.extend_from_slice(bytes);
                Ok(())
            }
            Backend::Provider(provider) => {
                let mut w = provider.writer()?;
                w.write_all(bytes)?;
                w.flush()
            }
            #[cfg(target_arch = "wasm32")]
            Backend::Web { id } => web_write_blocking(*id, bytes),
        }
    }

    /// Asynchronously reads the entire file into memory.
    ///
    /// On native targets this runs on Rune's background task pool so it does not block the caller's
    /// thread or UI event loop. On `wasm32` it awaits the browser `File` / `FileSystemFileHandle`
    /// directly and works both with and without JSPI.
    pub async fn read(&self) -> io::Result<Vec<u8>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let handle = self.clone();
            spawn_io(move || handle.read_bytes()).await
        }
        #[cfg(target_arch = "wasm32")]
        {
            match &self.0.backend {
                Backend::Web { id } => web_read_async(*id).await,
                _ => self.read_bytes(),
            }
        }
    }

    /// Asynchronously replaces the file's entire contents with `bytes`.
    ///
    /// On native targets this runs on Rune's background task pool. On `wasm32` it writes to a
    /// `FileSystemFileHandle` when available or triggers a browser download of `bytes` with
    /// [`name`](Self::name), working both with and without JSPI.
    pub async fn write(&self, bytes: &[u8]) -> io::Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let handle = self.clone();
            let owned = bytes.to_vec();
            spawn_io(move || handle.write_bytes(&owned)).await
        }
        #[cfg(target_arch = "wasm32")]
        {
            match &self.0.backend {
                Backend::Web { id } => web_write_async(*id, bytes).await,
                _ => self.write_bytes(bytes),
            }
        }
    }
}

impl fmt::Debug for FileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("FileHandle");
        s.field("name", &self.0.name);
        if let Some(path) = self.path() {
            s.field("path", &path);
        }
        s.finish()
    }
}

const _: fn() = || {
    fn assert_send_sync_clone<T: Send + Sync + Clone>() {}
    assert_send_sync_clone::<FileHandle>();
};

struct MemoryWriter(Arc<Mutex<Vec<u8>>>);

impl Write for MemoryWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_io<T: Send + 'static>(work: impl FnOnce() -> io::Result<T> + Send + 'static) -> Oneshot<io::Result<T>> {
    use std::task::Waker;
    let state: Arc<Mutex<(Option<io::Result<T>>, Option<Waker>)>> = Arc::new(Mutex::new((None, None)));
    let writer = Arc::clone(&state);
    if let Err(err) = rune_core::task::spawn("rune-file-handle", move || {
        let res = work();
        let waker = {
            let mut guard = writer.lock().unwrap_or_else(|e| e.into_inner());
            guard.0 = Some(res);
            guard.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }) {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.0 = Some(Err(err));
    }
    Oneshot(state)
}

#[cfg(not(target_arch = "wasm32"))]
struct Oneshot<T>(Arc<Mutex<(Option<T>, Option<std::task::Waker>)>>);

#[cfg(not(target_arch = "wasm32"))]
impl<T> std::future::Future for Oneshot<T> {
    type Output = T;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.0.take() {
            std::task::Poll::Ready(val)
        } else {
            guard.1 = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn web_non_jspi_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "blocking file handle I/O on web requires JSPI task work; use async FileHandle::read/write instead",
    )
}

#[cfg(target_arch = "wasm32")]
fn js_error_io(err: wasm_bindgen::JsValue) -> io::Error {
    let msg = err
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(&err, &"message".into())
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| format!("{err:?}"));
    io::Error::other(msg)
}

#[cfg(target_arch = "wasm32")]
fn bytes_from_js(val: wasm_bindgen::JsValue) -> io::Result<Vec<u8>> {
    use wasm_bindgen::JsCast;
    val.dyn_into::<js_sys::Uint8Array>()
        .map(|arr| arr.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "browser file handle is no longer available"))
}

#[cfg(target_arch = "wasm32")]
fn web_read_blocking(id: u32) -> io::Result<Vec<u8>> {
    if !rune_core::task::can_wait() {
        return Err(web_non_jspi_error());
    }
    let promise = js::read_picked_file(id);
    match rune_core::task::wait(&promise) {
        Some(Ok(val)) => bytes_from_js(val),
        Some(Err(err)) => Err(js_error_io(err)),
        None => Err(web_non_jspi_error()),
    }
}

#[cfg(target_arch = "wasm32")]
fn web_write_blocking(id: u32, bytes: &[u8]) -> io::Result<()> {
    if !rune_core::task::can_wait() {
        return Err(web_non_jspi_error());
    }
    let arr = js_sys::Uint8Array::from(bytes);
    let promise = js::write_picked_file(id, &arr);
    match rune_core::task::wait(&promise) {
        Some(Ok(_)) => Ok(()),
        Some(Err(err)) => Err(js_error_io(err)),
        None => Err(web_non_jspi_error()),
    }
}

#[cfg(target_arch = "wasm32")]
async fn web_read_async(id: u32) -> io::Result<Vec<u8>> {
    let promise = js::read_picked_file(id);
    let val = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(js_error_io)?;
    bytes_from_js(val)
}

#[cfg(target_arch = "wasm32")]
async fn web_write_async(id: u32, bytes: &[u8]) -> io::Result<()> {
    let arr = js_sys::Uint8Array::from(bytes);
    let promise = js::write_picked_file(id, &arr);
    wasm_bindgen_futures::JsFuture::from(promise).await.map_err(js_error_io)?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
struct WebWriter {
    id: u32,
    buf: Vec<u8>,
    dirty: bool,
}

#[cfg(target_arch = "wasm32")]
impl Write for WebWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        self.dirty = true;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.dirty {
            web_write_blocking(self.id, &self.buf)?;
            self.dirty = false;
        }
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for WebWriter {
    fn drop(&mut self) {
        if self.dirty {
            web_write_blocking(self.id, &self.buf).ok();
            self.dirty = false;
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/src/platform/web/bridge.js")]
    extern "C" {
        #[wasm_bindgen(js_name = claimPickedFile)]
        pub(crate) fn claim_picked_file(id: u32) -> bool;
        #[wasm_bindgen(js_name = pickedFile)]
        pub(super) fn picked_file(id: u32) -> JsValue;
        #[wasm_bindgen(js_name = releasePickedFile)]
        pub(super) fn release_picked_file(id: u32);
        #[wasm_bindgen(js_name = readPickedFile)]
        pub(super) fn read_picked_file(id: u32) -> js_sys::Promise;
        #[wasm_bindgen(js_name = writePickedFile)]
        pub(super) fn write_picked_file(id: u32, bytes: &js_sys::Uint8Array) -> js_sys::Promise;
    }
}
