//! Linux and the BSDs: system font files found through fontconfig, in process (fontique's
//! system collection over the dlopened `libfontconfig`, which reads fontconfig's own cache), as
//! DirectWrite finds them on Windows (`direct_write`): no font folder is walked and no
//! `fc-match` or `fc-list` is started. A character the app's fonts lack goes to fontconfig's
//! match for its script and language, the font a GTK or Qt app falls back to.
//!
//! Resolved family and script file lists are kept in bounded process-wide caches alongside the
//! shared memory-mapped blob cache (`blobs`). [`prewarm`] builds the fontconfig collection and
//! preloads the common fallback families (symbols, emoji, CJK, monospace) on the same background
//! worker so warmed apps hit memory with zero UI-thread fontconfig or file I/O.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use parley::fontique::{Collection, CollectionOptions, FallbackKey, FamilyId, GenericFamily, Language, Script, SourceKind};

use super::blobs;
use super::warm::WarmSlot;

/// Process-wide fontconfig system collection so building it on any thread (or via
/// `prewarm_system_fonts`) serves every thread without repeating the fontconfig load.
static SYSTEM: WarmSlot<Collection> = WarmSlot::new();

const MAX_FAMILY_CACHE_ENTRIES: usize = 256;
const MAX_SCRIPT_CACHE_ENTRIES: usize = 128;
const MAX_PATH_CACHE_BYTES: usize = 256 * 1024;

struct PathCache<K> {
    map: HashMap<K, Vec<PathBuf>>,
    order: VecDeque<K>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl<K: Eq + Hash + Clone> PathCache<K> {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    fn paths_bytes(paths: &[PathBuf]) -> usize {
        paths.iter().map(|p| p.as_os_str().len()).sum()
    }

    fn get(&self, key: &K) -> Option<Vec<PathBuf>> {
        self.map.get(key).cloned()
    }

    fn insert(&mut self, key: K, paths: Vec<PathBuf>) {
        if self.map.contains_key(&key) {
            return;
        }
        let cost = Self::paths_bytes(&paths);
        if self.max_entries == 0 || cost > self.max_bytes {
            return;
        }
        while self.order.len() >= self.max_entries || self.bytes.saturating_add(cost) > self.max_bytes {
            let Some(old_key) = self.order.pop_front() else {
                break;
            };
            if let Some(old_paths) = self.map.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(Self::paths_bytes(&old_paths));
            }
        }
        self.bytes = self.bytes.saturating_add(cost);
        self.order.push_back(key.clone());
        self.map.insert(key, paths);
    }
}

fn family_cache() -> &'static Mutex<PathCache<String>> {
    static CACHE: OnceLock<Mutex<PathCache<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(PathCache::new(MAX_FAMILY_CACHE_ENTRIES, MAX_PATH_CACHE_BYTES)))
}

fn script_cache() -> &'static Mutex<PathCache<([u8; 4], Option<Language>)>> {
    static CACHE: OnceLock<Mutex<PathCache<([u8; 4], Option<Language>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(PathCache::new(MAX_SCRIPT_CACHE_ENTRIES, MAX_PATH_CACHE_BYTES)))
}

static EMOJI_CACHE: OnceLock<Option<(String, Vec<PathBuf>)>> = OnceLock::new();
static MONO_CACHE: OnceLock<Option<(String, Vec<PathBuf>)>> = OnceLock::new();

fn build_system() -> Collection {
    Collection::new(CollectionOptions { shared: false, system_fonts: true })
}

fn with_system<R>(f: impl FnOnce(&mut Collection) -> R) -> R {
    SYSTEM.with(build_system, f)
}

/// Starts building fontconfig's process-wide system collection and preloading common fallback
/// families (symbols, colour emoji, CJK, monospace) on a single background thread.
pub(super) fn prewarm() {
    SYSTEM.prewarm_and_then(
        "rune-font-prewarm",
        super::system_fonts_enabled,
        build_system,
        preload_common_fallbacks,
    );
}

/// Resolves and memory-maps the common Linux fallback families on the prewarm background worker,
/// releasing `SYSTEM` between queries so concurrent callers are never blocked behind all steps.
fn preload_common_fallbacks() {
    // 1. Common symbol & emoji families (DejaVu Sans, Noto Sans, Noto Sans Symbols, Noto Color Emoji...).
    for &name in super::LINUX_SYMBOL_FAMILIES {
        if !super::system_fonts_enabled() {
            return;
        }
        let files = family_files(&[name]);
        let _ = blobs::preload_paths(files);
    }
    if !super::system_fonts_enabled() {
        return;
    }
    if let Some((_, files)) = emoji() {
        let _ = blobs::preload_paths(files);
    }

    // 2. Common CJK script fallbacks matching `language::of_text`'s default locales
    // (`zh-CN` for untagged Han, `ja` for kana, `ko` for Hangul, plus `None` for direct calls).
    for (sample, locale) in [
        ('\u{6F22}', Some("zh-CN")),
        ('\u{6F22}', None),
        ('\u{3042}', Some("ja")),
        ('\u{30A2}', Some("ja")),
        ('\u{D55C}', Some("ko")),
    ] {
        if !super::system_fonts_enabled() {
            return;
        }
        let files = fallback_family_files(&[sample], locale);
        let _ = blobs::preload_paths(files.into_iter().take(16));
    }

    // 3. System monospace fallback.
    if !super::system_fonts_enabled() {
        return;
    }
    if let Some((_, files)) = monospace() {
        let _ = blobs::preload_paths(files);
    }
    for &name in super::SYSTEM_MONO_FAMILIES {
        if !super::system_fonts_enabled() {
            return;
        }
        let files = family_files(&[name]);
        let _ = blobs::preload_paths(files.into_iter().take(12));
    }
}

/// Every file of family `id` in the system collection.
fn files_of(system: &mut Collection, id: FamilyId, out: &mut Vec<PathBuf>) {
    let Some(family) = system.family(id) else { return };
    for font in family.fonts() {
        if let SourceKind::Path(path) = &font.source().kind
            && !out.iter().any(|f| f.as_path() == path.as_ref())
        {
            out.push(path.to_path_buf());
        }
    }
}

fn push_unique(out: &mut Vec<PathBuf>, paths: Vec<PathBuf>) {
    for path in paths {
        if !out.contains(&path) {
            out.push(path);
        }
    }
}

/// Non-blocking cache-only lookup for `family_files(names)`. Returns `Some(paths)` when every
/// requested family name is already resolved in memory (zero fontconfig / lock contention on
/// `SYSTEM`), or `None` on any miss.
pub(super) fn try_cached_family_files(names: &[&str]) -> Option<Vec<PathBuf>> {
    if names.is_empty() {
        return Some(Vec::new());
    }
    let cache = family_cache().lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for name in names {
        let paths = cache.get(&name.to_lowercase())?;
        push_unique(&mut out, paths);
    }
    Some(out)
}

/// The files of the installed families `names` (any of their names, case aside), in order.
pub(super) fn family_files(names: &[&str]) -> Vec<PathBuf> {
    if names.is_empty() {
        return Vec::new();
    }
    if let Some(hit) = try_cached_family_files(names) {
        return hit;
    }
    let mut out = Vec::new();
    for name in names {
        let key = name.to_lowercase();
        let cached = family_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key);
        let paths = match cached {
            Some(paths) => paths,
            None => {
                let resolved = with_system(|system| {
                    let mut paths = Vec::new();
                    if let Some(id) = system.family_id(name) {
                        files_of(system, id, &mut paths);
                    }
                    paths
                });
                family_cache()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, resolved.clone());
                resolved
            }
        };
        push_unique(&mut out, paths);
    }
    out
}

/// Every installed font family from fontconfig's system collection, without walking font folders.
pub(super) fn installed_families() -> Vec<super::installed::RawFamily> {
    with_system(installed_families_from_collection)
}

pub(super) fn installed_families_from_collection(system: &mut Collection) -> Vec<super::installed::RawFamily> {
    let ids: Vec<FamilyId> = system.family_ids().collect();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(family) = system.family(id) else { continue };
        let name = family.name().to_owned();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let mut weights = Vec::new();
        for font in family.fonts() {
            weights.push(super::installed::clamp_weight(font.weight().value()));
            if let Some(axis) = font.axes().iter().find(|a| a.tag.to_be_bytes() == *b"wght") {
                super::installed::push_weight_range(&mut weights, axis.min, axis.max);
            }
        }
        let monospace = family
            .default_font()
            .or_else(|| family.fonts().first())
            .is_some_and(font_is_monospace);
        out.push(super::installed::RawFamily { name, monospace, weights });
    }
    out
}

fn font_is_monospace(font: &parley::fontique::FontInfo) -> bool {
    match &font.source().kind {
        SourceKind::Memory(blob) => {
            super::installed::reader_is_monospace(&mut std::io::Cursor::new(blob.as_ref()), font.index())
        }
        SourceKind::Path(path) => std::fs::File::open(path)
            .ok()
            .is_some_and(|mut f| super::installed::reader_is_monospace(&mut f, font.index())),
    }
}

/// Non-blocking cache-only lookup for `monospace()`.
pub(super) fn try_cached_monospace() -> Option<Option<(String, Vec<PathBuf>)>> {
    MONO_CACHE.get().cloned()
}

/// The family fontconfig resolves `monospace` to, and its files.
pub(super) fn monospace() -> Option<(String, Vec<PathBuf>)> {
    MONO_CACHE
        .get_or_init(|| {
            with_system(|system| {
                let id = system.generic_families(GenericFamily::Monospace).next()?;
                let name = system.family_name(id)?.to_owned();
                let mut files = Vec::new();
                files_of(system, id, &mut files);
                Some((name, files))
            })
        })
        .clone()
}

/// Non-blocking cache-only lookup for `emoji()`.
pub(super) fn try_cached_emoji() -> Option<Option<(String, Vec<PathBuf>)>> {
    EMOJI_CACHE.get().cloned()
}

/// The first colour emoji family fontconfig resolves `emoji` to, and its files.
pub(super) fn emoji() -> Option<(String, Vec<PathBuf>)> {
    EMOJI_CACHE
        .get_or_init(|| {
            with_system(|system| {
                let (id, name) = super::first_color_emoji_family(system)?;
                let mut files = Vec::new();
                files_of(system, id, &mut files);
                Some((name, files))
            })
        })
        .clone()
}

/// Extracts the distinct non-Common/Inherited/Unknown ISO 15924 script tags of `chars`.
fn script_tags(chars: &[char]) -> Vec<[u8; 4]> {
    use icu_properties::{CodePointMapData, PropertyNamesShort, props::Script as UScript};
    let names = PropertyNamesShort::<UScript>::new();
    let mut scripts: Vec<[u8; 4]> = Vec::new();
    for &ch in chars {
        let script = CodePointMapData::<UScript>::new().get(ch);
        if matches!(script, UScript::Common | UScript::Inherited | UScript::Unknown) {
            continue;
        }
        let Some(tag) = names.get_locale_script(script) else {
            continue;
        };
        let Ok(bytes) = <[u8; 4]>::try_from(tag.as_str().as_bytes()) else {
            continue;
        };
        if !scripts.contains(&bytes) {
            scripts.push(bytes);
        }
    }
    scripts
}

/// Non-blocking cache-only lookup for `fallback_family_files(chars, locale)`.
pub(super) fn try_cached_fallback_family_files(chars: &[char], locale: Option<&str>) -> Option<Vec<PathBuf>> {
    let scripts = script_tags(chars);
    if scripts.is_empty() {
        return Some(Vec::new());
    }
    let language = locale.and_then(|tag| Language::parse(tag).ok());
    let cache = script_cache().lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for script in scripts {
        let paths = cache.get(&(script, language))?;
        push_unique(&mut out, paths);
    }
    Some(out)
}

/// The files of the fonts fontconfig matches for the scripts of `chars` in `locale`, each
/// family whole (its bold and light faces too). A character in no script of its own (symbols,
/// punctuation) or that no installed font covers adds nothing.
pub(super) fn fallback_family_files(chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
    let scripts = script_tags(chars);
    if scripts.is_empty() {
        return Vec::new();
    }
    if let Some(hit) = try_cached_fallback_family_files(chars, locale) {
        return hit;
    }
    let language = locale.and_then(|tag| Language::parse(tag).ok());
    let mut out = Vec::new();
    for script in scripts {
        let key = (script, language);
        let cached = script_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key);
        let paths = match cached {
            Some(paths) => paths,
            None => {
                let resolved = with_system(|system| {
                    let mut paths = Vec::new();
                    let ids: Vec<FamilyId> = system
                        .fallback_families(FallbackKey::new(Script::from_bytes(script), language.as_ref()))
                        .take(2)
                        .collect();
                    for id in ids {
                        files_of(system, id, &mut paths);
                    }
                    paths
                });
                script_cache()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, resolved.clone());
                resolved
            }
        };
        push_unique(&mut out, paths);
    }
    out
}
