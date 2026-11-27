//! Cross-platform, race-free file scanning and change detection for `rune dev`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const SKIP_DIRS: &[&str] = &["target", ".git", "__pycache__", "node_modules"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
}

impl FileStamp {
    fn from_path(path: &Path) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
        Ok(Self::from_meta(&meta))
    }

    fn from_meta(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                len: meta.len(),
                modified: meta.modified().ok(),
                dev: meta.dev(),
                ino: meta.ino(),
                ctime: meta.ctime(),
                ctime_nsec: meta.ctime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: meta.len(),
                modified: meta.modified().ok(),
            }
        }
    }
}

/// A deterministic snapshot mapping normalized POSIX keys to file bytes.
pub type Snapshot = BTreeMap<String, Vec<u8>>;

/// Caches file metadata stamps + contents so unchanged files are not re-read on every poll.
#[derive(Debug, Default)]
pub struct FileCache {
    entries: BTreeMap<String, (FileStamp, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }

    pub fn all_changed(&self) -> Vec<String> {
        let mut all = BTreeSet::new();
        for k in self
            .added
            .iter()
            .chain(self.removed.iter())
            .chain(self.modified.iter())
        {
            all.insert(k.clone());
        }
        all.into_iter().collect()
    }
}

/// Computes the exact set of added, removed, and modified keys between two snapshots.
pub fn diff_snapshots(before: &Snapshot, after: &Snapshot) -> SnapshotDiff {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();

    for (k, after_bytes) in after {
        match before.get(k) {
            None => added.push(k.clone()),
            Some(before_bytes) if before_bytes != after_bytes => modified.push(k.clone()),
            Some(_) => {}
        }
    }
    for k in before.keys() {
        if !after.contains_key(k) {
            removed.push(k.clone());
        }
    }
    SnapshotDiff {
        added,
        removed,
        modified,
    }
}

/// Returns `true` if a file path should be ignored for security or build-noise reasons.
pub fn is_ignored_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.starts_with(".env") {
        return true;
    }
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("pem" | "key")
    )
}

/// Walks `root` recursively, collecting relative POSIX paths for every non-ignored file while
/// skipping `target`, `.git`, `__pycache__`, and `node_modules`.
pub fn walk_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk_recursive(root, root, &mut out);
    out.sort();
    out
}

fn walk_recursive(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if SKIP_DIRS.iter().any(|skip| *skip == name_str) {
                continue;
            }
            walk_recursive(root, &path, out);
        } else if file_type.is_file()
            && !is_ignored_file(&path)
            && let Ok(rel) = path.strip_prefix(root)
        {
            let posix = crate::glob::normalize_rel(&rel.to_string_lossy());
            if !posix.is_empty() {
                out.push(posix);
            }
        }
    }
}

/// Scans `sources` (`(key, host_path)` pairs), reading bytes only when metadata changed.
/// If any file changes while being read, returns `Err` without mutating `cache`.
pub fn scan_sources(
    cache: &mut FileCache,
    sources: &[(String, PathBuf)],
) -> io::Result<Snapshot> {
    let mut next_entries: BTreeMap<String, (FileStamp, Vec<u8>)> = BTreeMap::new();

    for (key, path) in sources {
        if is_ignored_file(path) {
            continue;
        }
        let before = match FileStamp::from_path(path) {
            Ok(s) => s,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };

        if let Some((cached_stamp, cached_bytes)) = cache.entries.get(key)
            && *cached_stamp == before
        {
            next_entries.insert(key.clone(), (before, cached_bytes.clone()));
            continue;
        }

        let data = fs::read(path)?;
        let after = FileStamp::from_path(path)?;
        if after != before {
            return Err(io::Error::other(format!(
                "input changed while reading: {key}"
            )));
        }
        next_entries.insert(key.clone(), (before, data));
    }

    let snapshot = next_entries
        .iter()
        .map(|(k, (_, bytes))| (k.clone(), bytes.clone()))
        .collect();
    cache.entries = next_entries;
    Ok(snapshot)
}
