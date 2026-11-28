//! Cached-hit and async-dispatch bridges for system fallback font loading.
//!
//! Each entry point first checks whether the needed platform resolution and memory-mapped blobs
//! are already cached in memory (e.g. warmed by `prewarm_system_fonts()`), registering them
//! synchronously with zero fontconfig/OS queries and zero file I/O. On a cache miss, live
//! windows with async fallbacks enabled dispatch a background job and return immediately;
//! headless runs, snapshots, and `measure` stay synchronous and deterministic.

use parley::fontique::{FallbackKey, Script};

use crate::TextSystem;
use crate::fonts::fallback;

use super::async_load::{PreloadedFont, register_system_cjk_preloaded};
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use super::async_load::FallbackJob;
#[cfg(windows)]
use super::async_load::WINDOWS_TEXT_SYMBOLS;
use super::{
    load_preloaded, present_families, system_fonts_enabled,
};
use super::SYMBOL_FAMILIES;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use super::{SYSTEM_MONO_FAMILIES, blobs, platform};

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn try_cached_family_entries(names: &[&str]) -> Option<Vec<PreloadedFont>> {
    let paths = platform::try_cached_family_files(names)?;
    blobs::try_cached_paths(&paths)
}

#[cfg(any(target_arch = "wasm32", target_os = "android"))]
fn try_cached_family_entries(_: &[&str]) -> Option<Vec<PreloadedFont>> {
    Some(Vec::new())
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn try_cached_fallback_entries(chars: &[char], locale: Option<&str>, cap: usize) -> Option<Vec<PreloadedFont>> {
    let mut paths = platform::try_cached_fallback_family_files(chars, locale)?;
    if paths.len() > cap {
        paths.truncate(cap);
    }
    blobs::try_cached_paths(&paths)
}

#[cfg(any(target_arch = "wasm32", target_os = "android"))]
fn try_cached_fallback_entries(_: &[char], _: Option<&str>, _: usize) -> Option<Vec<PreloadedFont>> {
    Some(Vec::new())
}

#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
fn try_cached_linux_emoji() -> Option<Option<(String, Vec<PreloadedFont>)>> {
    match super::fontconfig::try_cached_emoji()? {
        None => Some(None),
        Some((family, files)) => Some(Some((family, blobs::try_cached_paths(&files)?))),
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn try_cached_system_mono() -> Option<(Option<String>, Vec<PreloadedFont>)> {
    #[cfg_attr(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"))), allow(unused_mut))]
    let mut preferred_family = None;
    let mut paths = Vec::new();
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    if let Some((family, files)) = super::fontconfig::try_cached_monospace()? {
        preferred_family = Some(family);
        paths.extend(files);
    }
    let fallback_paths = platform::try_cached_family_files(SYSTEM_MONO_FAMILIES)?;
    for path in fallback_paths.into_iter().take(12) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let entries = blobs::try_cached_paths(&paths)?;
    Some((preferred_family, entries))
}

#[cfg(any(target_arch = "wasm32", target_os = "android"))]
fn try_cached_system_mono() -> Option<(Option<String>, Vec<PreloadedFont>)> {
    Some((None, Vec::new()))
}

impl TextSystem {
    pub(super) fn load_script_fallback_maybe_async(&mut self, index: usize, ch: char, locale: Option<&str>) {
        let needs_mono = self.mono_scripts.is_none()
            && matches!(fallback::SCRIPT_FONTS[index].script, b"Arab" | b"Hebr");
        let cached_script = try_cached_fallback_entries(&[ch], locale, 16);
        let cached_mono = if needs_mono {
            try_cached_family_entries(fallback::MONO_SCRIPT_FAMILIES)
        } else {
            Some(Vec::new())
        };
        if let (Some(entries), Some(mono_entries)) = (cached_script, cached_mono) {
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
            }
            if needs_mono {
                load_preloaded(&mut self.font_cx.collection, &mut self.font_files, mono_entries);
                self.mono_scripts = Some(present_families(&mut self.font_cx.collection, fallback::MONO_SCRIPT_FAMILIES));
                self.fallbacks.async_load.mark_mono_scripts_loaded();
            }
            return;
        }
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        if self.fallbacks.async_load.is_enabled() {
            let also_mono_scripts = needs_mono && self.fallbacks.async_load.mark_mono_scripts_pending();
            self.fallbacks.async_load.enqueue(FallbackJob::Script {
                index,
                sample: ch,
                locale: locale.map(str::to_owned),
                also_mono_scripts,
            });
            return;
        }
        super::register_script_fallbacks(
            &mut self.font_cx.collection,
            &mut self.font_files,
            &fallback::SCRIPT_FONTS[index],
            ch,
            locale,
        );
        if needs_mono {
            self.mono_scripts = Some(super::load_families(
                &mut self.font_cx.collection,
                &mut self.font_files,
                fallback::MONO_SCRIPT_FILES,
                fallback::MONO_SCRIPT_FAMILIES,
            ));
            self.fallbacks.async_load.mark_mono_scripts_loaded();
        }
    }

    pub(super) fn load_cjk_fallback_maybe_async(&mut self, cjk: Vec<char>, locale: Option<&str>, cjk_done: bool) {
        self.fallbacks.asked.extend(cjk.iter().copied());
        if !cjk_done {
            self.fallbacks.cjk.push(locale.map(str::to_owned));
        }
        if let Some(entries) = try_cached_fallback_entries(&cjk, locale, 16) {
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
            return;
        }
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        if self.fallbacks.async_load.is_enabled() {
            self.fallbacks.async_load.enqueue(FallbackJob::Cjk {
                chars: cjk,
                locale: locale.map(str::to_owned),
            });
            return;
        }
        let files = super::platform::fallback_family_files(&cjk, locale);
        let families = (self.families.0.clone(), self.families.1.clone());
        let prev = self.fallbacks.cjk_families.len();
        super::register_system_cjk(
            &mut self.font_cx.collection,
            &mut self.font_files,
            files,
            (&families.0, &families.1),
            &mut self.fallbacks.cjk_families,
        );
        for &id in &self.fallbacks.cjk_families[prev..] {
            if !self.fallbacks.system_cjk.contains(&id) {
                self.fallbacks.system_cjk.push(id);
            }
        }
    }

    #[cfg(not(windows))]
    pub(super) fn load_symbol_families_maybe_async(&mut self, emoji_wanted: bool) -> bool {
        if self.fallbacks.symbols {
            return true;
        }
        if self.fallbacks.async_load.symbols_pending() {
            if emoji_wanted {
                self.fallbacks.async_load.emoji_wanted_with_symbols = true;
            }
            return false;
        }
        #[cfg(target_vendor = "apple")]
        let query_names = if self.fallbacks.slot_fallback {
            &SYMBOL_FAMILIES[super::SLOT_FALLBACK_FAMILIES.len()..]
        } else {
            SYMBOL_FAMILIES
        };
        #[cfg(not(target_vendor = "apple"))]
        let query_names = SYMBOL_FAMILIES;
        let cached_entries = try_cached_family_entries(query_names);
        #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
        let cached_emoji = try_cached_linux_emoji();
        #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
        let ready = cached_entries.is_some() && cached_emoji.is_some();
        #[cfg(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"))))]
        let ready = cached_entries.is_some();
        if ready && let Some(entries) = cached_entries {
            self.fallbacks.symbols = true;
            self.fallbacks.async_load.mark_symbols_loaded();
            load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
            #[cfg(target_vendor = "apple")]
            for name in present_families(&mut self.font_cx.collection, SYMBOL_FAMILIES) {
                self.push_symbol_family(name, false);
            }
            #[cfg(not(target_vendor = "apple"))]
            {
                self.symbol_families = present_families(&mut self.font_cx.collection, SYMBOL_FAMILIES);
                self.emoji = self.symbol_families.iter().copied().find(|f| f.contains("Emoji"));
                #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
                if self.emoji.is_none()
                    && let Some(Some((family, emoji_entries))) = cached_emoji
                {
                    self.add_fallback_emoji_preloaded(&family, emoji_entries);
                }
            }
            return true;
        }
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        if self.fallbacks.async_load.is_enabled() {
            if self.fallbacks.async_load.mark_symbols_pending(emoji_wanted) {
                self.fallbacks.async_load.enqueue(FallbackJob::Symbols {
                    #[cfg(target_vendor = "apple")]
                    skip_slot_prefix: self.fallbacks.slot_fallback,
                });
            }
            return false;
        }
        self.load_symbol_families();
        self.fallbacks.async_load.mark_symbols_loaded();
        true
    }

    pub(super) fn load_emoji_maybe_async(&mut self) -> bool {
        #[cfg(not(any(windows, target_vendor = "apple")))]
        {
            let loaded = self.load_symbol_families_maybe_async(true);
            if loaded {
                self.fallbacks.asked.insert('\u{FE0F}');
            }
            loaded
        }
        #[cfg(any(windows, target_vendor = "apple"))]
        {
            if self.fallbacks.async_load.emoji_pending() {
                return false;
            }
            #[cfg(target_vendor = "apple")]
            let names = super::EMOJI_FAMILIES;
            #[cfg(windows)]
            let names = SYMBOL_FAMILIES;
            if let Some(entries) = try_cached_family_entries(names) {
                self.fallbacks.asked.insert('\u{FE0F}');
                self.fallbacks.async_load.mark_emoji_loaded();
                #[cfg(target_vendor = "apple")]
                {
                    load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                    for name in present_families(&mut self.font_cx.collection, super::EMOJI_FAMILIES) {
                        self.push_symbol_family(name, true);
                    }
                }
                #[cfg(windows)]
                self.add_symbol_entries(entries);
                return true;
            }
            if self.fallbacks.async_load.is_enabled() {
                if self.fallbacks.async_load.mark_emoji_pending() {
                    self.fallbacks.async_load.enqueue(FallbackJob::Emoji);
                }
                return false;
            }
            self.fallbacks.asked.insert('\u{FE0F}');
            #[cfg(windows)]
            self.add_symbol_files(super::direct_write::family_files(SYMBOL_FAMILIES));
            #[cfg(target_vendor = "apple")]
            self.load_emoji_family();
            self.fallbacks.async_load.mark_emoji_loaded();
            true
        }
    }

    pub(super) fn load_math_maybe_async(&mut self) -> bool {
        if self.math.is_some() {
            return true;
        }
        if self.fallbacks.async_load.math_pending() {
            return false;
        }
        if let Some(entries) = try_cached_family_entries(fallback::MATH_FAMILIES) {
            load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
            self.math = Some(present_families(&mut self.font_cx.collection, fallback::MATH_FAMILIES));
            self.fallbacks.async_load.mark_math_loaded();
            return true;
        }
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        if self.fallbacks.async_load.is_enabled() {
            if self.fallbacks.async_load.mark_math_pending() {
                self.fallbacks.async_load.enqueue(FallbackJob::Math);
            }
            return false;
        }
        self.math = Some(super::load_families(
            &mut self.font_cx.collection,
            &mut self.font_files,
            fallback::MATH_FONT_FILES,
            fallback::MATH_FAMILIES,
        ));
        self.fallbacks.async_load.mark_math_loaded();
        true
    }

    pub(super) fn load_missing_chars_maybe_async(
        &mut self,
        missing: Vec<char>,
        locale: Option<&str>,
        #[cfg(windows)] windows_text_symbols: bool,
    ) {
        #[cfg(windows)]
        if windows_text_symbols {
            self.fallbacks.text_symbols = true;
        }
        self.fallbacks.asked.extend(missing.iter().copied());
        let cached_chars = try_cached_fallback_entries(&missing, locale, usize::MAX);
        #[cfg(windows)]
        let cached_text = if windows_text_symbols {
            try_cached_family_entries(&WINDOWS_TEXT_SYMBOLS)
        } else {
            Some(Vec::new())
        };
        #[cfg(windows)]
        let ready = cached_chars.is_some() && cached_text.is_some();
        #[cfg(not(windows))]
        let ready = cached_chars.is_some();
        if ready && let Some(entries) = cached_chars {
            #[cfg(windows)]
            if windows_text_symbols && let Some(text_entries) = cached_text {
                self.add_symbol_entries(text_entries);
                let (mut text, rest): (Vec<_>, Vec<_>) = self
                    .symbol_families
                    .drain(..)
                    .partition(|f| WINDOWS_TEXT_SYMBOLS.contains(f));
                text.sort_by_key(|f| WINDOWS_TEXT_SYMBOLS.iter().position(|n| n == f));
                self.symbol_families = text.into_iter().chain(rest).collect();
            }
            self.add_symbol_entries(entries);
            return;
        }
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        if self.fallbacks.async_load.is_enabled() {
            self.fallbacks.async_load.enqueue(FallbackJob::MissingChars {
                chars: missing,
                locale: locale.map(str::to_owned),
                #[cfg(windows)]
                windows_text_symbols,
            });
            return;
        }
        #[cfg(windows)]
        if windows_text_symbols {
            self.add_symbol_files(super::direct_write::family_files(&WINDOWS_TEXT_SYMBOLS));
            let (mut text, rest): (Vec<_>, Vec<_>) = self
                .symbol_families
                .drain(..)
                .partition(|f| WINDOWS_TEXT_SYMBOLS.contains(f));
            text.sort_by_key(|f| WINDOWS_TEXT_SYMBOLS.iter().position(|n| n == f));
            self.symbol_families = text.into_iter().chain(rest).collect();
        }
        self.add_symbol_files(super::platform::fallback_family_files(&missing, locale));
    }

    pub(crate) fn load_system_mono_maybe_async(&mut self) -> &[String] {
        if self.system_mono.is_some() {
            return self.system_mono.as_deref().unwrap_or_default();
        }
        if !system_fonts_enabled() {
            self.system_mono = Some(Vec::new());
            return self.system_mono.as_deref().unwrap_or_default();
        }
        if let Some((preferred_family, entries)) = try_cached_system_mono() {
            load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
            let mut names = Vec::new();
            if let Some(family) = preferred_family {
                names.push(family);
            }
            for name in super::SYSTEM_MONO_FAMILIES {
                if !names.iter().any(|n| n == name) {
                    names.push((*name).to_owned());
                }
            }
            names.retain(|name| self.font_cx.collection.family_id(name).is_some());
            self.system_mono = Some(names);
            self.fallbacks.async_load.mark_system_mono_loaded();
            return self.system_mono.as_deref().unwrap_or_default();
        }
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        if self.fallbacks.async_load.is_enabled() {
            if self.fallbacks.async_load.mark_system_mono_pending() {
                self.fallbacks.async_load.enqueue(FallbackJob::SystemMono);
            }
            return &[];
        }
        let _fallback = crate::profile::TextStageScope::enter(crate::profile::TextStage::SystemFallback);
        let names = super::load_system_mono(&mut self.font_cx.collection, &mut self.font_files);
        self.system_mono = Some(names);
        self.fallbacks.async_load.mark_system_mono_loaded();
        self.system_mono.as_deref().unwrap_or_default()
    }

    pub(crate) fn load_installed_maybe_async(&mut self, list: &str, emoji_mode: bool) {
        if !system_fonts_enabled() {
            return;
        }
        let parsed = if emoji_mode {
            super::emoji_family_list(list)
        } else {
            super::css_or_named(list)
        };
        let names: Vec<String> = parsed
            .into_iter()
            .filter_map(|f| match f {
                parley::FontFamilyName::Named(name) => Some(name.into_owned()),
                parley::FontFamilyName::Generic(_) => None,
            })
            .collect();
        for name in names {
            if self.font_cx.collection.family_id(&name).is_some()
                || !self.installed.insert(name.to_lowercase())
            {
                continue;
            }
            if let Some(entries) = try_cached_family_entries(&[name.as_str()]) {
                load_preloaded(&mut self.font_cx.collection, &mut self.font_files, entries);
                continue;
            }
            #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
            if self.fallbacks.async_load.is_enabled() {
                self.fallbacks.async_load.enqueue(FallbackJob::InstalledFamily { name });
                continue;
            }
            super::load_files(
                &mut self.font_cx.collection,
                &mut self.font_files,
                super::installed_family_files(&name),
            );
        }
    }
}
