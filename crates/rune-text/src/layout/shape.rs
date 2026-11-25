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
    Entry, HIDDEN_SIZE, Ink, Keep, PILL_BOX, PillChip, PillMetrics, SPAN_BOX, Span, SpanBoxRun, TextSystem, Trak, family_list, font_trak, mono_tab_stops,
    named_list, shape_key,
};
use rune_core::tok::{Family, OpticalSize, TextStyle};

use super::span_map::SpanMap;
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
        text_indent: f32,
        pills: &[PillChip],
        span_boxes: &[SpanBoxRun],
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
        // breaking adds the width, alignment, block indents and first-line text_indent. A layout
        // cached at one width or indent is re-broken at another rather than shaped again.
        let shape_key = shape.unwrap_or_else(|| hash::directed(hash::mix_span_boxes(shape_key(text, spans, style, scale, wrap_anywhere, pills), span_boxes), base));
        // Pretty wrapping breaks only wrapped proportional text again.
        let pretty = pretty && max_width.is_some() && !matches!(style.family, Family::Mono | Family::SystemMono);
        let key = hash::break_key(shape_key, max_width, align, has_indented_blocks.then_some(blocks), text_indent) ^ if pretty { 0x7072_6574_7479_0001 } else { 0 };
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
        // A replaced label's box also stands on the baseline: its height and baseline in px.
        let pills: Vec<(usize, std::ops::Range<usize>, u32, u32, Option<(f32, f32)>)> = pills
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let r = (p.start as usize).min(text.len())..(p.end as usize).min(text.len());
                let replace = p.bare.as_ref().and_then(|b| b.replace);
                let (lead, trail) = match (&p.bare, replace) {
                    (_, Some(r)) => (r.width, 0.0),
                    (Some(b), None) => (b.size + b.gap, 0.0),
                    (None, None) => (pm.lead(p.has_lead_visual()), pm.trail()),
                };
                let stands = replace.map(|r| (((r.ascent + r.descent) * scale).round(), (r.ascent * scale).round()));
                (i, r, (lead * scale).round() as u32, (trail * scale).round() as u32, stands)
            })
            .filter(|(_, r, ..)| {
                !r.is_empty() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end) && !text.as_bytes()[r.clone()].contains(&b'\n')
            })
            .collect();
        let padded_boxes: Vec<(usize, std::ops::Range<usize>, u32, u32)> = span_boxes
            .iter()
            .enumerate()
            .filter_map(|(i, sb)| {
                let lead = (sb.style.pad_left() * scale).round() as u32;
                let trail = (sb.style.pad_right() * scale).round() as u32;
                if lead == 0 && trail == 0 {
                    return None;
                }
                let r = (sb.start as usize).min(text.len())..(sb.end as usize).min(text.len());
                (!r.is_empty() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
                    .then_some((i, r, lead, trail))
            })
            .collect();
        let indented = has_indented_blocks || !pills.is_empty() || !padded_boxes.is_empty();
        let break_lines = |layout: &mut Layout<Ink>| line_break::break_new(layout, text, blocks, text_indent, indented, style.size, scale, max_width, pretty);
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
        let has_mono_span = spans.iter().any(|s| s.mono);
        if self.needs_system_mono(style.family, has_mono_span) {
            self.system_mono();
        }
        if let Family::Named(list) = style.family {
            self.load_installed(list);
        }
        let routes = fallback::routes(text);
        let language = language::of_text(text, self.language);
        let mono_spans = if has_mono_span && !text.is_ascii() {
            SpanMap::new(spans.iter().filter(|s| s.mono).map(|s| (s.start as usize..s.end as usize, ())))
        } else {
            SpanMap::empty()
        };
        self.load_fallbacks(text, style, &mono_spans, &routes, language);
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
        let strut = if mono_style || !has_mono_span { None } else { self.primary_extents(style.family, style.weight, style.italic) };
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
        let has_emoji_route = routes.iter().any(|(_, r)| *r == Route::Emoji);
        let preferred_emoji = if has_emoji_route {
            self.preferred_emoji_family_names()
        } else {
            Vec::new()
        };
        let selected_emoji_sans = if has_emoji_route {
            self.selected_color_family_names(style.family, false)
        } else {
            Vec::new()
        };
        let selected_emoji_mono = if has_emoji_route && has_mono_span {
            self.selected_color_family_names(style.family, true)
        } else {
            Vec::new()
        };
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
        let mono = if has_mono_span { with_symbols(self.mono.as_str(), mono_scripts) } else { Vec::new() };
        // Routed clusters try their preferred font(s) first, then the run's own stack (mono spans
        // included). For `Route::Emoji`, any colour emoji family explicitly selected by the app
        // precedes the system emoji fallback so monochrome text fonts still cannot steal emoji.
        // Each of the four {math, emoji} × {sans, mono} stacks is built at most once.
        let mut stacks: [Option<Vec<FontFamilyName<'_>>>; 4] = Default::default();
        let mut routed = Vec::new();
        for (range, route) in routes {
            let mono_span = mono_spans.overlaps(range.clone());
            match route {
                Route::Math => {
                    if math.is_empty() {
                        continue;
                    }
                    let stack = mono_span as usize;
                    stacks[stack].get_or_insert_with(|| {
                        let base = if mono_span { &mono } else { &sans };
                        math.iter().map(|name| FontFamilyName::named(name)).chain(base.iter().cloned()).collect()
                    });
                    routed.push((range, stack));
                }
                Route::Emoji => {
                    let selected = if mono_span { &selected_emoji_mono } else { &selected_emoji_sans };
                    if preferred_emoji.is_empty() && selected.is_empty() && emoji.is_empty() {
                        continue;
                    }
                    let stack = 2 + mono_span as usize;
                    stacks[stack].get_or_insert_with(|| {
                        let base = if mono_span { &mono } else { &sans };
                        preferred_emoji
                            .iter()
                            .map(|name| FontFamilyName::named(name.as_str()))
                            .chain(
                                selected
                                    .iter()
                                    .filter(|name| !preferred_emoji.iter().any(|p| p == *name))
                                    .map(|name| FontFamilyName::named(name.as_str())),
                            )
                            .chain(
                                emoji
                                    .iter()
                                    .filter(|name| !preferred_emoji.iter().any(|p| p == **name))
                                    .map(|name| FontFamilyName::named(name)),
                            )
                            .chain(base.iter().cloned())
                            .collect()
                    });
                    routed.push((range, stack));
                }
            }
        }
        let span_range = |s: &Span| (s.start as usize).min(text.len())..(s.end as usize).min(text.len());
        let given_map = SpanMap::new(spans.iter().zip(&span_spacing).filter_map(|(s, &v)| v.map(|v| (span_range(s), v))));
        // `exact` overrides the predicted spacing with each run's own, by byte range.
        // Code never takes ligatures (`->` stays two characters); later features override earlier.
        let off = [b"liga", b"clig", b"calt", b"dlig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 0));
        let unligated: Vec<parley::FontFeature> = features.iter().chain(&off).copied().collect();
        type Spacings = [(std::ops::Range<usize>, f32)];
        let build = |lcx: &mut LayoutContext<Ink>, fcx: &mut FontContext, exact: &Spacings, widen: &Spacings, weights: &Spacings, fits: &Spacings, bold: &[(std::ops::Range<usize>, u32)]| {
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
            let mut layout_span = 0u32;
            for (i, s) in spans.iter().enumerate() {
                if !(s.start < s.end && s.affects_layout()) {
                    continue;
                }
                layout_span = layout_span.saturating_add(1);
                let r = span_range(s);
                if r.is_empty() {
                    continue;
                }
                let brush = if layout_span < synthetic_bold::SYNTHETIC_BOLD { layout_span } else { 0 };
                b.push(StyleProperty::Brush(Ink(brush)), r.clone());
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
            let exact_map = if bold.is_empty() || exact.is_empty() {
                SpanMap::empty()
            } else {
                SpanMap::new(exact.iter().map(|(r, v)| (r.clone(), *v)))
            };
            for (range, _) in bold {
                let at = range.start;
                let v = exact_map.get(at).or_else(|| given_map.get(at)).unwrap_or(base_spacing);
                b.push(StyleProperty::LetterSpacing(v + synthetic_bold::ADVANCE), range.clone());
            }
            // Boxes before and after a pill's label make room for its chip. The label may break
            // like any text; `break_lines_with_blocks` moves a split pill whole to the next line
            // and lets it wrap only when it is wider than a line, as a browser wraps a span.
            // A replaced label shapes so small it takes no room and draws nothing (`glyph_emit`
            // skips it), and its box before it stands as tall as its image.
            for (i, r, lead, trail, stands) in &pills {
                if stands.is_some() {
                    b.push(StyleProperty::FontSize(HIDDEN_SIZE / scale), r.clone());
                    b.push(LineHeight::Absolute(0.0), r.clone());
                    b.push(StyleProperty::LetterSpacing(0.0), r.clone());
                    b.push(StyleProperty::WordSpacing(0.0), r.clone());
                }
                for (side, index, width) in [(0, r.start, *lead), (1, r.end, *trail)] {
                    let (height, baseline) = stands.filter(|_| side == 0).map_or((0.0, None), |(h, a)| (h, Some(a)));
                    b.push_inline_box(parley::InlineBox {
                        id: PILL_BOX | (*i as u64) << 1 | side,
                        kind: parley::InlineBoxKind::InFlow,
                        index,
                        width: width as f32,
                        height,
                        baseline,
                        vertical_align: parley::VerticalAlign::BASELINE,
                    });
                }
            }
            // Horizontal spacers for padded `SpanBox` ranges reserve room in the line
            // without changing UTF-8 byte offsets.
            for (i, r, lead, trail) in &padded_boxes {
                for (side, index, width) in [(0, r.start, *lead), (1, r.end, *trail)] {
                    if width == 0 {
                        continue;
                    }
                    b.push_inline_box(parley::InlineBox {
                        id: SPAN_BOX | (*i as u64) << 1 | side,
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
        let system = cfg!(target_os = "windows") && !self.font_files.is_empty();
        let weight_map = if system {
            SpanMap::new(spans.iter().filter_map(|s| s.weight.map(|w| (span_range(s), w))))
        } else {
            SpanMap::empty()
        };
        let wanted = |at: usize| weight_map.get(at).unwrap_or(style.weight);
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
        let bolds = system && (style.weight >= 600.0 || spans.iter().any(|s| s.weight.is_some_and(|w| w >= 600.0)));
        let bold = if bolds {
            synthetic_bold::inks(&system_weight::synthetic_bold(&mut self.system_faces, &mut self.font_cx, &self.font_files, &layout, wanted), spans)
        } else {
            Vec::new()
        };
        if !fits.is_empty() || !bold.is_empty() {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &[], &widen, &weights, &fits, &bold);
        }
        // The spacing each run got, against the spacing its own font needs.
        let given = |at: usize| given_map.get(at).unwrap_or(base_spacing);
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
