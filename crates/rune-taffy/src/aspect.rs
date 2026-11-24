//! CSS Sizing 4 / Flexbox aspect-ratio transfer after min/max clamping.
//!
//! Upstream Taffy 0.15 applies `aspect_ratio` to resolved `size()` before clamping by
//! `min_size`/`max_size` (both in `generate_anonymous_flex_items` and in `compute_leaf_layout`),
//! and transfers `min_size`/`max_size` even when both axes (or `flex_basis` and the cross axis)
//! are definite. These wrappers pre-clamp the ratio-determining axis and resolve the transferred
//! dimension before Taffy sees `size()`, with zero extra work for elements without `aspect_ratio`.

#![forbid(unsafe_code)]

use taffy::geometry::{Point, Rect, Size};
use taffy::geometry::Line;
use taffy::style::{
    AlignContent, AlignItems, AlignSelf, AvailableSpace, BlockContainerStyle, BoxGenerationMode, BoxSizing, Contain,
    CoreStyle, Dimension, Direction, ExpandedDimension, ExpandedLengthPercentageAuto, FlexDirection, FlexWrap,
    FlexboxContainerStyle, GridAutoFlow, GridContainerStyle, GridItemStyle, GridPlacement, JustifyContent,
    LengthPercentage, LengthPercentageAuto, OofItemStyle, Overflow, Position, TextAlign,
};
use taffy::tree::{LayoutInput, LayoutOutput, NodeId, SizingMode};
use taffy::util::{MaybeMath, MaybeResolve, ResolveOrZero};

use crate::style::CompactStyle;

/// Active container context saved on `LayoutStoreView` while Taffy solves a container node.
#[derive(Clone, Copy)]
pub(crate) struct ContainerCtx {
    pub(crate) node_id: NodeId,
    pub(crate) known_dimensions: Size<Option<f32>>,
    pub(crate) known_dimensions_are_definite: Size<bool>,
    pub(crate) parent_size: Size<Option<f32>>,
    pub(crate) sizing_mode: SizingMode,
}

impl ContainerCtx {
    #[inline(always)]
    pub(crate) fn new(node_id: NodeId, inputs: &LayoutInput) -> Self {
        Self {
            node_id,
            known_dimensions: inputs.known_dimensions,
            known_dimensions_are_definite: inputs.known_dimensions_are_definite,
            parent_size: inputs.parent_size,
            sizing_mode: inputs.sizing_mode,
        }
    }
}

/// Overridden `size` and `aspect_ratio` for a leaf or container under `SizingMode::InherentSize`.
#[derive(Clone, Copy)]
pub(crate) struct AspectOverride {
    pub(crate) size: Size<Dimension>,
    pub(crate) aspect_ratio: Option<f32>,
}

/// Overridden flex-item fields for a flex child with `aspect_ratio`.
#[derive(Clone, Copy)]
pub(crate) struct FlexAspectOverride {
    pub(crate) size: Size<Dimension>,
    pub(crate) min_size: Size<LengthPercentageAuto>,
    pub(crate) flex_basis: Dimension,
    pub(crate) aspect_ratio: Option<f32>,
}

#[inline(always)]
fn no_calc(_val: *const (), _basis: f32) -> f32 {
    0.0
}

#[inline(always)]
fn is_row(dir: FlexDirection) -> bool {
    matches!(dir, FlexDirection::Row | FlexDirection::RowReverse)
}

#[inline(always)]
fn main_of<T>(s: Size<T>, dir: FlexDirection) -> T {
    if is_row(dir) { s.width } else { s.height }
}

#[inline(always)]
fn cross_of<T>(s: Size<T>, dir: FlexDirection) -> T {
    if is_row(dir) { s.height } else { s.width }
}

#[inline(always)]
fn set_main<T>(s: &mut Size<T>, dir: FlexDirection, val: T) {
    if is_row(dir) {
        s.width = val;
    } else {
        s.height = val;
    }
}

#[inline(always)]
fn with_main<T>(mut s: Size<T>, dir: FlexDirection, val: T) -> Size<T> {
    set_main(&mut s, dir, val);
    s
}

#[inline(always)]
fn apply_aspect(s: Size<Option<f32>>, ratio: Option<f32>) -> Size<Option<f32>> {
    match ratio {
        Some(r) => match (s.width, s.height) {
            (Some(w), None) => Size { width: Some(w), height: Some(w / r) },
            (None, Some(h)) => Size { width: Some(h * r), height: Some(h) },
            _ => s,
        },
        None => s,
    }
}

#[inline(always)]
fn round_dim(d: Dimension) -> Dimension {
    match d.expand() {
        ExpandedDimension::Length(h) => Dimension::length((h + 0.5).floor()),
        _ => d,
    }
}

#[inline(always)]
fn round_lpa(d: LengthPercentageAuto) -> LengthPercentageAuto {
    match d.expand() {
        ExpandedLengthPercentageAuto::Length(h) => LengthPercentageAuto::length((h + 0.5).floor()),
        _ => d,
    }
}

#[inline]
fn padding_border_sum(style: &CompactStyle, parent_width: Option<f32>) -> Size<f32> {
    let padding = style.padding().resolve_or_zero(parent_width, no_calc);
    let border = style.border().resolve_or_zero(parent_width, no_calc);
    (padding + border).sum_axes()
}

/// Resolves `aspect_ratio` for a standalone leaf or container under `SizingMode::InherentSize`.
/// Returns `None` when the raw style needs no adjustment.
#[inline]
pub(crate) fn container_aspect_override(
    style: &CompactStyle,
    parent_size: Size<Option<f32>>,
    known_dimensions: Size<Option<f32>>,
    sizing_mode: SizingMode,
) -> Option<AspectOverride> {
    if sizing_mode != SizingMode::InherentSize {
        return None;
    }
    let ratio = style.aspect_ratio()?;
    let raw_size = style.size();
    let res_w = raw_size.width.maybe_resolve(parent_size.width, no_calc);
    let res_h = raw_size.height.maybe_resolve(parent_size.height, no_calc);
    let eff_w = known_dimensions.width.or(res_w);
    let eff_h = known_dimensions.height.or(res_h);

    match (eff_w, eff_h) {
        (Some(_), Some(_)) => Some(AspectOverride { size: raw_size, aspect_ratio: None }),
        (None, None) => None,
        (Some(_), None) => {
            let w = res_w?;
            let min_w = style.min_size().width.maybe_resolve(parent_size.width, no_calc);
            let max_w = style.max_size().width.maybe_resolve(parent_size.width, no_calc);
            let floor_w = if style.box_sizing() == BoxSizing::BorderBox {
                padding_border_sum(style, parent_size.width).width
            } else {
                0.0
            };
            let cw = w.maybe_clamp(min_w, max_w).max(floor_w);
            Some(AspectOverride {
                size: Size { width: raw_size.width, height: Dimension::length(cw / ratio) },
                aspect_ratio: None,
            })
        }
        (None, Some(_)) => {
            let h = res_h?;
            let min_h = style.min_size().height.maybe_resolve(parent_size.height, no_calc);
            let max_h = style.max_size().height.maybe_resolve(parent_size.height, no_calc);
            let floor_h = if style.box_sizing() == BoxSizing::BorderBox {
                padding_border_sum(style, parent_size.width).height
            } else {
                0.0
            };
            let ch = h.maybe_clamp(min_h, max_h).max(floor_h);
            Some(AspectOverride {
                size: Size { width: Dimension::length(ch * ratio), height: raw_size.height },
                aspect_ratio: None,
            })
        }
    }
}

/// Computes a flex container's `(dir, node_inner_size, percent_resolution_size)`, mirroring
/// Taffy 0.15's `compute_flexbox_layout` + `compute_constants`. Called only when a flex item
/// actually has `aspect_ratio: Some(_)`.
#[inline]
fn flex_parent_resolution(
    style: &CompactStyle,
    ctx: ContainerCtx,
) -> (FlexDirection, Size<Option<f32>>, Size<Option<f32>>) {
    let dir = style.flex_direction();
    let parent_size = ctx.parent_size;
    let padding = style.padding().resolve_or_zero(parent_size.width, no_calc);
    let border = style.border().resolve_or_zero(parent_size.width, no_calc);
    let padding_border_sum = padding.sum_axes() + border.sum_axes();
    let box_sizing_adjustment =
        if style.box_sizing() == BoxSizing::ContentBox { padding_border_sum } else { Size::ZERO };

    let ov = container_aspect_override(style, parent_size, ctx.known_dimensions, ctx.sizing_mode);
    let eff_size = ov.map_or_else(|| style.size(), |o| o.size);
    let eff_ratio = ov.map_or_else(|| style.aspect_ratio(), |o| o.aspect_ratio);

    let min_size = apply_aspect(style.min_size().maybe_resolve(parent_size, no_calc), eff_ratio)
        .maybe_add(box_sizing_adjustment);
    let max_size = apply_aspect(style.max_size().maybe_resolve(parent_size, no_calc), eff_ratio)
        .maybe_add(box_sizing_adjustment);
    let clamped_style_size = if ctx.sizing_mode == SizingMode::InherentSize {
        apply_aspect(eff_size.maybe_resolve(parent_size, no_calc), eff_ratio)
            .maybe_add(box_sizing_adjustment)
            .maybe_clamp(min_size, max_size)
    } else {
        Size::NONE
    };
    let min_max_definite_size = min_size.zip_map(max_size, |min, max| match (min, max) {
        (Some(min), Some(max)) if max <= min => Some(min),
        _ => None,
    });
    let node_outer_size = ctx
        .known_dimensions
        .or(min_max_definite_size.or(clamped_style_size).maybe_max(padding_border_sum));

    let scrollbar_gutter = style.overflow().transpose().map(|overflow| match overflow {
        Overflow::Scroll => style.scrollbar_width(),
        _ => 0.0,
    });
    let mut content_box_inset = padding + border;
    content_box_inset.bottom += scrollbar_gutter.y;
    match style.direction() {
        Direction::Ltr => content_box_inset.right += scrollbar_gutter.x,
        Direction::Rtl => content_box_inset.left += scrollbar_gutter.x,
    }
    let node_inner_size = node_outer_size.maybe_sub(content_box_inset.sum_axes());

    let known_dimensions_are_definite = ctx
        .known_dimensions_are_definite
        .zip_map(ctx.known_dimensions, |is_def, kd| is_def || kd.is_none());
    let known_main_size_is_definite = main_of(known_dimensions_are_definite, dir);
    let percent_resolution_size = if known_main_size_is_definite {
        node_inner_size
    } else {
        with_main(node_inner_size, dir, None)
    };
    (dir, node_inner_size, percent_resolution_size)
}

/// Computes the CSS-compliant aspect-ratio override for a flex child.
/// Returns `None` immediately when `child_style.aspect_ratio()` is `None`.
#[inline]
pub(crate) fn flex_child_aspect_override(
    parent_style: &CompactStyle,
    parent_ctx: ContainerCtx,
    child_style: &CompactStyle,
    round_height: bool,
) -> Option<FlexAspectOverride> {
    let ratio = child_style.aspect_ratio()?;
    if child_style.position().is_out_of_flow() || child_style.box_generation_mode() == BoxGenerationMode::None {
        return None;
    }
    let (dir, node_inner_size, percent_resolution_size) = flex_parent_resolution(parent_style, parent_ctx);
    let row = is_row(dir);

    let mut raw_size = child_style.size();
    let mut raw_min = child_style.min_size();
    let mut raw_max = child_style.max_size();
    let mut raw_fb = child_style.flex_basis();
    if round_height {
        raw_size.height = round_dim(raw_size.height);
        raw_min.height = round_lpa(raw_min.height);
        raw_max.height = round_lpa(raw_max.height);
        raw_fb = round_dim(raw_fb);
    }

    let res_main = main_of(raw_size, dir).maybe_resolve(main_of(percent_resolution_size, dir), no_calc);
    let res_cross = cross_of(raw_size, dir).maybe_resolve(cross_of(percent_resolution_size, dir), no_calc);

    match (res_main, res_cross) {
        (Some(_), Some(_)) => Some(FlexAspectOverride {
            size: raw_size,
            min_size: raw_min,
            flex_basis: raw_fb,
            aspect_ratio: None,
        }),
        (None, None) => None,
        (None, Some(cross_val)) => {
            let min_cross = cross_of(raw_min, dir).maybe_resolve(cross_of(percent_resolution_size, dir), no_calc);
            let max_cross = cross_of(raw_max, dir).maybe_resolve(cross_of(percent_resolution_size, dir), no_calc);
            let floor_cross = if child_style.box_sizing() == BoxSizing::BorderBox {
                cross_of(padding_border_sum(child_style, node_inner_size.width), dir)
            } else {
                0.0
            };
            let clamped_cross = cross_val.maybe_clamp(min_cross, max_cross).max(floor_cross);
            let mut transferred_main = if row { clamped_cross * ratio } else { clamped_cross / ratio };

            let fb_resolved = raw_fb.maybe_resolve(main_of(percent_resolution_size, dir), no_calc);
            let fb_exp = raw_fb.expand();
            let fb_is_definite = fb_resolved.is_some() || matches!(fb_exp, ExpandedDimension::Stretch);
            let max_main = main_of(raw_max, dir).maybe_resolve(main_of(percent_resolution_size, dir), no_calc);

            if fb_is_definite {
                let mut min_size = raw_min;
                let overflow_main = if row { child_style.overflow().x } else { child_style.overflow().y };
                if main_of(raw_min, dir).is_auto() && matches!(overflow_main, Overflow::Visible | Overflow::Clip) {
                    let auto_min = transferred_main.maybe_min(max_main);
                    let d = LengthPercentageAuto::length(auto_min);
                    let d = if round_height && !row { round_lpa(d) } else { d };
                    set_main(&mut min_size, dir, d);
                }
                return Some(FlexAspectOverride {
                    size: raw_size,
                    min_size,
                    flex_basis: raw_fb,
                    aspect_ratio: None,
                });
            }

            if child_style.flex_grow() == 0.0 && child_style.flex_shrink() == 0.0 {
                let min_main = main_of(raw_min, dir).maybe_resolve(main_of(percent_resolution_size, dir), no_calc);
                transferred_main = transferred_main.maybe_clamp(min_main, max_main);
            }
            let mut size = raw_size;
            let main_dim = Dimension::length(transferred_main);
            let main_dim = if round_height && !row { round_dim(main_dim) } else { main_dim };
            set_main(&mut size, dir, main_dim);

            // An unresolvable percentage flex-basis (or `content`) behaves as `content` per
            // CSS Flexbox §9.2.3.B, so Taffy should use the transferred main size as flex base size.
            let flex_basis = match fb_exp {
                ExpandedDimension::Percent(_) | ExpandedDimension::Content => Dimension::auto(),
                _ => raw_fb,
            };

            Some(FlexAspectOverride {
                size,
                min_size: raw_min,
                flex_basis,
                aspect_ratio: None,
            })
        }
        (Some(main_val), None) => {
            if child_style.flex_grow() != 0.0 || child_style.flex_shrink() != 0.0 {
                return None;
            }
            let min_main = main_of(raw_min, dir).maybe_resolve(main_of(percent_resolution_size, dir), no_calc);
            let max_main = main_of(raw_max, dir).maybe_resolve(main_of(percent_resolution_size, dir), no_calc);
            let floor_main = if child_style.box_sizing() == BoxSizing::BorderBox {
                main_of(padding_border_sum(child_style, node_inner_size.width), dir)
            } else {
                0.0
            };
            let clamped_main = main_val.maybe_clamp(min_main, max_main).max(floor_main);
            if (clamped_main - main_val).abs() == 0.0 {
                return None;
            }
            // Keep the cross dimension `Auto` so `determine_used_cross_size` still stretches
            // `align-self: stretch` items across their flex line.
            let mut size = raw_size;
            let main_dim = Dimension::length(clamped_main);
            let main_dim = if round_height && !row { round_dim(main_dim) } else { main_dim };
            set_main(&mut size, dir, main_dim);
            Some(FlexAspectOverride {
                size,
                min_size: raw_min,
                flex_basis: raw_fb,
                aspect_ratio: Some(ratio),
            })
        }
    }
}

/// Thin style wrapper overriding `size` and `aspect_ratio` for leaves and containers.
#[derive(Clone, Copy)]
pub(crate) struct AspectStyle<'a> {
    pub(crate) style: &'a CompactStyle,
    pub(crate) aspect: Option<AspectOverride>,
}

impl CoreStyle for AspectStyle<'_> {
    type CustomIdent = <CompactStyle as CoreStyle>::CustomIdent;
    #[inline(always)]
    fn box_generation_mode(&self) -> BoxGenerationMode { self.style.box_generation_mode() }
    #[inline(always)]
    fn is_block(&self) -> bool { self.style.is_block() }
    #[inline(always)]
    fn is_compressible_replaced(&self) -> bool { self.style.is_compressible_replaced() }
    #[inline(always)]
    fn is_replaced(&self) -> bool { self.style.is_replaced() }
    #[inline(always)]
    fn box_sizing(&self) -> BoxSizing { self.style.box_sizing() }
    #[inline(always)]
    fn direction(&self) -> Direction { self.style.direction() }
    #[inline(always)]
    fn overflow(&self) -> Point<Overflow> { self.style.overflow() }
    #[inline(always)]
    fn scrollbar_width(&self) -> f32 { self.style.scrollbar_width() }
    #[inline(always)]
    fn position(&self) -> Position { self.style.position() }
    #[inline(always)]
    fn inset(&self) -> Rect<LengthPercentageAuto> { self.style.inset() }
    #[inline(always)]
    fn size(&self) -> Size<Dimension> {
        match self.aspect {
            Some(a) => a.size,
            None => self.style.size(),
        }
    }
    #[inline(always)]
    fn min_size(&self) -> Size<LengthPercentageAuto> { self.style.min_size() }
    #[inline(always)]
    fn max_size(&self) -> Size<LengthPercentageAuto> { self.style.max_size() }
    #[inline(always)]
    fn aspect_ratio(&self) -> Option<f32> {
        match self.aspect {
            Some(a) => a.aspect_ratio,
            None => self.style.aspect_ratio(),
        }
    }
    #[inline(always)]
    fn margin(&self) -> Rect<LengthPercentageAuto> { self.style.margin() }
    #[inline(always)]
    fn padding(&self) -> Rect<LengthPercentage> { self.style.padding() }
    #[inline(always)]
    fn border(&self) -> Rect<LengthPercentage> { self.style.border() }
    #[inline(always)]
    fn contain(&self) -> Contain { self.style.contain() }
}

impl FlexboxContainerStyle for AspectStyle<'_> {
    #[inline(always)]
    fn flex_direction(&self) -> FlexDirection { self.style.flex_direction() }
    #[inline(always)]
    fn flex_wrap(&self) -> FlexWrap { self.style.flex_wrap() }
    #[inline(always)]
    fn gap(&self) -> Size<LengthPercentage> { FlexboxContainerStyle::gap(self.style) }
    #[inline(always)]
    fn align_content(&self) -> AlignContent { FlexboxContainerStyle::align_content(self.style) }
    #[inline(always)]
    fn align_items(&self) -> AlignItems { FlexboxContainerStyle::align_items(self.style) }
    #[inline(always)]
    fn justify_content(&self) -> JustifyContent { FlexboxContainerStyle::justify_content(self.style) }
}

impl BlockContainerStyle for AspectStyle<'_> {
    #[inline(always)]
    fn text_align(&self) -> TextAlign { self.style.text_align() }
    #[inline(always)]
    fn align_content(&self) -> AlignContent { BlockContainerStyle::align_content(self.style) }
    #[inline(always)]
    fn justify_items(&self) -> AlignItems { BlockContainerStyle::justify_items(self.style) }
}

impl GridContainerStyle for AspectStyle<'_> {
    type Repetition<'a>
        = <CompactStyle as GridContainerStyle>::Repetition<'a>
    where
        Self: 'a;
    type TemplateTrackList<'a>
        = <CompactStyle as GridContainerStyle>::TemplateTrackList<'a>
    where
        Self: 'a;
    type AutoTrackList<'a>
        = <CompactStyle as GridContainerStyle>::AutoTrackList<'a>
    where
        Self: 'a;
    type TemplateLineNames<'a>
        = <CompactStyle as GridContainerStyle>::TemplateLineNames<'a>
    where
        Self: 'a;
    type GridTemplateAreas<'a>
        = <CompactStyle as GridContainerStyle>::GridTemplateAreas<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn grid_template_rows(&self) -> Option<Self::TemplateTrackList<'_>> { self.style.grid_template_rows() }
    #[inline(always)]
    fn grid_template_columns(&self) -> Option<Self::TemplateTrackList<'_>> { self.style.grid_template_columns() }
    #[inline(always)]
    fn grid_auto_rows(&self) -> Self::AutoTrackList<'_> { self.style.grid_auto_rows() }
    #[inline(always)]
    fn grid_auto_columns(&self) -> Self::AutoTrackList<'_> { self.style.grid_auto_columns() }
    #[inline(always)]
    fn grid_auto_flow(&self) -> GridAutoFlow { self.style.grid_auto_flow() }
    #[inline(always)]
    fn gap(&self) -> Size<LengthPercentage> { GridContainerStyle::gap(self.style) }
    #[inline(always)]
    fn align_content(&self) -> AlignContent { GridContainerStyle::align_content(self.style) }
    #[inline(always)]
    fn justify_content(&self) -> JustifyContent { GridContainerStyle::justify_content(self.style) }
    #[inline(always)]
    fn align_items(&self) -> AlignItems { GridContainerStyle::align_items(self.style) }
    #[inline(always)]
    fn justify_items(&self) -> AlignItems { GridContainerStyle::justify_items(self.style) }
    #[inline(always)]
    fn grid_template_areas(&self) -> Option<Self::GridTemplateAreas<'_>> { self.style.grid_template_areas() }
    #[inline(always)]
    fn grid_template_area_row_count(&self) -> u16 { self.style.grid_template_area_row_count() }
    #[inline(always)]
    fn grid_template_area_column_count(&self) -> u16 { self.style.grid_template_area_column_count() }
    #[inline(always)]
    fn grid_template_column_names(&self) -> Option<Self::TemplateLineNames<'_>> { self.style.grid_template_column_names() }
    #[inline(always)]
    fn grid_template_row_names(&self) -> Option<Self::TemplateLineNames<'_>> { self.style.grid_template_row_names() }
}

/// Overridden grid-item fields for a grid child with `aspect_ratio`.
#[derive(Clone, Copy)]
pub(crate) struct GridAspectOverride {
    pub(crate) size: Size<Dimension>,
    pub(crate) min_size: Size<LengthPercentageAuto>,
    pub(crate) max_size: Size<LengthPercentageAuto>,
    pub(crate) aspect_ratio: Option<f32>,
}

/// Computes the CSS-compliant aspect-ratio override for a grid item.
/// Returns `None` immediately when `child_style.aspect_ratio()` is `None`.
#[inline]
pub(crate) fn grid_child_aspect_override(child_style: &CompactStyle) -> Option<GridAspectOverride> {
    let ratio = child_style.aspect_ratio()?;
    if child_style.position().is_out_of_flow() || child_style.box_generation_mode() == BoxGenerationMode::None {
        return None;
    }
    let raw_size = child_style.size();
    let raw_min = child_style.min_size();
    let raw_max = child_style.max_size();
    let w_exp = raw_size.width.expand();
    let h_exp = raw_size.height.expand();
    let w_def = matches!(w_exp, ExpandedDimension::Length(_) | ExpandedDimension::Percent(_));
    let h_def = matches!(h_exp, ExpandedDimension::Length(_) | ExpandedDimension::Percent(_));

    match (w_def, h_def) {
        (true, true) => Some(GridAspectOverride {
            size: raw_size,
            min_size: raw_min,
            max_size: raw_max,
            aspect_ratio: None,
        }),
        (false, false) => None,
        (true, false) => {
            let pb_w = if child_style.box_sizing() == BoxSizing::BorderBox {
                padding_border_sum(child_style, None).width
            } else {
                0.0
            };
            let min_w = raw_min.width.maybe_resolve(None, no_calc);
            let max_w = raw_max.width.maybe_resolve(None, no_calc);
            if let ExpandedDimension::Length(w) = w_exp {
                if (raw_min.width.is_auto() || min_w.is_some()) && (raw_max.width.is_auto() || max_w.is_some()) {
                    let cw = w.maybe_clamp(min_w, max_w).max(pb_w);
                    return Some(GridAspectOverride {
                        size: Size { width: raw_size.width, height: Dimension::length(cw / ratio) },
                        min_size: raw_min,
                        max_size: raw_max,
                        aspect_ratio: None,
                    });
                }
            }
            // Percentage width with length limits: transfer the clamped cross limit into the auto
            // axis so Taffy's `GridItem::minimum_contribution` and `known_dimensions` clamp height
            // even when `max_size.height` is auto or larger than `max_w / ratio`.
            let mut min_size = raw_min;
            let mut max_size = raw_max;
            let mut changed = false;
            if let Some(mw) = max_w {
                let effective_max_w = mw.maybe_max(min_w).max(pb_w);
                let transferred_max_h = effective_max_w / ratio;
                match raw_max.height.expand() {
                    ExpandedLengthPercentageAuto::Auto => {
                        max_size.height = LengthPercentageAuto::length(transferred_max_h);
                        changed = true;
                    }
                    ExpandedLengthPercentageAuto::Length(existing) if transferred_max_h < existing => {
                        max_size.height = LengthPercentageAuto::length(transferred_max_h);
                        changed = true;
                    }
                    _ => {}
                }
            }
            if let Some(mw) = min_w {
                let transferred_min_h = mw.max(pb_w) / ratio;
                match raw_min.height.expand() {
                    ExpandedLengthPercentageAuto::Auto => {
                        min_size.height = LengthPercentageAuto::length(transferred_min_h);
                        changed = true;
                    }
                    ExpandedLengthPercentageAuto::Length(existing) if transferred_min_h > existing => {
                        min_size.height = LengthPercentageAuto::length(transferred_min_h);
                        changed = true;
                    }
                    _ => {}
                }
            }
            changed.then_some(GridAspectOverride {
                size: raw_size,
                min_size,
                max_size,
                aspect_ratio: Some(ratio),
            })
        }
        (false, true) => {
            let pb_h = if child_style.box_sizing() == BoxSizing::BorderBox {
                padding_border_sum(child_style, None).height
            } else {
                0.0
            };
            let min_h = raw_min.height.maybe_resolve(None, no_calc);
            let max_h = raw_max.height.maybe_resolve(None, no_calc);
            if let ExpandedDimension::Length(h) = h_exp {
                if (raw_min.height.is_auto() || min_h.is_some()) && (raw_max.height.is_auto() || max_h.is_some()) {
                    let ch = h.maybe_clamp(min_h, max_h).max(pb_h);
                    return Some(GridAspectOverride {
                        size: Size { width: Dimension::length(ch * ratio), height: raw_size.height },
                        min_size: raw_min,
                        max_size: raw_max,
                        aspect_ratio: None,
                    });
                }
            }
            let mut min_size = raw_min;
            let mut max_size = raw_max;
            let mut changed = false;
            if let Some(mh) = max_h {
                let effective_max_h = mh.maybe_max(min_h).max(pb_h);
                let transferred_max_w = effective_max_h * ratio;
                match raw_max.width.expand() {
                    ExpandedLengthPercentageAuto::Auto => {
                        max_size.width = LengthPercentageAuto::length(transferred_max_w);
                        changed = true;
                    }
                    ExpandedLengthPercentageAuto::Length(existing) if transferred_max_w < existing => {
                        max_size.width = LengthPercentageAuto::length(transferred_max_w);
                        changed = true;
                    }
                    _ => {}
                }
            }
            if let Some(mh) = min_h {
                let transferred_min_w = mh.max(pb_h) * ratio;
                match raw_min.width.expand() {
                    ExpandedLengthPercentageAuto::Auto => {
                        min_size.width = LengthPercentageAuto::length(transferred_min_w);
                        changed = true;
                    }
                    ExpandedLengthPercentageAuto::Length(existing) if transferred_min_w > existing => {
                        min_size.width = LengthPercentageAuto::length(transferred_min_w);
                        changed = true;
                    }
                    _ => {}
                }
            }
            changed.then_some(GridAspectOverride {
                size: raw_size,
                min_size,
                max_size,
                aspect_ratio: Some(ratio),
            })
        }
    }
}

/// Thin style wrapper overriding grid-item sizing fields for grid children with `aspect_ratio`.
#[derive(Clone, Copy)]
pub(crate) struct GridChildStyle<'a> {
    pub(crate) style: &'a CompactStyle,
    pub(crate) aspect: Option<GridAspectOverride>,
}

impl CoreStyle for GridChildStyle<'_> {
    type CustomIdent = <CompactStyle as CoreStyle>::CustomIdent;
    #[inline(always)]
    fn box_generation_mode(&self) -> BoxGenerationMode { self.style.box_generation_mode() }
    #[inline(always)]
    fn is_block(&self) -> bool { self.style.is_block() }
    #[inline(always)]
    fn is_compressible_replaced(&self) -> bool { self.style.is_compressible_replaced() }
    #[inline(always)]
    fn is_replaced(&self) -> bool { self.style.is_replaced() }
    #[inline(always)]
    fn box_sizing(&self) -> BoxSizing { self.style.box_sizing() }
    #[inline(always)]
    fn direction(&self) -> Direction { self.style.direction() }
    #[inline(always)]
    fn overflow(&self) -> Point<Overflow> { self.style.overflow() }
    #[inline(always)]
    fn scrollbar_width(&self) -> f32 { self.style.scrollbar_width() }
    #[inline(always)]
    fn position(&self) -> Position { self.style.position() }
    #[inline(always)]
    fn inset(&self) -> Rect<LengthPercentageAuto> { self.style.inset() }
    #[inline(always)]
    fn size(&self) -> Size<Dimension> {
        match self.aspect {
            Some(a) => a.size,
            None => self.style.size(),
        }
    }
    #[inline(always)]
    fn min_size(&self) -> Size<LengthPercentageAuto> {
        match self.aspect {
            Some(a) => a.min_size,
            None => self.style.min_size(),
        }
    }
    #[inline(always)]
    fn max_size(&self) -> Size<LengthPercentageAuto> {
        match self.aspect {
            Some(a) => a.max_size,
            None => self.style.max_size(),
        }
    }
    #[inline(always)]
    fn aspect_ratio(&self) -> Option<f32> {
        match self.aspect {
            Some(a) => a.aspect_ratio,
            None => self.style.aspect_ratio(),
        }
    }
    #[inline(always)]
    fn margin(&self) -> Rect<LengthPercentageAuto> { self.style.margin() }
    #[inline(always)]
    fn padding(&self) -> Rect<LengthPercentage> { self.style.padding() }
    #[inline(always)]
    fn border(&self) -> Rect<LengthPercentage> { self.style.border() }
    #[inline(always)]
    fn contain(&self) -> Contain { self.style.contain() }
}

impl GridItemStyle for GridChildStyle<'_> {
    #[inline(always)]
    fn grid_row(&self) -> Line<GridPlacement<Self::CustomIdent>> { OofItemStyle::grid_row(self.style) }
    #[inline(always)]
    fn grid_column(&self) -> Line<GridPlacement<Self::CustomIdent>> { OofItemStyle::grid_column(self.style) }
    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> { GridItemStyle::align_self(self.style) }
    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> { GridItemStyle::justify_self(self.style) }
}

/// Runs `taffy::compute_leaf_layout`, clamping a single definite axis against `min_size`/`max_size`
/// before transferring through `aspect_ratio` (and ignoring `aspect_ratio` when both axes are
/// definite) under `SizingMode::InherentSize`. Leaves without `aspect_ratio` forward straight to
/// `taffy::compute_leaf_layout` with no extra work.
#[inline]
pub fn compute_leaf_layout<M>(
    inputs: LayoutInput,
    style: &CompactStyle,
    measure_function: M,
) -> LayoutOutput
where
    M: FnOnce(Size<Option<f32>>, Size<AvailableSpace>) -> Size<f32>,
{
    let Some(aspect) = container_aspect_override(style, inputs.parent_size, inputs.known_dimensions, inputs.sizing_mode)
    else {
        return taffy::compute_leaf_layout(inputs, style, no_calc, measure_function);
    };
    let wrapped = AspectStyle { style, aspect: Some(aspect) };
    taffy::compute_leaf_layout(inputs, &wrapped, no_calc, measure_function)
}
