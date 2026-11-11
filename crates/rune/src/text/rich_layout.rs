//! The entry points that lay out rich text: [`TextSystem::layout_rich`] and its aligned,
//! option, block-indented and per-spec variants, all over `layout_blocks_inner`; and the tab
//! stops monospace text widens to.
use super::*;

/// How [`TextSystem::layout_rich_with`] lays out a paragraph. The default is
/// [`layout_rich`](TextSystem::layout_rich): start-aligned, breaking only between words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RichLayoutOptions {
    align: Alignment,
    wrap_anywhere: bool,
    direction: parley::BaseDirection,
}

impl Default for RichLayoutOptions {
    fn default() -> Self {
        RichLayoutOptions { align: Alignment::Start, wrap_anywhere: false, direction: parley::BaseDirection::Auto }
    }
}

impl RichLayoutOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Aligns each line within the wrap width, as [`layout_aligned`](TextSystem::layout_aligned) does.
    pub fn align(mut self, align: Alignment) -> Self {
        self.align = align;
        self
    }

    /// Breaks inside a word that has no break opportunity and is wider than the wrap width
    /// (a long URL, hash or token) instead of letting it run past the line, as
    /// [`El::wrap_anywhere`](crate::El::wrap_anywhere) does. Words that fit still break at spaces.
    pub fn wrap_anywhere(mut self) -> Self {
        self.wrap_anywhere = true;
        self
    }

    /// The paragraph's direction (its bidi base level), as a text element inside
    /// [`El::dir`](crate::El::dir) lays out in: `Rtl` aligns `Start` to the right edge and gives
    /// neutral characters at its ends the right-to-left side, `Auto` takes the first strong
    /// character's. Left to right by default.
    pub fn direction(mut self, direction: crate::Direction) -> Self {
        self.direction = match direction {
            crate::Direction::Ltr => parley::BaseDirection::Ltr,
            crate::Direction::Rtl => parley::BaseDirection::Rtl,
            crate::Direction::Auto => parley::BaseDirection::Auto,
        };
        self
    }

    /// [`Self::direction`] of a resolved flow.
    pub(crate) fn flow(mut self, flow: Option<crate::el::Flow>) -> Self {
        self.direction = flow.map_or(parley::BaseDirection::Auto, crate::el::Flow::base);
        self
    }
}

impl TextSystem {
    /// A layout in device pixels, wrapped at `max_width` device px when given.
    pub fn layout_rich(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
    ) -> Rc<Layout<Ink>> {
        self.layout_aligned(text, spans, style, scale, max_width, Alignment::Start)
    }

    /// [`layout_rich`](Self::layout_rich) with each line aligned within `max_width`, so wrapped
    /// centred or end-aligned text lines up per line rather than as one left-aligned block.
    pub fn layout_aligned(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
    ) -> Rc<Layout<Ink>> {
        self.layout_blocks(text, spans, &[], &[], style, scale, max_width, align)
    }

    /// [`layout_rich`](Self::layout_rich) with [`RichLayoutOptions`]: line alignment, and
    /// wrapping inside a word wider than `max_width`.
    pub fn layout_rich_with(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        options: RichLayoutOptions,
    ) -> Rc<Layout<Ink>> {
        self.layout_keyed(None, text, spans, &[], &[], style, scale, max_width, options.align, options.wrap_anywhere, Keep::Cached, options.direction)
    }

    /// [`layout_aligned`](Self::layout_aligned) with per-paragraph [`Block`](crate::Block)
    /// indentation applied during line breaking, and room around each pill's label for its
    /// chip. A pill never wraps inside; one that starts a line starts its chip at the line's start.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_blocks(
        &mut self,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
    ) -> Rc<Layout<Ink>> {
        self.layout_blocks_inner(text, spans, blocks, pills, style, scale, max_width, align, false, true)
    }

    /// [`layout_blocks`](Self::layout_blocks) with overflow-wrap, and out of the shared cache
    /// when `cache` is false.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_blocks_inner(
        &mut self,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        wrap_anywhere: bool,
        cache: bool,
    ) -> Rc<Layout<Ink>> {
        self.layout_keyed(None, text, spans, blocks, pills, style, scale, max_width, align, wrap_anywhere, if cache { Keep::Cached } else { Keep::Secret }, parley::BaseDirection::Auto)
    }

    /// Secure display runs must not enter the shared layout cache or leave character
    /// scratch in the reusable shaping context. Layouts contain glyphs and byte ranges,
    /// not source strings; only the mounted node owns a secure run's layout.
    pub(crate) fn layout_for_spec(
        &mut self,
        spec: &crate::el::TextSpec,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
    ) -> Rc<Layout<Ink>> {
        let shape = spec.shape_key(scale);
        self.layout_keyed(shape, &spec.content, &spec.spans, &spec.cold.blocks, &spec.cold.pills, spec.style, scale, max_width, align, spec.wrap_anywhere, if spec.redacted { Keep::Secret } else { Keep::Cached }, spec.base())
    }

    /// [`Self::layout_blocks_inner`] of a cut of `spec`'s content, in its direction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_cut(
        &mut self,
        spec: &crate::el::TextSpec,
        text: &str,
        spans: &[Span],
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        cache: bool,
    ) -> Rc<Layout<Ink>> {
        self.layout_keyed(None, text, spans, &spec.cold.blocks, &[], spec.style, scale, max_width, align, spec.wrap_anywhere, if cache { Keep::Cached } else { Keep::Secret }, spec.base())
    }
}

/// Tab stops in monospace text: a tab shapes as a space and widens to the next 4-column stop.
/// The byte of each tab in monospace text (mono spans, or all of it in a mono style) with the
/// columns it fills to reach the next 4-column stop. Columns count from the start of the line
/// or of the monospace run, whichever is later.
pub(super) fn mono_tab_stops(text: &str, spans: &[Span], mono_style: bool) -> Vec<(usize, u32)> {
    let mut regions: Vec<(usize, usize)> = if mono_style {
        vec![(0, text.len())]
    } else {
        let mut r: Vec<_> = spans.iter().filter(|s| s.mono && s.start < s.end).map(|s| (s.start as usize, (s.end as usize).min(text.len()))).collect();
        r.sort_unstable();
        r
    };
    regions.dedup_by(|next, prev| {
        let joined = next.0 <= prev.1;
        if joined {
            prev.1 = prev.1.max(next.1);
        }
        joined
    });
    let mut out = Vec::new();
    for (start, end) in regions {
        let Some(run) = text.get(start..end) else { continue };
        let mut column = 0u32;
        for (i, ch) in run.char_indices() {
            match ch {
                '\n' => column = 0,
                '\t' => {
                    let fill = 4 - column % 4;
                    out.push((start + i, fill));
                    column += fill;
                }
                _ => column += 1,
            }
        }
    }
    out
}

impl TextSystem {
    pub(super) fn mono_tab_widening(
        &mut self,
        tabs: &[(usize, u32)],
        spans: &[Span],
        style: TextStyle,
        scale: f32,
    ) -> Vec<(std::ops::Range<usize>, f32)> {
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let default_family = if mono_style { style.family } else { Family::Mono };
        let mut last_probe: Option<((u32, u32, bool), f32)> = None;
        let mut widen: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        for &(at, columns) in tabs {
            if columns <= 1 {
                continue;
            }
            let (mut sz, mut wt, mut it) = (style.size, style.weight, style.italic);
            for s in spans {
                if (s.start as usize) <= at && at < s.end as usize {
                    if let Some(v) = s.size { sz = v; }
                    if let Some(v) = s.weight { wt = v; }
                    if s.italic { it = true; }
                }
            }
            let probe_key = (sz.to_bits(), wt.to_bits(), it);
            let col_pt = match last_probe {
                Some((k, v)) if k == probe_key => v,
                _ => {
                    let probe = TextStyle { family: default_family, size: sz, weight: wt, italic: it, ..style };
                    let shaped = self.shaped;
                    let l = self.layout_rich(" ", &[], probe, scale, None);
                    self.shaped = shaped;
                    let adv = parley::Cluster::from_byte_index(&l, 0).map_or(0.0, |c| c.advance() / scale);
                    last_probe = Some((probe_key, adv));
                    adv
                }
            };
            if col_pt <= 0.0 {
                continue;
            }
            let add = (columns - 1) as f32 * col_pt;
            match widen.last_mut() {
                Some((r, last_add)) if r.end == at && (*last_add - add).abs() < 1e-4 => r.end = at + 1,
                _ => widen.push((at..at + 1, add)),
            }
        }
        widen
    }
}

#[cfg(test)]
#[path = "rich_layout_tests.rs"]
mod tests;
