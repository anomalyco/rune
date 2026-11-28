//! Process-wide bounded cache of memory-mapped system font files (`Blob<u8>`), so each
//! fallback font file is opened and mapped at most once across threads and `TextSystem`s, and
//! prewarmed or asynchronously loaded fallbacks register into a `Collection` with zero UI-thread
//! file I/O.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{HashMap, VecDeque};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(not(target_arch = "wasm32"))]
use parley::fontique::{Blob, SourceCache, SourceId, SourceInfo, SourceKind};

#[cfg(not(target_arch = "wasm32"))]
const MAX_BLOB_ENTRIES: usize = 256;
#[cfg(not(target_arch = "wasm32"))]
const MAX_BLOB_BYTES: usize = 512 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
struct BlobCache {
    map: HashMap<PathBuf, Option<Blob<u8>>>,
    order: VecDeque<PathBuf>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl BlobCache {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    fn entry_bytes(path: &Path, blob: &Option<Blob<u8>>) -> usize {
        path.as_os_str().len() + blob.as_ref().map_or(0, |b| b.as_ref().len())
    }

    fn get(&self, path: &Path) -> Option<Option<Blob<u8>>> {
        self.map.get(path).cloned()
    }

    fn insert(&mut self, path: PathBuf, blob: Option<Blob<u8>>) {
        if self.map.contains_key(&path) {
            return;
        }
        let cost = Self::entry_bytes(&path, &blob);
        if self.max_entries == 0 || cost > self.max_bytes {
            return;
        }
        while self.order.len() >= self.max_entries || self.bytes.saturating_add(cost) > self.max_bytes {
            let Some(old_path) = self.order.pop_front() else {
                break;
            };
            if let Some(old_blob) = self.map.remove(&old_path) {
                self.bytes = self.bytes.saturating_sub(Self::entry_bytes(&old_path, &old_blob));
            }
        }
        self.bytes = self.bytes.saturating_add(cost);
        self.order.push_back(path.clone());
        self.map.insert(path, blob);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn shared_cache() -> &'static Mutex<BlobCache> {
    static CACHE: OnceLock<Mutex<BlobCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BlobCache::new(MAX_BLOB_ENTRIES, MAX_BLOB_BYTES)))
}

/// Memory-maps `path` into a `Blob<u8>` through fontique's `SourceCache` (no heap copy for
/// large CJK TTC files).
#[cfg(not(target_arch = "wasm32"))]
fn map_font_file(path: &Path) -> Option<Blob<u8>> {
    let source = SourceInfo::new(SourceId::new(), SourceKind::Path(Arc::from(path)));
    SourceCache::default().get(&source)
}

/// Returns the memory-mapped `Blob<u8>` for `path`, mapping the file at most once per process
/// while the entry stays resident in the bounded cache.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn load_blob(path: &Path) -> Option<Blob<u8>> {
    if let Some(hit) = try_cached_blob(path) {
        return hit;
    }
    let blob = map_font_file(path);
    shared_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_path_buf(), blob.clone());
    blob
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
pub(super) fn load_blob(_: &std::path::Path) -> Option<parley::fontique::Blob<u8>> {
    None
}

/// Non-blocking cache lookup for `path`: returns `Some(blob_opt)` if resident (zero file I/O),
/// or `None` on a cache miss.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn try_cached_blob(path: &Path) -> Option<Option<Blob<u8>>> {
    shared_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(path)
}

/// Memory-maps every file in `paths` into the process-wide blob cache, returning the paths that
/// mapped cleanly alongside their `Blob<u8>`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn preload_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<(PathBuf, Blob<u8>)> {
    let mut out = Vec::new();
    for path in paths {
        if let Some(blob) = load_blob(&path) {
            out.push((path, blob));
        }
    }
    out
}

/// Returns `Some(entries)` when every path in `paths` is already cached in memory (zero file
/// I/O), or `None` if any path still needs disk/mmap I/O.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn try_cached_paths(paths: &[PathBuf]) -> Option<Vec<(PathBuf, Blob<u8>)>> {
    let cache = shared_cache().lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let entry = cache.get(path)?;
        if let Some(blob) = entry {
            out.push((path.clone(), blob));
        }
    }
    Some(out)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "blobs_tests.rs"]
mod tests;
