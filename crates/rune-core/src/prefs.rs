//! A small store for an app's settings (text size, last-used options): JSON in the platform's
//! config directory, read and written with serde.
//!
//! ```no_run
//! let mut prefs = rune_core::prefs::Prefs::open("com.example.notes");
//! let size: f32 = prefs.get("text_size").unwrap_or(14.0);
//! prefs.set("text_size", &(size + 1.0)).ok();
//! ```
//!
//! The file is `prefs.json` in `~/Library/Application Support/<app id>` on macOS and iOS,
//! `%APPDATA%\<app id>` on Windows and `$XDG_CONFIG_HOME/<app id>` (else `~/.config/<app id>`)
//! elsewhere: [`crate::dirs::AppDirs::config`]. `RUNE_PREFS_DIR` replaces the config directory,
//! and `RUNE_DIRS_ROOT` puts it at `<root>/config`, so a headless run or a test script keeps its
//! settings apart from the real ones. [`Prefs::memory`] keeps nothing on disk.
//!
//! Every `set` and `remove` rewrites the file whole through a temporary file and a rename, so a
//! crash leaves the old settings or the new, never half. Nothing runs in the background and
//! nothing watches the file: a second process's changes show up at the next `open`. A file that
//! doesn't parse is moved to `prefs.json.bad` and the store starts empty. On the web, and where
//! no config directory can be found, the store is in memory and `is_persistent` says so.
use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

const FILE: &str = "prefs.json";
#[cfg(not(target_arch = "wasm32"))]
static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

/// Why a change couldn't be saved. The value is still set in memory.
#[derive(Debug)]
pub enum PrefsError {
    /// The value can't be written as JSON.
    Serialize(String),
    /// The file couldn't be written.
    Io(io::Error),
}

impl fmt::Display for PrefsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(why) => write!(f, "can't store the value: {why}"),
            Self::Io(e) => write!(f, "can't save preferences: {e}"),
        }
    }
}
impl std::error::Error for PrefsError {}

/// An app's settings, keyed by name. See the [module docs](self).
#[derive(Debug)]
pub struct Prefs {
    values: BTreeMap<String, Value>,
    path: Option<PathBuf>,
}

impl Prefs {
    /// The settings of the app with this id (a reverse-DNS name is conventional), loaded from
    /// its config directory. Slashes and other characters a folder name can't hold become `_`.
    pub fn open(app_id: &str) -> Prefs {
        match config_dir(|k| std::env::var_os(k)) {
            Some(dir) => Prefs::at(dir.join(sanitize(app_id)).join(FILE)),
            None => Prefs::memory(),
        }
    }

    /// Settings kept in this file, which need not exist yet; its folder is made on the first save.
    pub fn at(path: impl Into<PathBuf>) -> Prefs {
        let path = path.into();
        let values = load(&path);
        Prefs { values, path: Some(path) }
    }

    /// Settings that live only as long as this value, for tests and headless runs.
    pub fn memory() -> Prefs { Prefs { values: BTreeMap::new(), path: None } }

    /// The file this store saves to, if it has one.
    pub fn path(&self) -> Option<&Path> { self.path.as_deref() }

    /// Whether changes reach a file.
    pub fn is_persistent(&self) -> bool { self.path.is_some() }

    /// The value under `key`, or `None` if it is unset or isn't a `T` (a setting written by
    /// another version of the app).
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_value(self.values.get(key)?.clone()).ok()
    }

    /// Sets `key` and saves. On an error the value is still set for this run.
    pub fn set<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) -> Result<(), PrefsError> {
        let value = serde_json::to_value(value).map_err(|e| PrefsError::Serialize(e.to_string()))?;
        self.values.insert(key.to_owned(), value);
        self.save()
    }

    /// Unsets `key` and saves.
    pub fn remove(&mut self, key: &str) -> Result<(), PrefsError> {
        if self.values.remove(key).is_none() { return Ok(()); }
        self.save()
    }

    /// Whether `key` has a value of any type.
    pub fn contains(&self, key: &str) -> bool { self.values.contains_key(key) }

    fn save(&self) -> Result<(), PrefsError> {
        let Some(path) = &self.path else { return Ok(()) };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(PrefsError::Io(io::Error::new(io::ErrorKind::Unsupported, "prefs files are unsupported on web")))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let json = serde_json::to_vec_pretty(&self.values).map_err(|e| PrefsError::Serialize(e.to_string()))?;
            write_atomic(path, &json).map_err(PrefsError::Io)
        }
    }
}

fn load(path: &Path) -> BTreeMap<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return BTreeMap::new(),
    };
    if let Ok(values) = serde_json::from_slice(&bytes) { return values; }
    let mut bad = path.as_os_str().to_owned();
    bad.push(".bad");
    if std::fs::rename(path, &bad).is_err() { std::fs::write(&bad, &bytes).ok(); }
    BTreeMap::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use io::Write;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    let seq = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
    tmp.push(format!(".{}.{seq}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_replace(&tmp, path)
    })();
    if result.is_err() { std::fs::remove_file(&tmp).ok(); }
    result
}

#[cfg(windows)]
fn rename_replace(tmp: &Path, path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    // On Windows, renaming over an existing file fails transiently with ERROR_ACCESS_DENIED while
    // a concurrent rename's replaced target is delete-pending or another reader holds the file,
    // and with ERROR_SHARING_VIOLATION when another handle lacks delete sharing. Retry a bounded
    // number of times (7 attempts, <= 31 ms sleep) on those two errors only.
    const DELAYS_MS: [u64; 6] = [0, 1, 2, 4, 8, 16];
    for delay in DELAYS_MS {
        match std::fs::rename(tmp, path) {
            Ok(()) => return Ok(()),
            Err(err)
                if err.kind() == io::ErrorKind::PermissionDenied
                    || err.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) =>
            {
                if delay == 0 {
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                }
            }
            Err(err) => return Err(err),
        }
    }
    std::fs::rename(tmp, path)
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
fn rename_replace(tmp: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(tmp, path)
}

pub(crate) fn sanitize(app_id: &str) -> String {
    let id: String = app_id.trim().chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let trimmed = id.trim_end_matches(|c: char| c.is_whitespace() || c == '.');
    if trimmed.is_empty() { "app".into() } else { trimmed.to_owned() }
}

/// The directory app folders live in, from an environment lookup.
fn config_dir(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("RUNE_PREFS_DIR") { return Some(dir); }
    crate::dirs::config_base(&var)
}

#[cfg(test)]
#[path = "prefs_tests.rs"]
mod tests;
