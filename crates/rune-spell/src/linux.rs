//! Linux spell checker using installed system Hunspell dictionaries via `spellbook`.
//!
//! Dictionary discovery follows the Hunspell / freedesktop Linux lookup order (`DICPATH`,
//! `$XDG_DATA_HOME/hunspell` or `~/.local/share/hunspell`, `$XDG_DATA_DIRS/*/hunspell`,
//! `/usr/local/share/hunspell`, `/usr/share/hunspell`, `/usr/share/myspell/dicts`,
//! `/usr/share/myspell`), and preferred languages follow POSIX locale precedence
//! (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`).
//!
//! `OFF_THREAD = true` ensures dictionary scanning, parsing and word checking run on the
//! `rune-spell` background worker thread rather than doing file I/O on the UI thread.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use super::{hunspell, tags_match};

/// Dictionary discovery, parsing and checking run on the `rune-spell` worker thread so the UI
/// thread never touches the filesystem during frames.
#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) const OFF_THREAD: bool = true;

/// Upper bound on discovered dictionary entries cached from search directories.
const MAX_DISCOVERED_DICTS: usize = 256;
/// Maximum byte size accepted for a single `.aff` or `.dic` file before reading it into memory.
const MAX_DICT_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Maximum number of active dictionaries kept loaded simultaneously.
const MAX_LOADED_DICTS: usize = 8;
/// Maximum combined source bytes (`.aff` + `.dic`) across simultaneously loaded dictionaries.
const MAX_LOADED_BYTES: usize = 64 * 1024 * 1024;
/// Maximum number of in-memory learned words retained for the session.
const MAX_LEARNED_WORDS: usize = 4096;
/// Maximum byte length of a single learned word.
const MAX_LEARNED_WORD_BYTES: usize = 256;

/// System Hunspell dictionary directories checked after user overrides.
const SYSTEM_DIRS: &[&str] = &[
    "/usr/local/share/hunspell",
    "/usr/share/hunspell",
    "/usr/share/myspell/dicts",
    "/usr/share/myspell",
];

/// A discovered Hunspell dictionary pair on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DictEntry {
    /// Normalized BCP 47 language tag (e.g. `en-US`, `de-DE`, `fr`).
    pub(crate) tag: String,
    pub(crate) aff: PathBuf,
    pub(crate) dic: PathBuf,
}

struct LoadedDict {
    tag: String,
    bytes: usize,
    dict: Arc<RwLock<spellbook::Dictionary>>,
}

struct LoadedState {
    generation: u64,
    dicts: Vec<LoadedDict>,
    learned: Vec<String>,
}

impl Default for LoadedState {
    fn default() -> Self {
        Self { generation: u64::MAX, dicts: Vec::new(), learned: Vec::new() }
    }
}

/// Process-shared Linux Hunspell backend state.
///
/// Kept as a struct so unit tests can instantiate isolated backends against synthetic temporary
/// dictionary directories without touching global environment variables or host system folders.
pub(crate) struct LinuxBackend {
    dirs: Box<dyn Fn() -> Vec<PathBuf> + Send + Sync>,
    preferred: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    catalog: OnceLock<Vec<DictEntry>>,
    state: Mutex<LoadedState>,
}

impl LinuxBackend {
    #[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
    fn system() -> Self {
        Self {
            dirs: Box::new(|| search_dirs_from(rune_core::env::var_os)),
            preferred: Box::new(|| preferred_from(rune_core::env::var)),
            catalog: OnceLock::new(),
            state: Mutex::new(LoadedState::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_dirs(dirs: Vec<PathBuf>, preferred: Vec<String>) -> Self {
        Self {
            dirs: Box::new(move || dirs.clone()),
            preferred: Box::new(move || preferred.clone()),
            catalog: OnceLock::new(),
            state: Mutex::new(LoadedState::default()),
        }
    }

    pub(crate) fn catalog(&self) -> &[DictEntry] {
        self.catalog.get_or_init(|| discover_in(&(self.dirs)()))
    }

    pub(crate) fn available(&self) -> Vec<String> {
        self.catalog().iter().map(|entry| entry.tag.clone()).collect()
    }

    pub(crate) fn preferred(&self) -> Vec<String> {
        (self.preferred)()
    }

    #[cfg(test)]
    pub(crate) fn loaded_tags(&self) -> Vec<String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).dicts.iter().map(|d| d.tag.clone()).collect()
    }

    /// Returns the loaded dictionaries for `generation` and `active`, reading and parsing any
    /// newly active `.aff`/`.dic` pairs outside the state lock and dropping deselected ones.
    fn ensure_loaded(&self, generation: u64, active: &[String], current_generation: impl Fn() -> u64) -> Vec<Arc<RwLock<spellbook::Dictionary>>> {
        let (existing, learned) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.generation == generation {
                return state.dicts.iter().map(|d| Arc::clone(&d.dict)).collect();
            }
            let existing: Vec<(String, usize, Arc<RwLock<spellbook::Dictionary>>)> =
                state.dicts.iter().map(|d| (d.tag.clone(), d.bytes, Arc::clone(&d.dict))).collect();
            (existing, state.learned.clone())
        };

        let catalog = self.catalog();
        let mut next: Vec<LoadedDict> = Vec::new();
        let mut newly_created: Vec<Arc<RwLock<spellbook::Dictionary>>> = Vec::new();
        let mut total_bytes: usize = 0;

        for wanted in active.iter().take(MAX_LOADED_DICTS) {
            if next.iter().any(|d| tags_match(&d.tag, wanted)) {
                continue;
            }
            if let Some((tag, bytes, dict)) = existing.iter().find(|(t, _, _)| tags_match(t, wanted)) {
                if !next.is_empty() && total_bytes.saturating_add(*bytes) > MAX_LOADED_BYTES {
                    continue;
                }
                total_bytes = total_bytes.saturating_add(*bytes);
                next.push(LoadedDict { tag: tag.clone(), bytes: *bytes, dict: Arc::clone(dict) });
                continue;
            }
            let Some(entry) = catalog.iter().find(|e| tags_match(&e.tag, wanted)) else {
                continue;
            };
            let Some((aff, dic)) = read_pair(&entry.aff, &entry.dic) else {
                continue;
            };
            let file_bytes = aff.len().saturating_add(dic.len());
            if !next.is_empty() && total_bytes.saturating_add(file_bytes) > MAX_LOADED_BYTES {
                continue;
            }
            let Some(mut dict) = hunspell::parse(&aff, &dic) else {
                continue;
            };
            for word in &learned {
                let escaped = hunspell::escape_dic_word(word);
                dict.add(&escaped).ok();
            }
            let shared = Arc::new(RwLock::new(dict));
            newly_created.push(Arc::clone(&shared));
            total_bytes = total_bytes.saturating_add(file_bytes);
            next.push(LoadedDict { tag: entry.tag.clone(), bytes: file_bytes, dict: shared });
        }

        let (result, extra_learned) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let extra = if state.learned.len() > learned.len() { state.learned[learned.len()..].to_vec() } else { Vec::new() };
            if state.generation == generation {
                (state.dicts.iter().map(|d| Arc::clone(&d.dict)).collect(), Vec::new())
            } else if current_generation() == generation {
                let handles: Vec<Arc<RwLock<spellbook::Dictionary>>> = next.iter().map(|d| Arc::clone(&d.dict)).collect();
                state.generation = generation;
                state.dicts = next;
                (handles, extra)
            } else {
                (next.into_iter().map(|d| d.dict).collect(), extra)
            }
        };

        if !extra_learned.is_empty() {
            for dict in newly_created {
                let mut guard = dict.write().unwrap_or_else(|e| e.into_inner());
                for word in &extra_learned {
                    let escaped = hunspell::escape_dic_word(word);
                    guard.add(&escaped).ok();
                }
            }
        }

        result
    }

    pub(crate) fn misspelled(&self, generation: u64, active: &[String], text: &str, current_generation: impl Fn() -> u64) -> Vec<(usize, usize)> {
        let dicts = self.ensure_loaded(generation, active, current_generation);
        if dicts.is_empty() {
            return Vec::new();
        }
        let guards: Vec<_> = dicts.iter().map(|d| d.read().unwrap_or_else(|e| e.into_inner())).collect();
        let refs: Vec<&spellbook::Dictionary> = guards.iter().map(|g| &**g).collect();
        hunspell::misspelled(&refs, text)
    }

    pub(crate) fn suggest(&self, generation: u64, active: &[String], word: &str, current_generation: impl Fn() -> u64) -> Vec<String> {
        let dicts = self.ensure_loaded(generation, active, current_generation);
        if dicts.is_empty() {
            return Vec::new();
        }
        let guards: Vec<_> = dicts.iter().map(|d| d.read().unwrap_or_else(|e| e.into_inner())).collect();
        let refs: Vec<&spellbook::Dictionary> = guards.iter().map(|g| &**g).collect();
        hunspell::suggest(&refs, word)
    }

    pub(crate) fn learn(&self, word: &str) {
        let word = word.trim();
        if word.is_empty() || word.len() > MAX_LEARNED_WORD_BYTES {
            return;
        }
        let dicts: Vec<Arc<RwLock<spellbook::Dictionary>>> = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.learned.len() < MAX_LEARNED_WORDS && !state.learned.iter().any(|w| w == word) {
                state.learned.push(word.to_owned());
            }
            state.dicts.iter().map(|d| Arc::clone(&d.dict)).collect()
        };
        let escaped = hunspell::escape_dic_word(word);
        for dict in dicts {
            let mut guard = dict.write().unwrap_or_else(|e| e.into_inner());
            guard.add(&escaped).ok();
        }
    }
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
fn backend() -> &'static LinuxBackend {
    static BACKEND: OnceLock<LinuxBackend> = OnceLock::new();
    BACKEND.get_or_init(LinuxBackend::system)
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn misspelled(text: &str) -> Vec<(usize, usize)> {
    let generation = super::generation();
    let active = super::languages();
    backend().misspelled(generation, &active, text, super::generation)
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn suggest(word: &str) -> Vec<String> {
    let generation = super::generation();
    let active = super::languages();
    backend().suggest(generation, &active, word, super::generation)
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn learn(word: &str) {
    backend().learn(word);
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn available() -> Vec<String> {
    backend().available()
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn preferred() -> Vec<String> {
    backend().preferred()
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn english_name(tag: &str) -> String {
    tag.to_owned()
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn name_in(_: &str, _: &str) -> Option<String> {
    None
}

#[cfg_attr(not(all(target_os = "linux", feature = "hunspell")), allow(dead_code))]
pub(super) fn sort_in<T>(_: &mut [(String, T)], _: &str) -> bool {
    false
}

/// Normalizes a Hunspell filename stem or POSIX locale tag into a canonical BCP 47 tag
/// (`en_US` -> `en-US`, `zh_hans` -> `zh-Hans`, `de_at` -> `de-AT`).
/// Returns `None` for empty, non-ASCII, or malformed tags.
pub(crate) fn normalize_tag(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(raw.len());
    for (idx, part) in raw.split(['-', '_']).enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        if idx == 0 {
            if !(2..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphabetic()) {
                return None;
            }
            for b in part.bytes() {
                out.push(b.to_ascii_lowercase() as char);
            }
        } else {
            out.push('-');
            if idx == 1 && part.len() == 4 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
                // BCP 47 script subtag (e.g. Hans, Hant, Latn).
                let mut bytes = part.bytes();
                if let Some(first) = bytes.next() {
                    out.push(first.to_ascii_uppercase() as char);
                }
                for b in bytes {
                    out.push(b.to_ascii_lowercase() as char);
                }
            } else if (idx == 1 || idx == 2) && part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
                // BCP 47 ISO 3166-1 alpha-2 region subtag (e.g. US, GB, DE).
                for b in part.bytes() {
                    out.push(b.to_ascii_uppercase() as char);
                }
            } else {
                out.push_str(part);
            }
        }
    }
    Some(out)
}

/// Resolves the Linux Hunspell search directories in priority order from environment lookup `var`.
pub(crate) fn search_dirs_from(var: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push_unique = |path: PathBuf| {
        if !path.as_os_str().is_empty() && !dirs.contains(&path) {
            dirs.push(path);
        }
    };

    if let Some(dicpath) = var("DICPATH").filter(|v| !v.is_empty()) {
        for p in std::env::split_paths(&dicpath) {
            if !p.as_os_str().is_empty() {
                push_unique(p);
            }
        }
    }

    let xdg_data_home = var("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| is_xdg_absolute(p));
    if let Some(data_home) = xdg_data_home {
        push_unique(data_home.join("hunspell"));
    } else if let Some(home) = var("HOME").filter(|v| !v.is_empty()).map(PathBuf::from) {
        push_unique(home.join(".local").join("share").join("hunspell"));
    }

    if let Some(data_dirs) = var("XDG_DATA_DIRS").filter(|v| !v.is_empty()) {
        for p in std::env::split_paths(&data_dirs) {
            if is_xdg_absolute(&p) {
                push_unique(p.join("hunspell"));
            }
        }
    }

    for sys in SYSTEM_DIRS {
        push_unique(PathBuf::from(sys));
    }

    dirs
}

/// Parses POSIX preferred languages from `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, and `LANG`.
/// Strips `.codeset` and `@modifier`, skips `C` and `POSIX`, normalizes tags, and deduplicates.
pub(crate) fn preferred_from(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let names = var("LANGUAGE").unwrap_or_default();
    let singles = ["LC_ALL", "LC_MESSAGES", "LANG"].map(|name| var(name).unwrap_or_default());
    for raw in names.split(':').chain(singles.iter().map(String::as_str)) {
        let base = raw.split(['.', '@']).next().unwrap_or("").trim();
        if base.is_empty() || base.eq_ignore_ascii_case("C") || base.eq_ignore_ascii_case("POSIX") {
            continue;
        }
        let Some(tag) = normalize_tag(base) else {
            continue;
        };
        if !out.iter().any(|existing| tags_match(existing, &tag)) {
            out.push(tag);
        }
    }
    out
}

/// Scans `dirs` in priority order for `<stem>.aff` + `<stem>.dic` pairs.
/// Earlier directories win on duplicate language tags. Follows symlinks via `Path::is_file`.
pub(crate) fn discover_in(dirs: &[PathBuf]) -> Vec<DictEntry> {
    let mut found: Vec<DictEntry> = Vec::new();
    for dir in dirs {
        if found.len() >= MAX_DISCOVERED_DICTS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        // Collect per-stem .aff and .dic paths in deterministic stem order.
        let mut pairs: BTreeMap<String, (Option<PathBuf>, Option<PathBuf>)> = BTreeMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            let is_aff = ext.eq_ignore_ascii_case("aff");
            let is_dic = ext.eq_ignore_ascii_case("dic");
            if !is_aff && !is_dic {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !path.is_file() {
                continue;
            }
            let slot = pairs.entry(stem.to_owned()).or_insert((None, None));
            if is_aff {
                slot.0 = Some(path);
            } else {
                slot.1 = Some(path);
            }
        }
        for (stem, (aff, dic)) in pairs {
            if found.len() >= MAX_DISCOVERED_DICTS {
                break;
            }
            let (Some(aff), Some(dic)) = (aff, dic) else {
                continue;
            };
            let Some(tag) = normalize_tag(&stem) else {
                continue;
            };
            if found.iter().any(|existing| tags_match(&existing.tag, &tag)) {
                continue;
            }
            found.push(DictEntry { tag, aff, dic });
        }
    }
    found
}

fn is_xdg_absolute(path: &Path) -> bool {
    path.is_absolute() || path.to_str().is_some_and(|s| s.starts_with('/'))
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_DICT_FILE_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

fn read_pair(aff: &Path, dic: &Path) -> Option<(Vec<u8>, Vec<u8>)> {
    Some((read_bounded(aff)?, read_bounded(dic)?))
}
