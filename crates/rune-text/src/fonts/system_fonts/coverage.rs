//! Active font coverage checks and on-demand system fallback loading (`TextSystem::load_fallbacks`).

use parley::{FontContext, FontFamilyName};
use parley::fontique::{
    Attributes, Blob, CharmapIndex, Collection, FamilyId, QueryFamily, QueryStatus,
};

use crate::{TextSystem, family_list, fonts::fallback, named_list};

#[cfg(any(windows, target_vendor = "apple"))]
use super::SYMBOL_FAMILIES;
use super::{
    emoji_family_list, is_cjk, load_families, needs_no_font, platform, reader_has_color_table,
    register_script_fallbacks, register_system_cjk,
};

#[cfg(windows)]
use super::direct_write;

/// Resolves `names` to deduplicated `FamilyId`s in `collection`, preserving order.
pub(super) fn push_family_ids<'a>(
    collection: &mut Collection,
    names: impl IntoIterator<Item = FontFamilyName<'a>>,
    out: &mut Vec<FamilyId>,
) {
    for family in names {
        match family {
            FontFamilyName::Named(name) => {
                if let Some(id) = collection.family_id(&name)
                    && !out.contains(&id)
                {
                    out.push(id);
                }
            }
            FontFamilyName::Generic(generic) => {
                for id in collection.generic_families(generic) {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
        }
    }
}

/// Resolves named family strings to deduplicated `FamilyId`s in `collection`, preserving order.
pub(super) fn push_named_ids<'a>(
    collection: &mut Collection,
    names: impl IntoIterator<Item = &'a str>,
    out: &mut Vec<FamilyId>,
) {
    for name in names {
        if let Some(id) = collection.family_id(name)
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
}

/// Lazy per-family charmap cache for a stack of resolved `FamilyId`s.
///
/// Families are queried through `fontique::Query` one at a time, only when earlier families do
/// not map the requested character, matching Parley's own face selection and charmap rules.
pub(super) struct FamilyCoverage {
    ids: Vec<FamilyId>,
    resolved: usize,
    faces: Vec<(Blob<u8>, CharmapIndex)>,
    attributes: Attributes,
}

impl FamilyCoverage {
    pub(super) fn from_ids(ids: Vec<FamilyId>, attributes: Attributes) -> Self {
        Self { ids, resolved: 0, faces: Vec::new(), attributes }
    }

    pub(super) fn from_names<'a>(
        collection: &mut Collection,
        names: impl IntoIterator<Item = &'a str>,
        attributes: Attributes,
    ) -> Self {
        let mut ids = Vec::new();
        push_named_ids(collection, names, &mut ids);
        Self::from_ids(ids, attributes)
    }

    pub(super) fn covers(&mut self, font_cx: &mut FontContext, ch: char) -> bool {
        if faces_cover(&self.faces, ch) {
            return true;
        }
        while self.resolved < self.ids.len() {
            let id = self.ids[self.resolved];
            self.resolved += 1;
            let start = self.faces.len();
            let mut query = font_cx.collection.query(&mut font_cx.source_cache);
            query.set_families([QueryFamily::Id(id)]);
            query.set_attributes(self.attributes);
            query.matches_with(|font| {
                self.faces.push((font.blob.clone(), font.charmap_index));
                QueryStatus::Continue
            });
            if faces_cover(&self.faces[start..], ch) {
                return true;
            }
        }
        false
    }
}

fn faces_cover(faces: &[(Blob<u8>, CharmapIndex)], ch: char) -> bool {
    faces
        .iter()
        .any(|(blob, cmap)| cmap.charmap(blob.as_ref()).is_some_and(|map| map.map(ch).is_some_and(|gid| gid != 0)))
}

impl TextSystem {
    /// Whether family `id`'s default (or first) face carries a colour glyph table (`COLR`, `CBDT`
    /// or `sbix`), cached per `FamilyId` and read through `source_cache`.
    fn is_color_family(&mut self, id: FamilyId) -> bool {
        if let Some(&cached) = self.fallbacks.color_families.get(&id) {
            return cached;
        }
        let font = self
            .font_cx
            .collection
            .family(id)
            .and_then(|f| f.default_font().or_else(|| f.fonts().first()).cloned());
        let is_color = font.is_some_and(|font| {
            font.load(Some(&mut self.font_cx.source_cache)).is_some_and(|blob| {
                reader_has_color_table(&mut std::io::Cursor::new(blob.as_ref()), Some(font.index()))
            })
        });
        self.fallbacks.color_families.insert(id, is_color);
        is_color
    }

    /// `FamilyId`s of colour fonts explicitly selected by name in the active stack for `family`
    /// (or the mono-span stack when `mono_span` is true), in shaping order and excluding
    /// `symbol_families`, `math`, and `mono_scripts`.
    fn selected_color_family_ids(&mut self, family: rune_core::tok::Family, mono_span: bool) -> Vec<FamilyId> {
        let mut candidates = Vec::new();
        let system = self.system_mono.as_deref().unwrap_or_default();
        let collection = &mut self.font_cx.collection;
        let push_named_only = |collection: &mut Collection, list: Vec<FontFamilyName<'_>>, out: &mut Vec<FamilyId>| {
            for entry in list {
                if let FontFamilyName::Named(name) = entry
                    && let Some(id) = collection.family_id(&name)
                    && !out.contains(&id)
                {
                    out.push(id);
                }
            }
        };
        if mono_span {
            push_named_only(collection, family_list(&self.mono, system, self.sans.as_str()), &mut candidates);
        } else {
            match family {
                rune_core::tok::Family::Sans => {
                    push_named_only(collection, family_list(&self.sans, system, self.sans.as_str()), &mut candidates);
                }
                rune_core::tok::Family::Mono | rune_core::tok::Family::SystemMono => {
                    push_named_only(collection, family_list(&self.mono, system, self.sans.as_str()), &mut candidates);
                }
                rune_core::tok::Family::Named(list) => {
                    push_named_only(collection, named_list(list), &mut candidates);
                    push_named_only(collection, family_list(&self.sans, system, self.sans.as_str()), &mut candidates);
                }
            }
        }
        let mut color = Vec::new();
        for id in candidates {
            if self.is_color_family(id) {
                color.push(id);
            }
        }
        color
    }

    /// `FamilyId`s resolved from the app's opt-in `preferred_emoji` setting, lazily loading any
    /// installed system font it names on first emoji encounter. Empty when `preferred_emoji` is
    /// `None` or resolves to no registered family.
    fn preferred_emoji_family_ids(&mut self) -> Vec<FamilyId> {
        let Some(list) = self.preferred_emoji.clone() else {
            return Vec::new();
        };
        self.load_installed_emoji(&list);
        let mut ids = Vec::new();
        push_family_ids(&mut self.font_cx.collection, emoji_family_list(&list), &mut ids);
        ids
    }

    /// Resolved family names for the app's opt-in `preferred_emoji` setting, in shaping order.
    pub(crate) fn preferred_emoji_family_names(&mut self) -> Vec<String> {
        let ids = self.preferred_emoji_family_ids();
        ids.into_iter()
            .filter_map(|id| self.font_cx.collection.family_name(id).map(str::to_owned))
            .collect()
    }

    /// Family names of colour fonts explicitly selected by name in the active stack for `family`
    /// (or the mono-span stack when `mono_span` is true), in shaping order.
    pub(crate) fn selected_color_family_names(&mut self, family: rune_core::tok::Family, mono_span: bool) -> Vec<String> {
        let ids = self.selected_color_family_ids(family, mono_span);
        ids.into_iter()
            .filter_map(|id| self.font_cx.collection.family_name(id).map(str::to_owned))
            .collect()
    }

    /// Active non-emoji `FamilyId`s in shaping order for `family` (or the mono-span stack when
    /// `mono_span` is true): primary families, optional `mono_scripts`, non-emoji
    /// `symbol_families`, and already-loaded `math`.
    fn active_family_ids(&mut self, family: rune_core::tok::Family, mono_span: bool) -> Vec<FamilyId> {
        let mut ids = Vec::new();
        let system = self.system_mono.as_deref().unwrap_or_default();
        let mono_scripts = self.mono_scripts.as_deref().unwrap_or(&[]);
        let collection = &mut self.font_cx.collection;
        let push_tail = |collection: &mut Collection, primary: &str, extra: &[&'static str], ids: &mut Vec<FamilyId>| {
            push_family_ids(collection, family_list(primary, system, self.sans.as_str()), ids);
            push_named_ids(collection, extra.iter().copied(), ids);
            push_named_ids(
                collection,
                self.symbol_families.iter().copied().filter(|f| self.emoji != Some(*f) && !f.contains("Emoji")),
                ids,
            );
            if let Some(math) = self.math.as_deref() {
                push_named_ids(collection, math.iter().copied(), ids);
            }
        };
        if mono_span {
            push_tail(collection, &self.mono, mono_scripts, &mut ids);
            return ids;
        }
        match family {
            rune_core::tok::Family::Sans => push_tail(collection, &self.sans, &[], &mut ids),
            rune_core::tok::Family::Mono => push_tail(collection, &self.mono, mono_scripts, &mut ids),
            rune_core::tok::Family::Named(list) => {
                push_family_ids(collection, named_list(list), &mut ids);
                push_tail(collection, &self.sans, &[], &mut ids);
            }
            rune_core::tok::Family::SystemMono => {
                push_named_ids(collection, system.iter().map(String::as_str), &mut ids);
                push_tail(collection, &self.mono, mono_scripts, &mut ids);
            }
        }
        ids
    }

    /// Moves any characters in `pending` not covered by `families` into `missing` (and into
    /// `symbol_math` when `symbol_math` is `Some` and `needs_symbols` holds off Windows).
    fn drain_uncovered(
        &mut self,
        pending: &mut Vec<char>,
        families: &[&str],
        attributes: Attributes,
        missing: &mut Vec<char>,
        mut symbol_math: Option<&mut Vec<char>>,
    ) {
        if pending.is_empty() {
            return;
        }
        let mut cov = if families.is_empty() {
            None
        } else {
            Some(FamilyCoverage::from_names(&mut self.font_cx.collection, families.iter().copied(), attributes))
        };
        for ch in pending.drain(..) {
            if cov.as_mut().is_some_and(|c| c.covers(&mut self.font_cx, ch)) {
                continue;
            }
            if !self.fallbacks.asked.contains(&ch) && !missing.contains(&ch) {
                missing.push(ch);
            }
            if let Some(sm) = symbol_math.as_deref_mut()
                && !cfg!(windows)
                && self.math.is_none()
                && fallback::needs_symbols(ch)
                && !sm.contains(&ch)
            {
                sm.push(ch);
            }
        }
    }

    /// Loads the system fonts `text` needs and the active fonts lack, each the first time:
    /// a script's fallback (`fallback::SCRIPT_FONTS`), CJK fonts, math fonts, the colour emoji
    /// family, and (Windows) the font Windows falls back to for any other character; elsewhere
    /// the symbol families. Text the active resolved fonts already cover loads nothing.
    pub(crate) fn load_fallbacks(
        &mut self,
        text: &str,
        style: rune_core::tok::TextStyle,
        mono_spans: &crate::layout::span_map::SpanMap<()>,
        routes: &[(std::ops::Range<usize>, fallback::Route)],
        language: Option<parley::fontique::Language>,
    ) {
        if text.is_ascii() {
            return;
        }
        let locale = language.as_ref().map(|l| l.to_string());
        let font_style = if style.italic { parley::FontStyle::Italic } else { parley::FontStyle::Normal };
        let attributes = Attributes::new(
            parley::fontique::FontWidth::NORMAL,
            font_style,
            parley::FontWeight::new(style.weight),
        );
        let primary_ids = self.active_family_ids(style.family, false);
        let mut primary_cov = FamilyCoverage::from_ids(primary_ids.clone(), attributes);
        let mut mono_cov = (!mono_spans.is_empty() && !matches!(style.family, rune_core::tok::Family::Mono | rune_core::tok::Family::SystemMono))
            .then(|| FamilyCoverage::from_ids(self.active_family_ids(style.family, true), attributes));
        let mut cjk_own_cov: Option<FamilyCoverage> = None;
        let mut preferred_emoji_cov: Option<FamilyCoverage> = None;
        let mut selected_color_primary: Option<FamilyCoverage> = None;
        let mut selected_color_mono: Option<FamilyCoverage> = None;

        let (mut math, mut emoji, mut scripts) = (false, false, Vec::new());
        let (mut cjk, mut cjk_more, mut missing): (Vec<char>, Vec<char>, Vec<char>) = (Vec::new(), Vec::new(), Vec::new());
        let (mut emoji_chars, mut math_chars, mut symbol_math_chars): (Vec<char>, Vec<char>, Vec<char>) = (Vec::new(), Vec::new(), Vec::new());
        let cjk_done = self.fallbacks.cjk.contains(&locale);
        let mut route_cursor = 0usize;
        for (idx, ch) in text.char_indices() {
            if ch.is_ascii() {
                continue;
            }
            if let Some(i) = fallback::SCRIPT_FONTS.iter().position(|fonts| fonts.covers(ch)) {
                if self.scripts & (1 << i) == 0 {
                    self.scripts |= 1 << i;
                    scripts.push((i, ch));
                }
                continue;
            }
            if is_cjk(ch) {
                let cov = cjk_own_cov.get_or_insert_with(|| {
                    let mut ids = primary_ids.clone();
                    for &id in &self.fallbacks.own_families {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                    FamilyCoverage::from_ids(ids, attributes)
                });
                if cov.covers(&mut self.font_cx, ch) {
                    continue;
                }
                // The language's own CJK font, the first time a text in it has CJK.
                if !cjk_done && !cjk.contains(&ch) && cjk.len() < 64 {
                    cjk.push(ch);
                } else if !self.fallbacks.asked.contains(&ch) && !cjk_more.contains(&ch) {
                    // A character the CJK fonts loaded so far may lack (Simplified Chinese in a
                    // Japanese font): the platform names a font that has it, once per character.
                    cjk_more.push(ch);
                }
                continue;
            }
            while route_cursor < routes.len() && routes[route_cursor].0.end <= idx {
                route_cursor += 1;
            }
            let route = routes.get(route_cursor).filter(|(r, _)| r.start <= idx).map(|(_, r)| *r);
            if route == Some(fallback::Route::Emoji) {
                if needs_no_font(ch) {
                    continue;
                }
                if self.preferred_emoji.is_some() {
                    let pref_cov = preferred_emoji_cov.get_or_insert_with(|| {
                        let ids = self.preferred_emoji_family_ids();
                        FamilyCoverage::from_ids(ids, attributes)
                    });
                    if pref_cov.covers(&mut self.font_cx, ch) {
                        continue;
                    }
                }
                let in_mono = !mono_spans.is_empty() && mono_spans.get(idx).is_some();
                let color_cov = if in_mono {
                    selected_color_mono.get_or_insert_with(|| {
                        let ids = self.selected_color_family_ids(style.family, true);
                        FamilyCoverage::from_ids(ids, attributes)
                    })
                } else {
                    selected_color_primary.get_or_insert_with(|| {
                        let ids = self.selected_color_family_ids(style.family, false);
                        FamilyCoverage::from_ids(ids, attributes)
                    })
                };
                if color_cov.covers(&mut self.font_cx, ch) {
                    continue;
                }
                emoji |= self.emoji.is_none() && !self.fallbacks.asked.contains(&'\u{FE0F}');
                if !self.fallbacks.asked.contains(&ch) && !emoji_chars.contains(&ch) {
                    emoji_chars.push(ch);
                }
                continue;
            }
            if route == Some(fallback::Route::Math) {
                math |= self.math.is_none();
                if !needs_no_font(ch) && !self.fallbacks.asked.contains(&ch) && !math_chars.contains(&ch) {
                    math_chars.push(ch);
                }
                continue;
            }
            // An emoji by default (✅ ⚡) wants the colour emoji family as `U+FE0F` does.
            emoji |= (ch == '\u{FE0F}' || fallback::is_emoji(ch.encode_utf8(&mut [0; 4]))) && self.emoji.is_none() && !self.fallbacks.asked.contains(&'\u{FE0F}');
            if needs_no_font(ch) {
                continue;
            }
            let in_mono = !mono_spans.is_empty() && mono_spans.get(idx).is_some();
            let covered = match (in_mono, mono_cov.as_mut()) {
                (true, Some(cov)) => cov.covers(&mut self.font_cx, ch),
                _ => primary_cov.covers(&mut self.font_cx, ch),
            };
            if covered {
                continue;
            }
            if !self.fallbacks.asked.contains(&ch) && !missing.contains(&ch) {
                missing.push(ch);
            }
            if !cfg!(windows) && self.math.is_none() && fallback::needs_symbols(ch) && !symbol_math_chars.contains(&ch) {
                symbol_math_chars.push(ch);
            }
        }
        // Check already-loaded emoji/math fonts before deciding whether any fallback work is needed.
        if !emoji {
            let loaded_emoji = self.emoji;
            self.drain_uncovered(&mut emoji_chars, loaded_emoji.as_slice(), attributes, &mut missing, Some(&mut symbol_math_chars));
        }
        if !math {
            let loaded_math: Vec<&'static str> = self.math.as_deref().unwrap_or(&[]).to_vec();
            self.drain_uncovered(&mut math_chars, &loaded_math, attributes, &mut missing, None);
        }
        if scripts.is_empty() && !math && cjk.is_empty() && cjk_more.is_empty() && !emoji && missing.is_empty() && symbol_math_chars.is_empty() {
            return;
        }
        // The first system font a launch loads, for its trace (`marks`).
        rune_core::marks::mark("text.fallback.begin");
        for (i, ch) in scripts {
            register_script_fallbacks(&mut self.font_cx.collection, &mut self.font_files, &fallback::SCRIPT_FONTS[i], ch, locale.as_deref());
            if self.mono_scripts.is_none() && matches!(fallback::SCRIPT_FONTS[i].script, b"Arab" | b"Hebr") {
                self.mono_scripts = Some(load_families(&mut self.font_cx.collection, fallback::MONO_SCRIPT_FILES, fallback::MONO_SCRIPT_FAMILIES));
            }
        }
        if !cjk.is_empty() || !cjk_more.is_empty() {
            self.fallbacks.asked.extend(cjk.iter().chain(&cjk_more).copied());
            cjk.extend(cjk_more);
            let files = platform::fallback_family_files(&cjk, locale.as_deref());
            if !cjk_done { self.fallbacks.cjk.push(locale.clone()); }
            let families = (self.families.0.clone(), self.families.1.clone());
            let prev = self.fallbacks.cjk_families.len();
            register_system_cjk(&mut self.font_cx.collection, &mut self.font_files, files, (&families.0, &families.1), &mut self.fallbacks.cjk_families);
            for &id in &self.fallbacks.cjk_families[prev..] {
                if !self.fallbacks.system_cjk.contains(&id) {
                    self.fallbacks.system_cjk.push(id);
                }
            }
        }
        if emoji {
            self.fallbacks.asked.insert('\u{FE0F}');
            #[cfg(windows)]
            self.add_symbol_files(direct_write::family_files(SYMBOL_FAMILIES));
            #[cfg(target_vendor = "apple")]
            self.load_emoji_family();
            #[cfg(not(any(windows, target_vendor = "apple")))]
            if !self.fallbacks.symbols {
                self.load_symbol_families();
            }
            let loaded_emoji = self.emoji;
            self.drain_uncovered(&mut emoji_chars, loaded_emoji.as_slice(), attributes, &mut missing, Some(&mut symbol_math_chars));
        }
        if math {
            self.math = Some(load_families(&mut self.font_cx.collection, fallback::MATH_FONT_FILES, fallback::MATH_FAMILIES));
            let loaded_math: Vec<&'static str> = self.math.as_deref().unwrap_or(&[]).to_vec();
            self.drain_uncovered(&mut math_chars, &loaded_math, attributes, &mut missing, None);
        }
        #[cfg(windows)]
        if !missing.is_empty() {
            // A symbol that isn't an emoji by default (☺ ✓ ♥) draws in a text face, as
            // Chromium draws it, not in the colour emoji Windows maps it to. Chromium tries
            // the page's families first, and official's end `system-ui, "Segoe UI",
            // sans-serif`: Segoe UI, then Arial (Windows' sans-serif), then the standard font
            // (Times New Roman), and only then the system fallback, Segoe UI Symbol. So ☺,
            // which Inter and Segoe UI lack, is Arial's (CDP `getPlatformFontsForNode`,
            // `proof\languages\symbols\chromium-faces.tsv`).
            if !self.fallbacks.text_symbols && missing.iter().any(|&c| fallback::needs_symbols(c) && !fallback::is_emoji(c.encode_utf8(&mut [0; 4]))) {
                self.fallbacks.text_symbols = true;
                const CHROMIUM: [&str; 4] = ["Segoe UI", "Arial", "Times New Roman", "Segoe UI Symbol"];
                self.add_symbol_files(direct_write::family_files(&CHROMIUM));
                // Ahead of the other symbol faces, in that order, wherever an earlier lookup
                // already put Segoe UI Symbol.
                let (mut text, rest): (Vec<_>, Vec<_>) = self.symbol_families.drain(..).partition(|f| CHROMIUM.contains(f));
                text.sort_by_key(|f| CHROMIUM.iter().position(|n| n == f));
                self.symbol_families = text.into_iter().chain(rest).collect();
            }
            self.fallbacks.asked.extend(missing.iter().copied());
            self.add_symbol_files(direct_write::fallback_family_files(&missing, locale.as_deref()));
        }
        #[cfg(target_vendor = "apple")]
        if !missing.is_empty() {
            if !self.fallbacks.symbols {
                self.load_symbol_families();
            }
            // Skip CoreText fallback for characters the non-emoji symbol families just loaded cover.
            let mut sym_cov = FamilyCoverage::from_names(&mut self.font_cx.collection, SYMBOL_FAMILIES.iter().copied(), attributes);
            missing.retain(|&ch| !sym_cov.covers(&mut self.font_cx, ch));
            // What the symbol families lack (Ethiopic, Khmer, Tibetan...): the platform's own
            // fallback, once per character.
            if !missing.is_empty() {
                self.fallbacks.asked.extend(missing.iter().copied());
                self.add_symbol_files(platform::fallback_family_files(&missing, locale.as_deref()));
            }
        }
        #[cfg(not(any(windows, target_vendor = "apple")))]
        if !missing.is_empty() {
            if !self.fallbacks.symbols {
                self.load_symbol_families();
            }
            // What the symbol families lack (Ethiopic, Khmer, Tibetan...): the platform's own
            // fallback, once per character.
            self.fallbacks.asked.extend(missing.iter().copied());
            self.add_symbol_files(platform::fallback_family_files(&missing, locale.as_deref()));
        }
        // Load the math fallback only when a non-emoji symbol in `needs_symbols` is still
        // uncovered after primary and non-emoji symbol fallbacks.
        if !cfg!(windows) && self.math.is_none() && !symbol_math_chars.is_empty() {
            let non_emoji: Vec<&'static str> = self
                .symbol_families
                .iter()
                .copied()
                .filter(|f| self.emoji != Some(*f) && !f.contains("Emoji"))
                .collect();
            let mut sym_cov = FamilyCoverage::from_names(&mut self.font_cx.collection, non_emoji, attributes);
            if symbol_math_chars.iter().any(|&ch| !sym_cov.covers(&mut self.font_cx, ch)) {
                self.math = Some(load_families(&mut self.font_cx.collection, fallback::MATH_FONT_FILES, fallback::MATH_FAMILIES));
            }
        }
        rune_core::marks::mark("text.fallback.loaded");
    }
}
