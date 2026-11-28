//! Off-thread system fallback font loading for live windows.
//!
//! When enabled on a `TextSystem` (live windows and overlays; never headless/snapshot runtimes or
//! `measure`), uncached system fallback queries and font file mappings run on a single shared
//! background worker thread (`"rune-font-fallback"`). The UI thread returns immediately with
//! whatever fonts are already loaded, performing zero blocking fontconfig/DirectWrite/CoreText
//! lookups or file I/O. Once the worker resolves the paths and memory-maps the font files into the
//! shared `blobs` cache, it deposits the ready blobs into the requesting `TextSystem`'s inbox and
//! invokes its wake callback so the next frame registers them in memory, drops stale shaped
//! layouts, and redraws cleanly.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Weak;

use parley::fontique::{Blob, FallbackKey, Script};

use crate::TextSystem;
use crate::fonts::fallback;

use super::{
    SYMBOL_FAMILIES, load_preloaded, present_families, register_system_cjk_ids,
    system_fonts_enabled,
};

pub(super) fn register_system_cjk_preloaded(
    collection: &mut parley::fontique::Collection,
    loaded: &mut super::LoadedFiles,
    entries: Vec<PreloadedFont>,
    families: (&str, &str),
    all: &mut Vec<parley::fontique::FamilyId>,
) {
    let added = load_preloaded(collection, loaded, entries);
    register_system_cjk_ids(collection, added, families, all);
}

impl TextSystem {
    #[cfg_attr(not(all(unix, not(target_vendor = "apple"))), allow(dead_code))]
    pub(super) fn add_fallback_emoji_preloaded(&mut self, family: &str, entries: Vec<PreloadedFont>) {
        if self.emoji.is_some() {
            return;
        }
        let added = load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
        self.finish_fallback_emoji(family, added);
    }

    pub(super) fn add_symbol_entries(&mut self, entries: Vec<PreloadedFont>) {
        let added = load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
        self.register_symbol_family_ids(added);
    }
}

#[cfg(not(target_arch = "wasm32"))]
use super::{SYSTEM_MONO_FAMILIES, blobs, platform};

/// Wake callback invoked from the background fallback worker when preloaded font blobs arrive.
pub(crate) type AsyncWaker = Arc<dyn Fn() + Send + Sync>;

/// Explicit load phase for a single fallback slot so repeated frames while a background load is in
/// flight never enqueue duplicate jobs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SlotLoadState {
    #[default]
    Idle,
    Pending,
    Loaded,
}

#[cfg(windows)]
pub(super) const WINDOWS_TEXT_SYMBOLS: [&str; 4] = ["Segoe UI", "Arial", "Times New Roman", "Segoe UI Symbol"];

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(super) enum FallbackJob {
    #[cfg(not(windows))]
    Symbols {
        #[cfg(target_vendor = "apple")]
        skip_slot_prefix: bool,
    },
    #[cfg(any(windows, target_vendor = "apple"))]
    Emoji,
    Math,
    Script {
        index: usize,
        sample: char,
        locale: Option<String>,
        also_mono_scripts: bool,
    },
    Cjk {
        chars: Vec<char>,
        locale: Option<String>,
    },
    MissingChars {
        chars: Vec<char>,
        locale: Option<String>,
        #[cfg(windows)]
        windows_text_symbols: bool,
    },
    SystemMono,
    InstalledFamily {
        name: String,
    },
}

pub(super) type PreloadedFont = (PathBuf, Blob<u8>);

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) enum FallbackResult {
    #[cfg(not(windows))]
    Symbols {
        entries: Vec<PreloadedFont>,
        #[cfg(all(unix, not(target_vendor = "apple")))]
        emoji: Option<(String, Vec<PreloadedFont>)>,
    },
    #[cfg(any(windows, target_vendor = "apple"))]
    Emoji {
        entries: Vec<PreloadedFont>,
    },
    Math {
        entries: Vec<PreloadedFont>,
    },
    Script {
        index: usize,
        entries: Vec<PreloadedFont>,
        mono_entries: Option<Vec<PreloadedFont>>,
    },
    Cjk {
        entries: Vec<PreloadedFont>,
    },
    MissingChars {
        entries: Vec<PreloadedFont>,
        #[cfg(windows)]
        windows_text_entries: Option<Vec<PreloadedFont>>,
    },
    SystemMono {
        preferred_family: Option<String>,
        entries: Vec<PreloadedFont>,
    },
    InstalledFamily {
        entries: Vec<PreloadedFont>,
    },
}

struct Inbox {
    ready: AtomicBool,
    results: Mutex<Vec<FallbackResult>>,
}

impl Inbox {
    fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            results: Mutex::new(Vec::new()),
        }
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn push(&self, result: FallbackResult) {
        self.results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(result);
        self.ready.store(true, Ordering::Release);
    }

    fn drain(&self) -> Vec<FallbackResult> {
        if !self.ready.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }
        let mut guard = self.results.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}

/// Per-`TextSystem` state for non-blocking background system fallback loading.
#[derive(Default)]
pub(super) struct AsyncFallbacks {
    waker: Option<AsyncWaker>,
    inbox: Option<Arc<Inbox>>,
    #[cfg(not(windows))]
    symbols: SlotLoadState,
    #[cfg(any(windows, target_vendor = "apple"))]
    emoji: SlotLoadState,
    #[cfg(not(windows))]
    #[cfg_attr(target_vendor = "apple", allow(dead_code))]
    pub(super) emoji_wanted_with_symbols: bool,
    math: SlotLoadState,
    mono_scripts: SlotLoadState,
    system_mono: SlotLoadState,
}

impl AsyncFallbacks {
    pub(super) fn set_waker(&mut self, waker: Option<AsyncWaker>) {
        if waker.is_some() && self.inbox.is_none() {
            self.inbox = Some(Arc::new(Inbox::new()));
        }
        self.waker = waker;
    }

    #[inline]
    pub(super) fn is_enabled(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.waker.is_some()
        }
    }

    #[inline]
    pub(super) fn is_ready(&self) -> bool {
        self.inbox.as_ref().is_some_and(|inbox| inbox.is_ready())
    }

    pub(super) fn drain_ready(&self) -> Vec<FallbackResult> {
        self.inbox.as_ref().map_or_else(Vec::new, |inbox| inbox.drain())
    }

    #[cfg(not(windows))]
    pub(super) fn symbols_pending(&self) -> bool {
        self.symbols == SlotLoadState::Pending
    }

    #[cfg(not(windows))]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) fn mark_symbols_pending(&mut self, emoji_wanted: bool) -> bool {
        self.emoji_wanted_with_symbols |= emoji_wanted;
        if self.symbols != SlotLoadState::Idle {
            return false;
        }
        self.symbols = SlotLoadState::Pending;
        true
    }

    #[cfg(not(windows))]
    pub(super) fn mark_symbols_loaded(&mut self) -> bool {
        let was_pending = self.symbols == SlotLoadState::Pending;
        self.symbols = SlotLoadState::Loaded;
        was_pending
    }

    #[cfg(not(windows))]
    #[cfg_attr(target_vendor = "apple", allow(dead_code))]
    pub(super) fn take_emoji_wanted_with_symbols(&mut self) -> bool {
        std::mem::take(&mut self.emoji_wanted_with_symbols)
    }

    #[cfg(any(windows, target_vendor = "apple"))]
    pub(super) fn emoji_pending(&self) -> bool {
        self.emoji == SlotLoadState::Pending
    }

    #[cfg(any(windows, target_vendor = "apple"))]
    pub(super) fn mark_emoji_pending(&mut self) -> bool {
        if self.emoji != SlotLoadState::Idle {
            return false;
        }
        self.emoji = SlotLoadState::Pending;
        true
    }

    #[cfg(any(windows, target_vendor = "apple"))]
    pub(super) fn mark_emoji_loaded(&mut self) -> bool {
        let was_pending = self.emoji == SlotLoadState::Pending;
        self.emoji = SlotLoadState::Loaded;
        was_pending
    }

    pub(super) fn math_pending(&self) -> bool {
        self.math == SlotLoadState::Pending
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) fn mark_math_pending(&mut self) -> bool {
        if self.math != SlotLoadState::Idle {
            return false;
        }
        self.math = SlotLoadState::Pending;
        true
    }

    pub(super) fn mark_math_loaded(&mut self) {
        self.math = SlotLoadState::Loaded;
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) fn mark_mono_scripts_pending(&mut self) -> bool {
        if self.mono_scripts != SlotLoadState::Idle {
            return false;
        }
        self.mono_scripts = SlotLoadState::Pending;
        true
    }

    pub(super) fn mark_mono_scripts_loaded(&mut self) {
        self.mono_scripts = SlotLoadState::Loaded;
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) fn mark_system_mono_pending(&mut self) -> bool {
        if self.system_mono != SlotLoadState::Idle {
            return false;
        }
        self.system_mono = SlotLoadState::Pending;
        true
    }

    pub(super) fn mark_system_mono_loaded(&mut self) {
        self.system_mono = SlotLoadState::Loaded;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn enqueue(&mut self, job: FallbackJob) -> bool {
        if !system_fonts_enabled() {
            return false;
        }
        let Some(waker) = self.waker.clone() else {
            return false;
        };
        let inbox = Arc::downgrade(self.inbox.get_or_insert_with(|| Arc::new(Inbox::new())));
        dispatch_job(WorkItem { job, inbox, waker })
    }

    #[cfg(test)]
    pub(super) fn push_for_test(&mut self, result: FallbackResult) {
        let inbox = self.inbox.get_or_insert_with(|| Arc::new(Inbox::new()));
        inbox.push(result);
        if let Some(waker) = self.waker.as_ref() {
            waker();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct WorkItem {
    job: FallbackJob,
    inbox: Weak<Inbox>,
    waker: AsyncWaker,
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_job(item: WorkItem) -> bool {
    use std::sync::{OnceLock, mpsc};

    static WORKER: OnceLock<Option<mpsc::Sender<WorkItem>>> = OnceLock::new();
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<WorkItem>();
        let spawned = std::thread::Builder::new()
            .name("rune-font-fallback".into())
            .spawn(move || {
                while let Ok(item) = rx.recv() {
                    if !system_fonts_enabled() {
                        continue;
                    }
                    let result = run_job(item.job);
                    if let Some(inbox) = item.inbox.upgrade() {
                        inbox.push(result);
                        (item.waker)();
                    }
                }
            });
        spawned.ok().map(|_| tx)
    });
    tx.as_ref().is_some_and(|tx| tx.send(item).is_ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn run_job(job: FallbackJob) -> FallbackResult {
    match job {
        #[cfg(not(windows))]
        FallbackJob::Symbols {
            #[cfg(target_vendor = "apple")]
            skip_slot_prefix,
        } => {
            #[cfg(target_vendor = "apple")]
            let query_names = if skip_slot_prefix {
                &SYMBOL_FAMILIES[super::SLOT_FALLBACK_FAMILIES.len()..]
            } else {
                SYMBOL_FAMILIES
            };
            #[cfg(not(target_vendor = "apple"))]
            let query_names = SYMBOL_FAMILIES;
            let entries = blobs::preload_paths(platform::family_files(query_names));
            #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
            let emoji = super::fontconfig::emoji().map(|(family, files)| (family, blobs::preload_paths(files)));
            #[cfg(target_os = "android")]
            let emoji = {
                let files = super::android::emoji_files();
                (!files.is_empty()).then(|| (String::new(), blobs::preload_paths(files)))
            };
            FallbackResult::Symbols {
                entries,
                #[cfg(all(unix, not(target_vendor = "apple")))]
                emoji,
            }
        }
        #[cfg(any(windows, target_vendor = "apple"))]
        FallbackJob::Emoji => {
            #[cfg(target_vendor = "apple")]
            let files = platform::family_files(super::EMOJI_FAMILIES);
            #[cfg(windows)]
            let files = platform::family_files(SYMBOL_FAMILIES);
            FallbackResult::Emoji {
                entries: blobs::preload_paths(files),
            }
        }
        FallbackJob::Math => FallbackResult::Math {
            entries: blobs::preload_paths(platform::family_files(fallback::MATH_FAMILIES)),
        },
        FallbackJob::Script {
            index,
            sample,
            locale,
            also_mono_scripts,
        } => {
            let files = platform::fallback_family_files(&[sample], locale.as_deref())
                .into_iter()
                .take(16);
            let entries = blobs::preload_paths(files);
            let mono_entries = also_mono_scripts.then(|| {
                blobs::preload_paths(platform::family_files(fallback::MONO_SCRIPT_FAMILIES))
            });
            FallbackResult::Script {
                index,
                entries,
                mono_entries,
            }
        }
        FallbackJob::Cjk { chars, locale } => {
            let files = platform::fallback_family_files(&chars, locale.as_deref())
                .into_iter()
                .take(16);
            FallbackResult::Cjk {
                entries: blobs::preload_paths(files),
            }
        }
        FallbackJob::MissingChars {
            chars,
            locale,
            #[cfg(windows)]
            windows_text_symbols,
        } => {
            #[cfg(windows)]
            let windows_text_entries = windows_text_symbols.then(|| {
                blobs::preload_paths(platform::family_files(&WINDOWS_TEXT_SYMBOLS))
            });
            let entries = blobs::preload_paths(platform::fallback_family_files(&chars, locale.as_deref()));
            FallbackResult::MissingChars {
                entries,
                #[cfg(windows)]
                windows_text_entries,
            }
        }
        FallbackJob::SystemMono => {
            #[cfg_attr(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"))), allow(unused_mut))]
            let mut preferred_family = None;
            let mut paths = Vec::new();
            #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
            if let Some((family, files)) = super::fontconfig::monospace() {
                preferred_family = Some(family);
                paths.extend(files);
            }
            #[cfg(target_os = "android")]
            for path in super::android::monospace_files() {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            for path in platform::family_files(SYSTEM_MONO_FAMILIES).into_iter().take(12) {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            FallbackResult::SystemMono {
                preferred_family,
                entries: blobs::preload_paths(paths),
            }
        }
        FallbackJob::InstalledFamily { name } => FallbackResult::InstalledFamily {
            entries: blobs::preload_paths(platform::family_files(&[name.as_str()])),
        },
    }
}

impl TextSystem {
    /// Installs or clears the wake callback used for off-thread system fallback loading.
    ///
    /// Live windows and overlays install their wake lane here so uncached fallback families load
    /// off the UI thread and trigger a relayout when ready. Headless runtimes, snapshot captures,
    /// and `rune_text::measure` keep this `None` so layout stays synchronous and deterministic.
    #[doc(hidden)]
    pub fn set_async_fallback_waker(&mut self, waker: Option<AsyncWaker>) {
        self.fallbacks.async_load.set_waker(waker);
    }

    /// Whether this text system dispatches uncached system fallback lookups off the UI thread.
    #[doc(hidden)]
    pub fn async_fallbacks_enabled(&self) -> bool {
        self.fallbacks.async_load.is_enabled()
    }

    /// Lock-free check for whether any background-loaded system fallback fonts have arrived and
    /// are waiting to be registered on the next frame.
    #[doc(hidden)]
    pub fn async_fallbacks_ready(&self) -> bool {
        self.fallbacks.async_load.is_ready()
    }

    /// Registers any background-loaded system fallback fonts that have arrived since the last
    /// frame (zero file I/O, pure in-memory registration from pre-mapped blobs). Drops shaped
    /// caches and returns `true` when a relayout/redraw is needed.
    #[doc(hidden)]
    pub fn follow_async_fallbacks(&mut self) -> bool {
        let results = self.fallbacks.async_load.drain_ready();
        if results.is_empty() || !system_fonts_enabled() {
            return false;
        }
        let mut changed = false;
        for result in results {
            changed |= self.apply_async_fallback(result);
        }
        if changed {
            self.drop_shaped();
        }
        changed
    }

    fn apply_async_fallback(&mut self, result: FallbackResult) -> bool {
        let files_before = self.font_files.len();
        match result {
            #[cfg(not(windows))]
            FallbackResult::Symbols {
                entries,
                #[cfg(all(unix, not(target_vendor = "apple")))]
                emoji,
            } => {
                let was_pending = self.fallbacks.async_load.mark_symbols_loaded();
                self.fallbacks.symbols = true;
                load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                #[cfg(target_vendor = "apple")]
                for name in present_families(&mut self.font_cx.collection, SYMBOL_FAMILIES) {
                    self.push_symbol_family(name, false);
                }
                #[cfg(not(target_vendor = "apple"))]
                {
                    self.symbol_families = present_families(&mut self.font_cx.collection, SYMBOL_FAMILIES);
                    self.emoji = self.symbol_families.iter().copied().find(|f| f.contains("Emoji"));
                    #[cfg(all(unix, not(target_vendor = "apple")))]
                    if self.emoji.is_none()
                        && let Some((family, emoji_entries)) = emoji
                    {
                        self.add_fallback_emoji_preloaded(&family, emoji_entries);
                    }
                    if self.fallbacks.async_load.take_emoji_wanted_with_symbols() {
                        self.fallbacks.asked.insert('\u{FE0F}');
                    }
                }
                was_pending || self.font_files.len() > files_before
            }
            #[cfg(any(windows, target_vendor = "apple"))]
            FallbackResult::Emoji { entries } => {
                let was_pending = self.fallbacks.async_load.mark_emoji_loaded();
                self.fallbacks.asked.insert('\u{FE0F}');
                #[cfg(target_vendor = "apple")]
                {
                    load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                    for name in present_families(&mut self.font_cx.collection, super::EMOJI_FAMILIES) {
                        self.push_symbol_family(name, true);
                    }
                }
                #[cfg(windows)]
                {
                    self.add_symbol_entries(entries);
                }
                was_pending || self.font_files.len() > files_before
            }
            FallbackResult::Math { entries } => {
                self.fallbacks.async_load.mark_math_loaded();
                let before = self.math.clone();
                load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                self.math = Some(present_families(&mut self.font_cx.collection, fallback::MATH_FAMILIES));
                self.font_files.len() > files_before || self.math != before
            }
            FallbackResult::Script {
                index,
                entries,
                mono_entries,
            } => {
                let mut changed = false;
                let added = load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                if !added.is_empty() {
                    let fonts = &fallback::SCRIPT_FONTS[index];
                    if matches!(fonts.script, b"Arab" | b"Hebr") {
                        self.font_cx.collection.append_fallbacks(
                            FallbackKey::new(Script::from_bytes(*b"Latn"), None),
                            added.iter().copied(),
                        );
                    }
                    self.font_cx.collection.append_fallbacks(
                        FallbackKey::new(Script::from_bytes(*fonts.script), None),
                        added.into_iter(),
                    );
                    changed = true;
                }
                if let Some(mono_entries) = mono_entries {
                    self.fallbacks.async_load.mark_mono_scripts_loaded();
                    load_preloaded(&mut self.font_cx.collection, &mut self.font_files, mono_entries);
                    let present = present_families(&mut self.font_cx.collection, fallback::MONO_SCRIPT_FAMILIES);
                    changed |= !present.is_empty();
                    self.mono_scripts = Some(present);
                }
                changed || self.font_files.len() > files_before
            }
            FallbackResult::Cjk { entries } => {
                let families = (self.families.0.clone(), self.families.1.clone());
                let prev = self.fallbacks.cjk_families.len();
                register_system_cjk_preloaded(
                    &mut self.font_cx.collection,
                    &mut self.font_files,
                    entries,
                    (&families.0, &families.1),
                    &mut self.fallbacks.cjk_families,
                );
                for &id in &self.fallbacks.cjk_families[prev..] {
                    if !self.fallbacks.system_cjk.contains(&id) {
                        self.fallbacks.system_cjk.push(id);
                    }
                }
                self.fallbacks.cjk_families.len() > prev || self.font_files.len() > files_before
            }
            FallbackResult::MissingChars {
                entries,
                #[cfg(windows)]
                windows_text_entries,
            } => {
                let sym_before = self.symbol_families.len();
                #[cfg(windows)]
                if let Some(text_entries) = windows_text_entries {
                    self.add_symbol_entries(text_entries);
                    let (mut text, rest): (Vec<_>, Vec<_>) = self
                        .symbol_families
                        .drain(..)
                        .partition(|f| WINDOWS_TEXT_SYMBOLS.contains(f));
                    text.sort_by_key(|f| WINDOWS_TEXT_SYMBOLS.iter().position(|n| n == f));
                    self.symbol_families = text.into_iter().chain(rest).collect();
                }
                self.add_symbol_entries(entries);
                self.symbol_families.len() > sym_before || self.font_files.len() > files_before
            }
            FallbackResult::SystemMono {
                preferred_family,
                entries,
            } => {
                self.fallbacks.async_load.mark_system_mono_loaded();
                let added = load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                let mut names = Vec::new();
                if let Some(family) = preferred_family {
                    names.push(family);
                }
                #[cfg(target_os = "android")]
                for id in added {
                    if let Some(name) = self.font_cx.collection.family_name(id)
                        && !names.iter().any(|n| n == name)
                    {
                        names.push(name.to_owned());
                    }
                }
                #[cfg(not(target_os = "android"))]
                let _ = added;
                for name in super::SYSTEM_MONO_FAMILIES {
                    if !names.iter().any(|n| n == name) {
                        names.push((*name).to_owned());
                    }
                }
                names.retain(|name| self.font_cx.collection.family_id(name).is_some());
                let changed = !names.is_empty() || self.font_files.len() > files_before;
                self.system_mono = Some(names);
                changed
            }
            FallbackResult::InstalledFamily { entries } => {
                let added = load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                !added.is_empty() || self.font_files.len() > files_before
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "async_load_tests.rs"]
mod tests;
