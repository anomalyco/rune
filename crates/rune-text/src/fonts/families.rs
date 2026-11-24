//! The families behind the sans and mono slots, and changing them while text is cached.

use std::cell::RefCell;

use parley::{FontFamilyName, Layout, PositionedLayoutItem};
use rune_core::tok::Family;
use swash::FontRef;

use crate::{Ink, TextLayout, TextSystem, family_list, is_hidden_size, measure, named_list, names_system_mono};

thread_local! {
    /// The families last chosen for every window (`Tx::set_font_families`), numbered by change.
    /// Windows and overlays share the main thread; each text system follows on its next frame.
    static CHOSEN: RefCell<(u64, Option<(String, String)>)> = const { RefCell::new((0, None)) };
}

/// Chooses `sans` and `mono` for every window on this thread. Choosing the families already
/// chosen changes nothing.
#[doc(hidden)]
pub fn choose_families(sans: &str, mono: &str) {
    CHOSEN.with_borrow_mut(|(n, chosen)| {
        if chosen.as_ref().is_none_or(|(s, m)| s != sans || m != mono) {
            *n += 1;
            *chosen = Some((sans.to_owned(), mono.to_owned()));
        }
    });
}

/// The slot strings for `sans` and `mono`. Web has no system fonts to fall back on, so its
/// slots end in the bundled faces.
pub(crate) fn slots(sans: &str, mono: &str) -> (String, String) {
    if cfg!(target_arch = "wasm32") {
        (format!("{sans}, Inter"), format!("{mono}, IBM Plex Mono"))
    } else {
        (sans.to_owned(), mono.to_owned())
    }
}

impl TextSystem {
    /// A window's or overlay's text system: in the families chosen for every window when the
    /// app has chosen, else in `families`. Opening one after a switch then never sends
    /// [`measure`](super::measure) back to the launch families.
    #[doc(hidden)]
    pub fn for_window(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        CHOSEN.with_borrow(|(n, chosen)| {
            let families = chosen.as_ref().map_or(families, |(sans, mono)| (sans.as_str(), mono.as_str()));
            let mut text = TextSystem::new(fonts, families);
            text.chosen = *n;
            text
        })
    }

    /// The sans and mono slots' families (CSS lists), as [`TextSystem::new`] or
    /// [`Self::set_families`] named them.
    pub fn families(&self) -> (&str, &str) {
        (&self.families.0, &self.families.1)
    }

    /// Puts `sans` and `mono` (CSS lists, as `AppSpec::families` takes them) behind the slots.
    /// Styles name slots, not families, so every `Family::Sans` and `Family::Mono` text follows.
    /// Every layout shaped in the old families goes, and [`measure`](super::measure) follows
    /// too. Glyphs stay in the atlas under their own font's key until they age out. A family
    /// that isn't registered or installed falls back as it would at launch. Returns false,
    /// dropping nothing, when the slots already hold these families.
    pub fn set_families(&mut self, sans: &str, mono: &str) -> bool {
        if self.families.0 == sans && self.families.1 == mono {
            return false;
        }
        measure::note_families(sans, mono);
        self.apply_families(sans, mono);
        true
    }

    /// Updates this text system's slots without touching measurement state, for
    /// [`measure::with_text`]'s thread-local measurer.
    pub(crate) fn apply_families(&mut self, sans: &str, mono: &str) {
        if self.families.0 == sans && self.families.1 == mono {
            return;
        }
        self.load_installed(sans);
        self.load_installed(mono);
        self.families = (sans.to_owned(), mono.to_owned());
        (self.sans, self.mono) = slots(sans, mono);
        self.slot_system_mono = [names_system_mono(sans), names_system_mono(mono)];
        self.sync_cjk_fallbacks();
        self.load_slot_fallbacks();
        self.drop_shaped();
    }

    /// Whether shaping `family` (with `has_mono_span` inline mono spans) can resolve
    /// `ui-monospace` and therefore needs the platform's system monospace families loaded.
    pub(crate) fn needs_system_mono(&self, family: rune_core::tok::Family, has_mono_span: bool) -> bool {
        family == rune_core::tok::Family::SystemMono
            || (self.slot_system_mono[1] && (family == rune_core::tok::Family::Mono || has_mono_span))
            || (self.slot_system_mono[0] && matches!(family, rune_core::tok::Family::Sans | rune_core::tok::Family::Named(_)))
    }

    /// Drops every layout, run and measurement shaped so far, for new families or a new
    /// language. Glyphs stay in the atlas under their own font's key until they age out.
    pub(crate) fn drop_shaped(&mut self) {
        self.shaped_epoch = self.shaped_epoch.wrapping_add(1);
        self.layouts.clear();
        self.shapes.clear();
        self.superseded.clear();
        self.elisions.clear();
        self.runs.clear();
        self.ascii.clear();
        self.paragraphs.clear();
        self.last_paragraphs = None;
        self.primary_traks.clear();
        self.primary_axes.clear();
        self.glyph_cache.clear();
    }

    /// Whether families were chosen or runtime faces arrived since this text system last
    /// followed them.
    #[doc(hidden)]
    pub fn families_behind(&self) -> bool {
        CHOSEN.with_borrow(|(n, _)| *n != self.chosen) || self.full_faces_behind()
    }

    /// Takes up the families chosen since it last looked; true when that changed its slots.
    #[doc(hidden)]
    pub fn follow_chosen(&mut self) -> bool {
        CHOSEN.with_borrow(|(n, chosen)| {
            if *n == self.chosen {
                return false;
            }
            self.chosen = *n;
            chosen.as_ref().is_some_and(|(sans, mono)| self.set_families(sans, mono))
        })
    }

    /// The first available family name in the collection that `family` resolves to, following the
    /// same `named_list` / `family_list` fallback stack shaping uses. Used when a text node has no
    /// shaped glyph runs (empty text) or is secure.
    pub fn resolved_family(&self, family: Family) -> Option<String> {
        let system = self.system_mono.as_deref().unwrap_or_default();
        let mut collection = self.font_cx.collection.clone();
        let mut find = |f: FontFamilyName<'_>| -> Option<String> {
            let id = match f {
                FontFamilyName::Named(name) => collection.family_id(&name),
                FontFamilyName::Generic(generic) => collection.generic_families(generic).next(),
            }?;
            collection.family_name(id).map(str::to_owned)
        };
        match family {
            Family::Sans => family_list(&self.sans, system, &self.sans).into_iter().find_map(&mut find),
            Family::Mono => family_list(&self.mono, system, &self.sans).into_iter().find_map(&mut find),
            Family::Named(list) => named_list(list)
                .into_iter()
                .chain(family_list(&self.sans, system, &self.sans))
                .find_map(&mut find),
            Family::SystemMono => system
                .iter()
                .map(|name| FontFamilyName::named(name.as_str()))
                .chain(family_list(&self.mono, system, &self.sans))
                .find_map(&mut find),
        }
    }
}

/// The OpenType family name of the first visible shaped run in `layout` (`TypographicFamily`,
/// falling back to `Family`), or `None` when `layout` has no visible glyph runs.
pub fn resolved_family_of(layout: &TextLayout) -> Option<String> {
    match layout {
        TextLayout::Single(l) => layout_resolved_family(l),
        TextLayout::Paragraphs(p) => p
            .paragraphs
            .iter()
            .filter(|para| para.shaped)
            .find_map(|para| layout_resolved_family(&para.layout)),
    }
}

fn layout_resolved_family(layout: &Layout<Ink>) -> Option<String> {
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
            if is_hidden_size(gr.run().font_size()) {
                continue;
            }
            let font = gr.run().font();
            if let Some(name) = FontRef::from_index(font.data.as_ref(), font.index as usize).and_then(font_family_name) {
                return Some(name);
            }
        }
    }
    None
}

fn font_family_name(font: FontRef<'_>) -> Option<String> {
    let strings = font.localized_strings();
    let pick = |id| {
        strings
            .find_by_id(id, Some("en"))
            .or_else(|| strings.find_by_id(id, None))
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };
    pick(swash::StringId::TypographicFamily).or_else(|| pick(swash::StringId::Family))
}
