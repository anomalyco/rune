//! Compact node style representation (96 bytes inline vs 552 bytes for `taffy::Style`).
//!
//! Packs the 13 hot `CompactLength` properties (size, min_size, max_size, padding, gap,
//! flex_basis) into `(u32, u8)` value+tag arrays alongside the flex/alignment scalars, and
//! spills cold CSS properties (grid tracks/placement, margin, inset, border, etc.) to an
//! optional boxed `taffy::Style` only on nodes that set them.

use taffy::geometry::{Line, Point, Rect, Size};
use taffy::style::{
    AlignContent, AlignItems, AlignSelf, BlockContainerStyle, BlockItemStyle, BoxGenerationMode, BoxSizing,
    CompactLength, Contain, CoreStyle, Dimension, Direction, Display, FlexDirection, FlexWrap, FlexboxContainerStyle,
    FlexboxItemStyle, GridAutoFlow, GridContainerStyle, GridItemStyle, GridPlacement, JustifyContent, LengthPercentage,
    LengthPercentageAuto, OofItemStyle, Overflow, Position, Style, TextAlign,
};
use taffy::style_helpers::{TaffyAuto, TaffyZero};

const IDX_SIZE_W: usize = 0;
const IDX_SIZE_H: usize = 1;
const IDX_MIN_W: usize = 2;
const IDX_MIN_H: usize = 3;
const IDX_MAX_W: usize = 4;
const IDX_MAX_H: usize = 5;
const IDX_PAD_L: usize = 6;
const IDX_PAD_R: usize = 7;
const IDX_PAD_T: usize = 8;
const IDX_PAD_B: usize = 9;
const IDX_GAP_W: usize = 10;
const IDX_GAP_H: usize = 11;
const IDX_BASIS: usize = 12;
const HOT_LEN_COUNT: usize = 13;

#[inline(always)]
fn pack_len(len: CompactLength) -> (u32, u8) {
    #[cfg(target_pointer_width = "64")]
    {
        // SAFETY: On 64-bit targets, `CompactLength` is `repr(transparent)` over `*const ()`
        // where non-calc variants pack `f32::to_bits(val)` in bits 32..64 and `tag` in bits 0..8.
        let raw: usize = unsafe { core::mem::transmute(len) };
        ((raw >> 32) as u32, (raw & 0xFF) as u8)
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        let tag = len.tag() as u8;
        let bits = if (tag as usize & CompactLength::KEYWORD_BIT) == 0
            || matches!(
                tag as usize,
                CompactLength::FR_TAG | CompactLength::FIT_CONTENT_PX_TAG | CompactLength::FIT_CONTENT_PERCENT_TAG
            ) {
            len.value().to_bits()
        } else {
            0
        };
        (bits, tag)
    }
}

#[inline(always)]
fn unpack_len(bits: u32, tag: u8) -> CompactLength {
    #[cfg(target_pointer_width = "64")]
    {
        let raw: usize = ((bits as usize) << 32) | (tag as usize);
        // SAFETY: Inverse of `pack_len` on 64-bit targets.
        unsafe { core::mem::transmute(raw) }
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        let val = f32::from_bits(bits);
        match tag as usize {
            CompactLength::LENGTH_TAG => CompactLength::length(val),
            CompactLength::PERCENT_TAG => CompactLength::percent(val),
            CompactLength::AUTO_TAG => CompactLength::auto(),
            CompactLength::MIN_CONTENT_TAG => CompactLength::min_content(),
            CompactLength::MAX_CONTENT_TAG => CompactLength::max_content(),
            CompactLength::FR_TAG => CompactLength::fr(val),
            CompactLength::FIT_CONTENT_PX_TAG => CompactLength::fit_content_px(val),
            CompactLength::FIT_CONTENT_PERCENT_TAG => CompactLength::fit_content_percent(val),
            CompactLength::FIT_CONTENT_KEYWORD_TAG => CompactLength::fit_content_keyword(),
            CompactLength::STRETCH_TAG => CompactLength::stretch(),
            CompactLength::CONTENT_TAG => CompactLength::content(),
            _ => CompactLength::auto(),
        }
    }
}

#[inline(always)]
fn unpack_dim(bits: u32, tag: u8) -> Dimension {
    // SAFETY: `bits` and `tag` were packed from a valid `Dimension`.
    unsafe { Dimension::from_raw(unpack_len(bits, tag)) }
}

#[inline(always)]
fn unpack_lpa(bits: u32, tag: u8) -> LengthPercentageAuto {
    // SAFETY: `bits` and `tag` were packed from a valid `LengthPercentageAuto`.
    unsafe { LengthPercentageAuto::from_raw(unpack_len(bits, tag)) }
}

#[inline(always)]
fn unpack_lp(bits: u32, tag: u8) -> LengthPercentage {
    // SAFETY: `bits` and `tag` were packed from a valid `LengthPercentage`.
    unsafe { LengthPercentage::from_raw(unpack_len(bits, tag)) }
}

#[inline(always)]
fn rect_is_zero_lpa(r: &Rect<LengthPercentageAuto>) -> bool {
    const ZERO: Rect<LengthPercentageAuto> = Rect {
        left: LengthPercentageAuto::ZERO,
        right: LengthPercentageAuto::ZERO,
        top: LengthPercentageAuto::ZERO,
        bottom: LengthPercentageAuto::ZERO,
    };
    let a: &[u64; 4] = unsafe { &*(r as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
    let b: &[u64; 4] = unsafe { &*(&ZERO as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
    a == b || *r == ZERO
}

#[inline(always)]
fn rect_is_zero_lp(r: &Rect<LengthPercentage>) -> bool {
    const ZERO: Rect<LengthPercentage> = Rect {
        left: LengthPercentage::ZERO,
        right: LengthPercentage::ZERO,
        top: LengthPercentage::ZERO,
        bottom: LengthPercentage::ZERO,
    };
    let a: &[u64; 4] = unsafe { &*(r as *const Rect<LengthPercentage> as *const [u64; 4]) };
    let b: &[u64; 4] = unsafe { &*(&ZERO as *const Rect<LengthPercentage> as *const [u64; 4]) };
    a == b || *r == ZERO
}

#[inline(always)]
fn rect_is_auto_lpa(r: &Rect<LengthPercentageAuto>) -> bool {
    const AUTO: Rect<LengthPercentageAuto> = Rect {
        left: LengthPercentageAuto::AUTO,
        right: LengthPercentageAuto::AUTO,
        top: LengthPercentageAuto::AUTO,
        bottom: LengthPercentageAuto::AUTO,
    };
    let a: &[u64; 4] = unsafe { &*(r as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
    let b: &[u64; 4] = unsafe { &*(&AUTO as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
    a == b || *r == AUTO
}

#[inline(always)]
fn has_cold_style(s: &Style) -> bool {
    s.display == Display::Grid
        || !rect_is_zero_lpa(&s.margin)
        || !rect_is_auto_lpa(&s.inset)
        || !rect_is_zero_lp(&s.border)
        || s.box_sizing != BoxSizing::BorderBox
        || s.direction != Direction::Ltr
        || s.scrollbar_width != 0.0
        || s.contain != Contain::NONE
        || s.justify_items != AlignItems::NORMAL
        || s.justify_self.is_some()
        || s.text_align != TextAlign::Auto
        || s.item_is_table
        || s.item_is_replaced
        || s.item_is_compressible_replaced
        || s.grid_auto_flow != GridAutoFlow::Row
        || s.grid_row != Line { start: GridPlacement::Auto, end: GridPlacement::Auto }
        || s.grid_column != Line { start: GridPlacement::Auto, end: GridPlacement::Auto }
        || !s.grid_template_rows.is_empty()
        || !s.grid_template_columns.is_empty()
        || !s.grid_auto_rows.is_empty()
        || !s.grid_auto_columns.is_empty()
        || s.grid_template_areas.is_some()
        || !s.grid_template_column_names.is_empty()
        || !s.grid_template_row_names.is_empty()
        || s.aspect_ratio.is_some_and(f32::is_nan)
}

#[inline(always)]
const fn encode_overflow(o: Overflow) -> u8 {
    match o {
        Overflow::Visible => 0,
        Overflow::Clip => 1,
        Overflow::Hidden => 2,
        Overflow::Scroll => 3,
    }
}

#[inline(always)]
const fn decode_overflow(b: u8) -> Overflow {
    match b & 0b11 {
        0 => Overflow::Visible,
        1 => Overflow::Clip,
        2 => Overflow::Hidden,
        _ => Overflow::Scroll,
    }
}

#[inline(always)]
const fn encode_wrap(w: FlexWrap) -> u8 {
    match w {
        FlexWrap::NoWrap => 0,
        FlexWrap::Wrap => 1,
        FlexWrap::WrapReverse => 2,
    }
}

#[inline(always)]
const fn decode_wrap(b: u8) -> FlexWrap {
    match b & 0b11 {
        0 => FlexWrap::NoWrap,
        1 => FlexWrap::Wrap,
        _ => FlexWrap::WrapReverse,
    }
}

#[inline(always)]
const fn encode_dir(d: FlexDirection) -> u8 {
    match d {
        FlexDirection::Row => 0,
        FlexDirection::Column => 1,
        FlexDirection::RowReverse => 2,
        FlexDirection::ColumnReverse => 3,
    }
}

#[inline(always)]
const fn decode_dir(b: u8) -> FlexDirection {
    match b & 0b11 {
        0 => FlexDirection::Row,
        1 => FlexDirection::Column,
        2 => FlexDirection::RowReverse,
        _ => FlexDirection::ColumnReverse,
    }
}

#[inline(always)]
const fn pack_box_flags(dir: FlexDirection, wrap: FlexWrap, ox: Overflow, oy: Overflow) -> u8 {
    encode_dir(dir) | (encode_wrap(wrap) << 2) | (encode_overflow(ox) << 4) | (encode_overflow(oy) << 6)
}

/// Compact representation of `taffy::Style` (96 bytes inline).
#[derive(Debug, Clone)]
pub struct CompactStyle {
    vals: [u32; HOT_LEN_COUNT],
    flex_grow: f32,
    flex_shrink: f32,
    aspect_ratio: f32,
    tags: [u8; HOT_LEN_COUNT],
    pub display: Display,
    pub position: Position,
    box_flags: u8,
    pub align_items: AlignItems,
    pub align_self: Option<AlignSelf>,
    pub justify_content: JustifyContent,
    pub align_content: AlignContent,
    extra: Option<Box<Style>>,
}

impl Default for CompactStyle {
    fn default() -> Self {
        Self::from_style(Style::DEFAULT)
    }
}

impl CompactStyle {
    pub fn from_style(s: Style) -> Self {
        let mut vals = [0u32; HOT_LEN_COUNT];
        let mut tags = [0u8; HOT_LEN_COUNT];
        let raw_lens = [
            s.size.width.into_raw(),
            s.size.height.into_raw(),
            s.min_size.width.into_raw(),
            s.min_size.height.into_raw(),
            s.max_size.width.into_raw(),
            s.max_size.height.into_raw(),
            s.padding.left.into_raw(),
            s.padding.right.into_raw(),
            s.padding.top.into_raw(),
            s.padding.bottom.into_raw(),
            s.gap.width.into_raw(),
            s.gap.height.into_raw(),
            s.flex_basis.into_raw(),
        ];
        for (i, len) in raw_lens.into_iter().enumerate() {
            let (v, t) = pack_len(len);
            vals[i] = v;
            tags[i] = t;
        }
        let flex_grow = s.flex_grow;
        let flex_shrink = s.flex_shrink;
        let aspect_ratio = s.aspect_ratio.unwrap_or(f32::NAN);
        let display = s.display;
        let position = s.position;
        let box_flags = pack_box_flags(s.flex_direction, s.flex_wrap, s.overflow.x, s.overflow.y);
        let align_items = s.align_items;
        let align_self = s.align_self;
        let justify_content = s.justify_content;
        let align_content = s.align_content;
        let extra = has_cold_style(&s).then(|| Box::new(s));
        Self {
            vals,
            flex_grow,
            flex_shrink,
            aspect_ratio,
            tags,
            display,
            position,
            box_flags,
            align_items,
            align_self,
            justify_content,
            align_content,
            extra,
        }
    }

    pub fn to_style(&self) -> Style {
        if let Some(extra) = &self.extra {
            return (**extra).clone();
        }
        Style {
            display: self.display,
            position: self.position,
            overflow: CoreStyle::overflow(self),
            size: CoreStyle::size(self),
            min_size: CoreStyle::min_size(self),
            max_size: CoreStyle::max_size(self),
            aspect_ratio: CoreStyle::aspect_ratio(self),
            padding: CoreStyle::padding(self),
            gap: FlexboxContainerStyle::gap(self),
            align_items: self.align_items,
            align_self: self.align_self,
            justify_content: self.justify_content,
            align_content: self.align_content,
            flex_direction: FlexboxContainerStyle::flex_direction(self),
            flex_wrap: FlexboxContainerStyle::flex_wrap(self),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            flex_basis: FlexboxItemStyle::flex_basis(self),
            ..Style::DEFAULT
        }
    }

    pub fn heap_bytes(&self) -> usize {
        self.extra.as_ref().map_or(0, |s| {
            type Ident = <Style as CoreStyle>::CustomIdent;
            core::mem::size_of::<Style>()
                + s.grid_template_rows.capacity() * core::mem::size_of::<taffy::GridTemplateComponent<Ident>>()
                + s.grid_template_columns.capacity() * core::mem::size_of::<taffy::GridTemplateComponent<Ident>>()
        })
    }

    #[inline(always)]
    pub fn eq_style(&self, other: &Style) -> bool {
        if let Some(extra) = &self.extra {
            return &**extra == other;
        }
        self.display == other.display
            && self.position == other.position
            && self.box_flags == pack_box_flags(other.flex_direction, other.flex_wrap, other.overflow.x, other.overflow.y)
            && self.align_items == other.align_items
            && self.align_self == other.align_self
            && self.justify_content == other.justify_content
            && self.align_content == other.align_content
            && self.flex_grow == other.flex_grow
            && self.flex_shrink == other.flex_shrink
            && CoreStyle::aspect_ratio(self) == other.aspect_ratio
            && CoreStyle::size(self) == other.size
            && CoreStyle::min_size(self) == other.min_size
            && CoreStyle::max_size(self) == other.max_size
            && CoreStyle::padding(self) == other.padding
            && FlexboxContainerStyle::gap(self) == other.gap
            && FlexboxItemStyle::flex_basis(self) == other.flex_basis
            && !has_cold_style(other)
    }
}

impl From<Style> for CompactStyle {
    #[inline]
    fn from(s: Style) -> Self {
        Self::from_style(s)
    }
}

impl From<&Style> for CompactStyle {
    #[inline]
    fn from(s: &Style) -> Self {
        Self::from_style(s.clone())
    }
}

impl From<CompactStyle> for Style {
    #[inline]
    fn from(s: CompactStyle) -> Self {
        match s.extra {
            Some(extra) => *extra,
            None => s.to_style(),
        }
    }
}

impl From<&CompactStyle> for Style {
    #[inline]
    fn from(s: &CompactStyle) -> Self {
        s.to_style()
    }
}

impl PartialEq for CompactStyle {
    fn eq(&self, other: &Self) -> bool {
        match (&self.extra, &other.extra) {
            (None, None) => {
                self.vals == other.vals
                    && self.tags == other.tags
                    && self.flex_grow == other.flex_grow
                    && self.flex_shrink == other.flex_shrink
                    && self.aspect_ratio.to_bits() == other.aspect_ratio.to_bits()
                    && self.display == other.display
                    && self.position == other.position
                    && self.box_flags == other.box_flags
                    && self.align_items == other.align_items
                    && self.align_self == other.align_self
                    && self.justify_content == other.justify_content
                    && self.align_content == other.align_content
            }
            (Some(a), Some(b)) => a == b,
            (Some(a), None) => other.eq_style(a),
            (None, Some(b)) => self.eq_style(b),
        }
    }
}

impl PartialEq<Style> for CompactStyle {
    #[inline(always)]
    fn eq(&self, other: &Style) -> bool {
        self.eq_style(other)
    }
}

impl CoreStyle for CompactStyle {
    type CustomIdent = <Style as CoreStyle>::CustomIdent;

    #[inline(always)]
    fn box_generation_mode(&self) -> BoxGenerationMode {
        match self.display {
            Display::None => BoxGenerationMode::None,
            _ => BoxGenerationMode::Normal,
        }
    }

    #[inline(always)]
    fn is_block(&self) -> bool {
        matches!(self.display, Display::Block)
    }

    #[inline(always)]
    fn is_compressible_replaced(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_replaced || s.item_is_compressible_replaced)
    }

    #[inline(always)]
    fn is_replaced(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_replaced)
    }

    #[inline(always)]
    fn box_sizing(&self) -> BoxSizing {
        self.extra.as_ref().map_or(BoxSizing::BorderBox, |s| s.box_sizing)
    }

    #[inline(always)]
    fn direction(&self) -> Direction {
        self.extra.as_ref().map_or(Direction::Ltr, |s| s.direction)
    }

    #[inline(always)]
    fn overflow(&self) -> Point<Overflow> {
        Point {
            x: decode_overflow(self.box_flags >> 4),
            y: decode_overflow(self.box_flags >> 6),
        }
    }

    #[inline(always)]
    fn scrollbar_width(&self) -> f32 {
        self.extra.as_ref().map_or(0.0, |s| s.scrollbar_width)
    }

    #[inline(always)]
    fn position(&self) -> Position {
        self.position
    }

    #[inline(always)]
    fn inset(&self) -> Rect<LengthPercentageAuto> {
        self.extra.as_ref().map_or(Rect::auto(), |s| s.inset)
    }

    #[inline(always)]
    fn size(&self) -> Size<Dimension> {
        Size {
            width: unpack_dim(self.vals[IDX_SIZE_W], self.tags[IDX_SIZE_W]),
            height: unpack_dim(self.vals[IDX_SIZE_H], self.tags[IDX_SIZE_H]),
        }
    }

    #[inline(always)]
    fn min_size(&self) -> Size<LengthPercentageAuto> {
        Size {
            width: unpack_lpa(self.vals[IDX_MIN_W], self.tags[IDX_MIN_W]),
            height: unpack_lpa(self.vals[IDX_MIN_H], self.tags[IDX_MIN_H]),
        }
    }

    #[inline(always)]
    fn max_size(&self) -> Size<LengthPercentageAuto> {
        Size {
            width: unpack_lpa(self.vals[IDX_MAX_W], self.tags[IDX_MAX_W]),
            height: unpack_lpa(self.vals[IDX_MAX_H], self.tags[IDX_MAX_H]),
        }
    }

    #[inline(always)]
    fn aspect_ratio(&self) -> Option<f32> {
        if !self.aspect_ratio.is_nan() {
            Some(self.aspect_ratio)
        } else {
            self.extra.as_ref().and_then(|s| s.aspect_ratio)
        }
    }

    #[inline(always)]
    fn margin(&self) -> Rect<LengthPercentageAuto> {
        self.extra.as_ref().map_or(Rect::zero(), |s| s.margin)
    }

    #[inline(always)]
    fn padding(&self) -> Rect<LengthPercentage> {
        Rect {
            left: unpack_lp(self.vals[IDX_PAD_L], self.tags[IDX_PAD_L]),
            right: unpack_lp(self.vals[IDX_PAD_R], self.tags[IDX_PAD_R]),
            top: unpack_lp(self.vals[IDX_PAD_T], self.tags[IDX_PAD_T]),
            bottom: unpack_lp(self.vals[IDX_PAD_B], self.tags[IDX_PAD_B]),
        }
    }

    #[inline(always)]
    fn border(&self) -> Rect<LengthPercentage> {
        self.extra.as_ref().map_or(Rect::zero(), |s| s.border)
    }

    #[inline(always)]
    fn contain(&self) -> Contain {
        self.extra.as_ref().map_or(Contain::NONE, |s| s.contain)
    }
}

impl OofItemStyle for CompactStyle {
    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> {
        self.extra.as_ref().and_then(|s| s.justify_self)
    }

    #[inline(always)]
    fn is_table(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_table)
    }

    #[inline(always)]
    fn grid_row(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        self.extra
            .as_ref()
            .map_or(Line { start: GridPlacement::Auto, end: GridPlacement::Auto }, |s| s.grid_row.clone())
    }

    #[inline(always)]
    fn grid_column(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        self.extra
            .as_ref()
            .map_or(Line { start: GridPlacement::Auto, end: GridPlacement::Auto }, |s| s.grid_column.clone())
    }
}

impl FlexboxContainerStyle for CompactStyle {
    #[inline(always)]
    fn flex_direction(&self) -> FlexDirection {
        decode_dir(self.box_flags)
    }

    #[inline(always)]
    fn flex_wrap(&self) -> FlexWrap {
        decode_wrap(self.box_flags >> 2)
    }

    #[inline(always)]
    fn gap(&self) -> Size<LengthPercentage> {
        Size {
            width: unpack_lp(self.vals[IDX_GAP_W], self.tags[IDX_GAP_W]),
            height: unpack_lp(self.vals[IDX_GAP_H], self.tags[IDX_GAP_H]),
        }
    }

    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        self.align_content
    }

    #[inline(always)]
    fn align_items(&self) -> AlignItems {
        self.align_items
    }

    #[inline(always)]
    fn justify_content(&self) -> JustifyContent {
        self.justify_content
    }
}

impl FlexboxItemStyle for CompactStyle {
    #[inline(always)]
    fn flex_basis(&self) -> Dimension {
        unpack_dim(self.vals[IDX_BASIS], self.tags[IDX_BASIS])
    }

    #[inline(always)]
    fn flex_grow(&self) -> f32 {
        self.flex_grow
    }

    #[inline(always)]
    fn flex_shrink(&self) -> f32 {
        self.flex_shrink
    }

    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }
}

impl GridContainerStyle for CompactStyle {
    type Repetition<'a>
        = <Style as GridContainerStyle>::Repetition<'a>
    where
        Self: 'a;

    type TemplateTrackList<'a>
        = <Style as GridContainerStyle>::TemplateTrackList<'a>
    where
        Self: 'a;

    type AutoTrackList<'a>
        = <Style as GridContainerStyle>::AutoTrackList<'a>
    where
        Self: 'a;

    type TemplateLineNames<'a>
        = <Style as GridContainerStyle>::TemplateLineNames<'a>
    where
        Self: 'a;

    type GridTemplateAreas<'a>
        = <Style as GridContainerStyle>::GridTemplateAreas<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn grid_template_rows(&self) -> Option<Self::TemplateTrackList<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_rows(&**s))
    }

    #[inline(always)]
    fn grid_template_columns(&self) -> Option<Self::TemplateTrackList<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_columns(&**s))
    }

    #[inline(always)]
    fn grid_auto_rows(&self) -> Self::AutoTrackList<'_> {
        self.extra.as_ref().map_or_else(|| [].iter().copied(), |s| GridContainerStyle::grid_auto_rows(&**s))
    }

    #[inline(always)]
    fn grid_auto_columns(&self) -> Self::AutoTrackList<'_> {
        self.extra.as_ref().map_or_else(|| [].iter().copied(), |s| GridContainerStyle::grid_auto_columns(&**s))
    }

    #[inline(always)]
    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.extra.as_ref().map_or(GridAutoFlow::Row, |s| s.grid_auto_flow)
    }

    #[inline(always)]
    fn gap(&self) -> Size<LengthPercentage> {
        FlexboxContainerStyle::gap(self)
    }

    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        self.align_content
    }

    #[inline(always)]
    fn justify_content(&self) -> JustifyContent {
        self.justify_content
    }

    #[inline(always)]
    fn align_items(&self) -> AlignItems {
        self.align_items
    }

    #[inline(always)]
    fn justify_items(&self) -> AlignItems {
        self.extra.as_ref().map_or(AlignItems::NORMAL, |s| s.justify_items)
    }

    #[inline(always)]
    fn grid_template_areas(&self) -> Option<Self::GridTemplateAreas<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_areas(&**s))
    }

    #[inline(always)]
    fn grid_template_area_row_count(&self) -> u16 {
        self.extra.as_ref().map_or(0, |s| GridContainerStyle::grid_template_area_row_count(&**s))
    }

    #[inline(always)]
    fn grid_template_area_column_count(&self) -> u16 {
        self.extra.as_ref().map_or(0, |s| GridContainerStyle::grid_template_area_column_count(&**s))
    }

    #[inline(always)]
    fn grid_template_column_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_column_names(&**s))
    }

    #[inline(always)]
    fn grid_template_row_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_row_names(&**s))
    }
}

impl GridItemStyle for CompactStyle {
    #[inline(always)]
    fn grid_row(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        OofItemStyle::grid_row(self)
    }

    #[inline(always)]
    fn grid_column(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        OofItemStyle::grid_column(self)
    }

    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> {
        self.extra.as_ref().and_then(|s| s.justify_self)
    }
}

impl BlockContainerStyle for CompactStyle {
    #[inline(always)]
    fn text_align(&self) -> TextAlign {
        self.extra.as_ref().map_or(TextAlign::Auto, |s| s.text_align)
    }

    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        self.align_content
    }

    #[inline(always)]
    fn justify_items(&self) -> AlignItems {
        self.extra.as_ref().map_or(AlignItems::NORMAL, |s| s.justify_items)
    }
}

impl BlockItemStyle for CompactStyle {
    #[inline(always)]
    fn is_table(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_table)
    }

    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> {
        self.extra.as_ref().and_then(|s| s.justify_self)
    }

    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        self.align_content
    }
}
