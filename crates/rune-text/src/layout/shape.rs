//! Shaping and line-breaking a keyed rich text layout ([`TextSystem::layout_keyed`]).

use std::rc::Rc;

use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight, Layout, LayoutContext,
    LineHeight, StyleProperty,
};

use crate::fonts::fallback::{self, Route};
use crate::fonts::{language, system_weight};
use crate::raster::synthetic_bold;
use crate::{
    Entry, Ink, Keep, PILL_BOX, PillChip, PillMetrics, Span, TextSystem, Trak, family_list, font_trak, mono_tab_stops,
    named_list, shape_key,
};
use rune_core::tok::{Family, OpticalSize, TextStyle};

use super::{hash, inline_fit, line_break};

/// How strictly Japanese and Chinese lines break, one switch. `Strict` (JIS X 4051, UAX #14's
/// default) never starts a line with a small kana (ぁ ッ ャ), ー or 々. `Normal` is Chromium's
/// `line-break: auto`, which lets them start one. Native Japanese readers decide which reads
/// better; this is the one place to change.
const LINE_BREAK: parley::LineBreak = parley::LineBreak::Strict;

/// The family every stack ends in: in a browser, the face the page draws (`platform_face`).
#[cfg(target_arch = "wasm32")]
const PLATFORM_FAMILY: &[&str] = crate::fonts::platform_face::FAMILY;
#[cfg(not(target_arch = "wasm32"))]
const PLATFORM_FAMILY: &[&str] = &[];

fn variations(style: TextStyle, size: f32) -> Vec<parley::FontVariation> {
    let mut settings: Vec<_> = parley::FontVariation::parse_css_list(style.variations).flatten().collect();
    let opsz = parley::setting::Tag::new(b"opsz");
    let value = match style.optical_size {
        OpticalSize::Auto if !settings.iter().any(|v| v.tag == opsz) => Some(size),
        OpticalSize::Fixed(value) => { settings.retain(|v| v.tag != opsz); Some(value) }
        _ => None,
    };
    if let Some(value) = value.filter(|v| v.is_finite() && *v > 0.0) {
        settings.push(parley::FontVariation::new(opsz, value));
    }
    settings
}

impl TextSystem {
    /// `shape` is [`shape_key`] of the same text when the caller already has it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_keyed(
        &mut self,
        shape: Option<u64>,
        text: &str,
        spans: &[Span],
        blocks: &[crate::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        wrap_anywhere: bool,
        pretty: bool,
        keep: Keep,
        base: parley::BaseDirection,
    ) -> Rc<Layout<Ink>> {
        self.follow_language();
        let cache = keep == Keep::Cached;
        let shared = (keep == Keep::Secret).then(|| std::mem::replace(&mut self.layout_cx, LayoutContext::new()));
        let has_indented_blocks = blocks.iter().any(|&b| b != crate::Block::Paragraph);
        // Shaping depends on the text, spans, style, scale, overflow-wrap and pills; line
        // breaking adds the width, alignment and block indents. A layout cached at one width
        // is re-broken at another rather than shaped again.
        let shape_key = shape.unwrap_or_else(|| hash::directed(shape_key(text, spans, style, scale, wrap_anywhere, pills), base));
        // Pretty wrapping breaks only wrapped proportional text again.
        let pretty = pretty && max_width.is_some() && !matches!(style.family, Family::Mono | Family::SystemMono);
        let key = hash::break_key(shape_key, max_width, align, has_indented_blocks.then_some(blocks)) ^ if pretty { 0x7072_6574_7479_0001 } else { 0 };
        let g = self.generation;
        if cache && let Some(Entry { value: l, used, .. }) = self.layouts.get_mut(&key) {
            *used = g;
            // A width change re-breaks the newest layout, not one measured since at another.
            self.shapes.entry(shape_key).or_insert(key);
            return l.clone();
        }
        // Each pill's label range, and the room before and after it in device px; boxes keep
        // the pill's index in their id, so ranges that fail the checks keep no chip.
        let pm = PillMetrics::new(style.size);
        let pills: Vec<(usize, std::ops::Range<usize>, u32, u32)> = pills
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let r = (p.start as usize).min(text.len())..(p.end as usize).min(text.len());
                let (lead, trail) = match &p.bare {
                    Some(b) => (b.size + b.gap, 0.0),
                    None => (pm.lead(p.icon.is_some()), pm.trail()),
                };
                (i, r, (lead * scale).round() as u32, (trail * scale).round() as u32)
            })
            .filter(|(_, r, ..)| {
                !r.is_empty() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end) && !text.as_bytes()[r.clone()].contains(&b'\n')
            })
            .collect();
        let indented = has_indented_blocks || !pills.is_empty();
        let break_lines = |layout: &mut Layout<Ink>| line_break::break_new(layout, text, blocks, indented, style.size, scale, max_width, pretty);
        if cache
            && let Some(prev) = self.shapes.get(&shape_key).copied()
            && let Some(Entry { value: shaped, used, .. }) = self.layouts.get(&prev)
        {
            self.rebroken += 1;
            let _break_time = crate::profile::TextScope::enter();
            // One this commit hasn't used and nothing else holds moves to the new width and
            // re-breaks in place; otherwise the shaped runs are copied.
            let (moves, mut l) = (*used != g && Rc::strong_count(shaped) == 1, shaped.clone());
            if moves { self.layouts.remove(&prev); self.glyph_cache.forget(&l); }
            let layout = Rc::make_mut(&mut l);
            break_lines(layout);
            layout.align(align, AlignmentOptions::default());
            // Re-broken, in place or as a copy: whatever it draws now is its own.
            self.layout_ids.build(&l);
            self.layouts.insert(key, Entry::new(l.clone(), g, text.len(), shape_key));
            self.shapes.insert(shape_key, key);
            return l;
        }
        self.shaped += 1;
        rune_core::marks::add("text.shaped", 1);
        let _shape_ns = rune_core::marks::Timed::start("text.shape_ns", true);
        let _shape_time = crate::profile::TextScope::enter();
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let tabs = if text.contains('\t') { mono_tab_stops(text, spans, mono_style) } else { Vec::new() };
        let widen = if tabs.is_empty() { Vec::new() } else { self.mono_tab_widening(&tabs, spans, style, scale) };
        // Parley has no tab stops and fonts draw a tab as a missing glyph (or nothing): shape it
        // as a space, one byte like the tab, so every byte range still holds. Monospace tabs
        // then widen to the next 4-column stop through word spacing.
        let spaced;
        let text = if text.contains('\t') {
            spaced = zeroize::Zeroizing::new(text.replace('\t', " "));
            spaced.as_str()
        } else {
            text
        };
        let routes = fallback::routes(text);
        let language = language::of_text(text, self.language);
        self.load_fallbacks(text, language);
        // `trak` tracking at the point size (see `trak.rs`), on top of `style.tracking`. An
        // explicit optical size `o` picks the entry instead, as Core Text tracks `o`-point text
        // scaled to the set size; spans scale the lookup with their size. Each style range gets
        // its predicted first font's spacing; runs in another font are checked after shaping.
        let trak_tag = parley::setting::Tag::new(b"trak");
        let mut features: Vec<parley::FontFeature> = parley::FontFeature::parse_css_list(style.features).flatten().collect();
        let tracked = features.iter().rev().find(|f| f.tag == trak_tag).is_none_or(|f| f.value != 0);
        let listed = features.len();
        features.retain(|f| f.tag != trak_tag);
        let stripped = features.len() != listed;
        let optical = match style.optical_size {
            OpticalSize::Fixed(o) if o.is_finite() && o > 0.0 && style.size > 0.0 => Some(o),
            _ => None,
        };
        let spacing = |trak: Option<Rc<Trak>>, size: f32| {
            let px = size * scale;
            let points = px / scale;
            // `o` itself at the style's size (a run's size comes back through device px).
            let points = optical.map_or(points, |o| {
                if (points - style.size).abs() <= style.size * 1e-6 { o } else { o * points / style.size }
            });
            style.tracking + trak.map_or(0.0, |t| t.spacing(px, points, tracked) / scale)
        };
        let base_spacing = spacing(self.primary_trak(style.family, style.weight, style.italic), style.size);
        let span_spacing: Vec<Option<f32>> = spans
            .iter()
            .map(|s| {
                if !(s.mono || s.italic || s.weight.is_some() || s.size.is_some()) {
                    return None;
                }
                let family = if s.mono { Family::Mono } else { style.family };
                let trak = self.primary_trak(family, s.weight.unwrap_or(style.weight), s.italic || style.italic);
                Some(spacing(trak, s.size.unwrap_or(style.size))).filter(|&v| v != base_spacing)
            })
            .collect();
        // Each style range's predicted first font at its weight, drawn at its named instance.
        let base_axes = self.primary_axes(style.family, style.weight, style.italic);
        // The strut inline code keeps inside (`inline_fit`).
        let strut = if mono_style || !spans.iter().any(|s| s.mono) { None } else { self.primary_extents(style.family, style.weight, style.italic) };
        let span_axes: Vec<Rc<[parley::FontVariation]>> = spans
            .iter()
            .map(|s| {
                let family = if s.mono { Family::Mono } else { style.family };
                self.primary_axes(family, s.weight.unwrap_or(style.weight), s.italic || style.italic)
            })
            .collect();
        let with_axes = |mut settings: Vec<parley::FontVariation>, axes: &[parley::FontVariation]| {
            let extra: Vec<_> = axes.iter().filter(|a| !settings.iter().any(|v| v.tag == a.tag)).copied().collect();
            settings.extend(extra);
            settings
        };
        if style.family == Family::SystemMono || self.slot_system_mono {
            self.system_mono();
        }
        if let Family::Named(list) = style.family {
            self.load_installed(list);
        }
        let math = self.math.as_deref().unwrap_or(&[]);
        let emoji = self.emoji.as_slice();
        let system = self.system_mono.as_deref().unwrap_or_default();
        let mono_scripts = self.mono_scripts.as_deref().unwrap_or(&[]);
        let with_symbols = |primary, extra: &[&'static str]| {
            family_list(primary, system, self.sans.as_str())
                .into_iter()
                .chain(extra.iter().chain(self.symbol_families.iter()).chain(math).chain(PLATFORM_FAMILY).map(|name| FontFamilyName::named(name)))
                .collect::<Vec<_>>()
        };
        let sans = match style.family {
            Family::Sans => with_symbols(self.sans.as_str(), &[]),
            Family::Mono => with_symbols(self.mono.as_str(), mono_scripts),
            Family::Named(list) => {
                let mut families = with_symbols(self.sans.as_str(), &[]);
                families.splice(0..0, named_list(list));
                families
            }
            Family::SystemMono => {
                let mut families = with_symbols(self.mono.as_str(), mono_scripts);
                families.splice(0..0, system.iter().map(|name| FontFamilyName::named(name)));
                families
            }
        };
        let mono = with_symbols(self.mono.as_str(), mono_scripts);
        // Routed clusters try their fallback first, then the run's own stack (mono spans included).
        // Each of the four {math, emoji} × {sans, mono} stacks is built at most once.
        let mut stacks: [Option<Vec<FontFamilyName<'_>>>; 4] = Default::default();
        let mut routed = Vec::new();
        for (range, route) in routes {
            let fonts = match route { Route::Math => math, Route::Emoji => emoji };
            if fonts.is_empty() {
                continue;
            }
            let mono_span = spans.iter().any(|s| s.mono && (s.start as usize) < range.end && range.start < s.end as usize);
            let stack = 2 * (route == Route::Emoji) as usize + mono_span as usize;
            stacks[stack].get_or_insert_with(|| {
                let base = if mono_span { &mono } else { &sans };
                fonts.iter().map(|name| FontFamilyName::named(name)).chain(base.iter().cloned()).collect()
            });
            routed.push((range, stack));
        }
        let span_range = |s: &Span| (s.start as usize).min(text.len())..(s.end as usize).min(text.len());
        // `exact` overrides the predicted spacing with each run's own, by byte range.
        // Code never takes ligatures (`->` stays two characters); later features override earlier.
        let off = [b"liga", b"clig", b"calt", b"dlig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 0));
        let unligated: Vec<parley::FontFeature> = features.iter().chain(&off).copied().collect();
        type Spacings = [(std::ops::Range<usize>, f32)];
        let build = |lcx: &mut LayoutContext<Ink>, fcx: &mut FontContext, exact: &Spacings, widen: &Spacings, weights: &Spacings, fits: &Spacings, bold: &[(std::ops::Range<usize>, u16)]| {
            let mut b = lcx.ranged_builder(fcx, text, scale, true);
            b.set_base_direction(base);
            // Lines break where Chromium breaks them: after every run of spaces, even before a
            // closing bracket or comma (UAX #14 LB13 forbids that), and by Blink's ASCII pair
            // table elsewhere, so "number }" wraps before "}" rather than before "number".
            b.set_line_break_override(Some(parley::CHROMIUM_LINE_BREAK_OVERRIDE));
            b.push_default(FontFamily::from(sans.as_slice()));
            b.push_default(StyleProperty::Locale(language));
            b.push_default(StyleProperty::FontSize(style.size));
            b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
            if style.italic {
                b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
            }
            // Parley turns optional ligatures off under letter spacing; Core Text's tracking
            // keeps them, so spacing that only corrects `trak` turns them back on.
            let corrected = base_spacing != 0.0 || span_spacing.iter().flatten().chain(exact.iter().map(|e| &e.1)).any(|&v| v != 0.0);
            if mono_style {
                b.push_default(StyleProperty::FontFeatures(unligated.as_slice().into()));
            } else if style.tracking == 0.0 && corrected {
                let ligatures = [b"liga", b"clig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 1));
                let list = ligatures.iter().chain(&features).copied().collect();
                b.push_default(StyleProperty::FontFeatures(parley::FontFeatures::List(std::borrow::Cow::Owned(list))));
            } else if stripped {
                b.push_default(StyleProperty::FontFeatures(features.as_slice().into()));
            } else if !style.features.is_empty() {
                b.push_default(StyleProperty::FontFeatures(style.features.into()));
            }
            let settings = with_axes(variations(style, style.size), &base_axes);
            if !settings.is_empty() {
                b.push_default(parley::FontVariations::List(std::borrow::Cow::Owned(settings)));
            }
            b.push_default(LineHeight::Absolute(style.line_px()));
            b.push_default(StyleProperty::LineBreak(LINE_BREAK));
            if wrap_anywhere {
                b.push_default(StyleProperty::OverflowWrap(parley::OverflowWrap::Anywhere));
            }
            if base_spacing != 0.0 {
                b.push_default(StyleProperty::LetterSpacing(base_spacing));
            }
            let mut layout_span = 0u16;
            for (i, s) in spans.iter().enumerate() {
                if !(s.start < s.end && s.affects_layout()) {
                    continue;
                }
                layout_span += 1;
                let r = span_range(s);
                if r.is_empty() {
                    continue;
                }
                b.push(StyleProperty::Brush(Ink(layout_span)), r.clone());
                if s.mono {
                    b.push(FontFamily::from(mono.as_slice()), r.clone());
                    b.push(StyleProperty::FontFeatures(unligated.as_slice().into()), r.clone());
                }
                if let Some(w) = s.weight {
                    b.push(StyleProperty::FontWeight(FontWeight::new(w)), r.clone());
                }
                if s.italic {
                    b.push(StyleProperty::FontStyle(FontStyle::Italic), r.clone());
                }
                if let Some(sz) = s.size {
                    b.push(StyleProperty::FontSize(sz), r.clone());
                    // CSS's unitless line-height: a span of its own size (inline code, a smaller run) keeps the
                    // paragraph's leading ratio, so its box fits inside the line instead of growing it
                    b.push(LineHeight::FontSizeRelative(style.line_px() / style.size), r.clone());
                }
                let resized = s.size.is_some() && style.optical_size == OpticalSize::Auto;
                if resized || span_axes[i] != base_axes {
                    let settings = with_axes(variations(style, s.size.unwrap_or(style.size)), &span_axes[i]);
                    b.push(parley::FontVariations::List(std::borrow::Cow::Owned(settings)), r.clone());
                }
                if let Some(v) = span_spacing[i] {
                    b.push(StyleProperty::LetterSpacing(v), r.clone());
                }
            }
            for (range, w) in weights {
                b.push(StyleProperty::FontWeight(FontWeight::new(*w)), range.clone());
            }
            for (range, lh) in fits {
                b.push(LineHeight::Absolute(*lh), range.clone());
            }
            for (range, ink) in bold {
                b.push(StyleProperty::Brush(Ink(*ink)), range.clone());
            }
            for (range, stack) in &routed {
                b.push(FontFamily::from(stacks[*stack].as_deref().unwrap_or_default()), range.clone());
            }
            for (range, v) in exact {
                b.push(StyleProperty::LetterSpacing(*v), range.clone());
            }
            for (range, v) in widen {
                b.push(StyleProperty::WordSpacing(*v), range.clone());
            }
            // Blink widens each glyph in synthetic bold by a pixel (`SyntheticBoldOffset`).
            for (range, _) in bold {
                let at = range.start;
                let v = exact.iter().find(|(r, _)| r.contains(&at)).map(|e| e.1)
                    .or_else(|| spans.iter().zip(&span_spacing).rev().find_map(|(s, v)| v.filter(|_| span_range(s).contains(&at))))
                    .unwrap_or(base_spacing);
                b.push(StyleProperty::LetterSpacing(v + synthetic_bold::ADVANCE), range.clone());
            }
            // Boxes before and after a pill's label make room for its chip. The label may break
            // like any text; `break_lines_with_blocks` moves a split pill whole to the next line
            // and lets it wrap only when it is wider than a line, as a browser wraps a span.
            for (i, r, lead, trail) in &pills {
                for (side, index, width) in [(0, r.start, *lead), (1, r.end, *trail)] {
                    b.push_inline_box(parley::InlineBox {
                        id: PILL_BOX | (*i as u64) << 1 | side,
                        kind: parley::InlineBoxKind::InFlow,
                        index,
                        width: width as f32,
                        height: 0.0,
                        baseline: None,
                        vertical_align: parley::VerticalAlign::BASELINE,
                    });
                }
            }
            let mut layout: Layout<Ink> = b.build(text);
            break_lines(&mut layout);
            layout
        };
        let mut layout = build(&mut self.layout_cx, &mut self.font_cx, &[], &widen, &[], &[], &[]);
        // On Windows, system faces as DirectWrite picks them for each run's weight, as Chromium.
        let system = cfg!(target_os = "windows") && !text.is_ascii();
        let wanted = |at: usize| spans.iter().rev().find_map(|s| s.weight.filter(|_| span_range(s).contains(&at))).unwrap_or(style.weight);
        let weights = if system {
            system_weight::corrections(&mut self.system_faces, &mut self.font_cx, &self.font_files, &layout, wanted)
        } else {
            Vec::new()
        };
        if !weights.is_empty() {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &[], &widen, &weights, &[], &[]);
        }
        // Inline code keeps inside the line box its text sets, as a browser's does.
        let fits = strut.map_or_else(Vec::new, |strut| inline_fit::mono_fits(&layout, text, spans, style, scale, strut));
        // Bold asked of a system face with no bold draws synthetic bold, as Chromium's does.
        let bolds = cfg!(target_os = "windows") && (style.weight >= 600.0 || spans.iter().any(|s| s.weight.is_some_and(|w| w >= 600.0)));
        let bold = if bolds {
            synthetic_bold::inks(&system_weight::synthetic_bold(&mut self.system_faces, &mut self.font_cx, &self.font_files, &layout, wanted), spans)
        } else {
            Vec::new()
        };
        if !fits.is_empty() || !bold.is_empty() {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &[], &widen, &weights, &fits, &bold);
        }
        // The spacing each run got, against the spacing its own font needs.
        let given = |at: usize| {
            spans.iter().zip(&span_spacing).rev().find_map(|(s, v)| v.filter(|_| span_range(s).contains(&at))).unwrap_or(base_spacing)
        };
        let mut exact: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        let mut mismatch = false;
        for line in layout.lines() {
            for run in line.runs() {
                let font = run.font();
                let trak = font_trak(&mut self.traks, &font.data, font.index);
                let needed = spacing(trak, run.font_size() / scale);
                let range = run.text_range();
                if range.is_empty() {
                    continue;
                }
                mismatch |= (needed - given(range.start)).abs() > 1e-4;
                match exact.last_mut() {
                    Some((last, v)) if last.end == range.start && *v == needed => last.end = range.end,
                    _ => exact.push((range, needed)),
                }
            }
        }
        if mismatch {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &exact, &widen, &weights, &fits, &bold);
        }
        layout.align(align, AlignmentOptions::default());
        let l = Rc::new(layout);
        self.layout_ids.build(&l);
        if cache {
            self.layouts.insert(key, Entry::new(l.clone(), g, text.len(), shape_key));
            self.shapes.insert(shape_key, key);
        }
        if let Some(shared) = shared {
            self.layout_cx = shared;
        }
        l
    }
}
