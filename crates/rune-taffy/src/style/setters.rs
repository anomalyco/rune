//! Builder setters for [`CompactStyle`].

use super::*;

impl CompactStyle {
    #[inline(always)]
    fn set_dim_slot(&mut self, idx: usize, raw: CompactLength, cold_set: impl FnOnce(&mut Style)) {
        if self.extra.is_none() {
            let (v, t) = pack_raw_len(raw);
            if let Some(code) = encode_dim_tag(t) {
                self.vals[idx] = v;
                let shift = idx * 2;
                self.len_tags = (self.len_tags & !(0b11 << shift)) | (code << shift);
                return;
            }
        }
        self.modify_cold(cold_set);
    }

    #[inline(always)]
    pub fn set_width(&mut self, d: Dimension) {
        self.set_dim_slot(IDX_SIZE_W, d.into_raw(), |s| s.size.width = d);
    }

    #[inline(always)]
    pub fn set_height(&mut self, d: Dimension) {
        self.set_dim_slot(IDX_SIZE_H, d.into_raw(), |s| s.size.height = d);
    }

    #[inline(always)]
    pub fn set_size(&mut self, size: Size<Dimension>) {
        self.set_width(size.width);
        self.set_height(size.height);
    }

    #[inline(always)]
    pub fn set_min_width(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MIN_W, d.into_raw(), |s| s.min_size.width = d);
    }

    #[inline(always)]
    pub fn set_min_height(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MIN_H, d.into_raw(), |s| s.min_size.height = d);
    }

    #[inline(always)]
    pub fn set_max_width(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MAX_W, d.into_raw(), |s| s.max_size.width = d);
    }

    #[inline(always)]
    pub fn set_max_height(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MAX_H, d.into_raw(), |s| s.max_size.height = d);
    }

    #[inline(always)]
    pub fn set_pad_left(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_L] = p.to_bits();
            self.len_tags &= !(1 << 12);
        } else {
            self.modify_cold(|s| s.padding.left = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_pad_right(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_R] = p.to_bits();
            self.len_tags &= !(1 << 13);
        } else {
            self.modify_cold(|s| s.padding.right = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_pad_top(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_T] = p.to_bits();
            self.len_tags &= !(1 << 14);
        } else {
            self.modify_cold(|s| s.padding.top = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_pad_bottom(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_B] = p.to_bits();
            self.len_tags &= !(1 << 15);
        } else {
            self.modify_cold(|s| s.padding.bottom = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_padding_all(&mut self, p: f32) {
        if self.extra.is_none() {
            let bits = p.to_bits();
            self.vals[IDX_PAD_L] = bits;
            self.vals[IDX_PAD_R] = bits;
            self.vals[IDX_PAD_T] = bits;
            self.vals[IDX_PAD_B] = bits;
            self.len_tags &= !0xF000;
        } else {
            let lp = LengthPercentage::length(p);
            self.modify_cold(|s| s.padding = Rect { left: lp, right: lp, top: lp, bottom: lp });
        }
    }

    #[inline(always)]
    pub fn set_gap(&mut self, column: f32, row: f32) {
        if self.extra.is_none()
            && let (Some(cw), Some(rh)) = (pack_bf16_exact(column), pack_bf16_exact(row))
        {
            self.gap_w_bf16 = cw;
            self.gap_h_bf16 = rh;
            self.mode_flags &= !(MODE_GAP_W_PCT | MODE_GAP_H_PCT);
            return;
        }
        self.modify_cold(|s| {
            s.gap = Size {
                width: LengthPercentage::length(column),
                height: LengthPercentage::length(row),
            };
        });
    }

    #[inline(always)]
    pub fn set_grow(&mut self, g: f32) {
        if self.extra.is_none()
            && let Some(gb) = pack_bf16_exact(g)
        {
            self.flex_grow_bf16 = gb;
            self.flex_shrink_bf16 = ONE_BF16;
            self.mode_flags |= MODE_BASIS_ZERO;
            return;
        }
        self.modify_cold(|s| {
            s.flex_grow = g;
            s.flex_shrink = 1.0;
            s.flex_basis = Dimension::ZERO;
        });
    }

    #[inline(always)]
    pub fn set_flex_grow(&mut self, g: f32) {
        if self.extra.is_none()
            && let Some(gb) = pack_bf16_exact(g)
        {
            self.flex_grow_bf16 = gb;
            return;
        }
        self.modify_cold(|s| s.flex_grow = g);
    }

    #[inline(always)]
    pub fn set_flex_shrink(&mut self, sh: f32) {
        if self.extra.is_none()
            && let Some(sb) = pack_bf16_exact(sh)
        {
            self.flex_shrink_bf16 = sb;
            return;
        }
        self.modify_cold(|s| s.flex_shrink = sh);
    }

    #[inline(always)]
    pub fn set_flex_basis(&mut self, basis: Dimension) {
        if self.extra.is_none() {
            let (bits, tag) = pack_raw_len(basis.into_raw());
            if tag as usize == CompactLength::AUTO_TAG {
                self.mode_flags &= !MODE_BASIS_ZERO;
                return;
            }
            if tag as usize == CompactLength::LENGTH_TAG && bits == 0 {
                self.mode_flags |= MODE_BASIS_ZERO;
                return;
            }
        }
        self.modify_cold(|s| s.flex_basis = basis);
    }

    #[inline(always)]
    pub fn set_flex_direction(&mut self, dir: FlexDirection) {
        self.box_flags = (self.box_flags & !0b11) | encode_dir(dir);
        if let Some(s) = &mut self.extra {
            s.flex_direction = dir;
        }
    }

    #[inline(always)]
    pub fn set_flex_wrap(&mut self, wrap: FlexWrap) {
        self.box_flags = (self.box_flags & !(0b11 << 2)) | (encode_wrap(wrap) << 2);
        if let Some(s) = &mut self.extra {
            s.flex_wrap = wrap;
        }
    }

    #[inline(always)]
    pub fn set_overflow(&mut self, x: Overflow, y: Overflow) {
        self.box_flags = (self.box_flags & 0x0F) | (encode_overflow(x) << 4) | (encode_overflow(y) << 6);
        if let Some(s) = &mut self.extra {
            s.overflow = Point { x, y };
        }
    }

    #[inline(always)]
    pub fn set_align_items(&mut self, a: AlignItems) {
        self.align_items_u8 = pack_align_items(a);
        if let Some(s) = &mut self.extra {
            s.align_items = a;
        }
    }

    #[inline(always)]
    pub fn set_align_self(&mut self, a: Option<AlignSelf>) {
        self.align_self_u8 = pack_align_self(a);
        if let Some(s) = &mut self.extra {
            s.align_self = a;
        }
    }

    #[inline(always)]
    pub fn set_justify_content(&mut self, j: JustifyContent) {
        self.justify_content_u8 = pack_align_content(j);
        if let Some(s) = &mut self.extra {
            s.justify_content = j;
        }
    }

    #[inline(always)]
    pub fn set_aspect_ratio(&mut self, ratio: Option<f32>) {
        if self.extra.is_none() && ratio.is_none() {
            return;
        }
        self.modify_cold(|s| s.aspect_ratio = ratio);
    }

    #[inline(always)]
    pub fn set_equal_columns(&mut self, n: usize) {
        self.modify_cold(|s| {
            s.display = Display::Grid;
            s.grid_template_columns = vec![fr(1.0); n.max(1)];
        });
    }

    #[inline(always)]
    pub fn set_grid_cell(&mut self, column: usize) {
        self.modify_cold(|s| {
            s.grid_column = Line {
                start: line(column as i16 + 1),
                end: span(1),
            };
            s.grid_row = Line {
                start: line(1),
                end: span(1),
            };
        });
    }

    #[inline(always)]
    pub fn set_position_absolute(&mut self) {
        if let Some(s) = &mut self.extra {
            s.position = Position::Absolute;
        }
        self.mode_flags = (self.mode_flags & !MODE_POS_MASK) | (2 << MODE_POS_SHIFT);
    }

    #[inline(always)]
    pub fn set_absolute_inset(&mut self, inset: Rect<LengthPercentageAuto>) {
        if self.extra.is_none() && rect_is_auto_lpa(&inset) {
            self.mode_flags = (self.mode_flags & !MODE_POS_MASK) | (2 << MODE_POS_SHIFT);
            return;
        }
        self.modify_cold(|s| {
            s.position = Position::Absolute;
            s.inset = inset;
        });
    }

    #[inline(always)]
    pub fn set_margin_left(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.left = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_right(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.right = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_top(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.top = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_bottom(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.bottom = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_x(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        let v = LengthPercentageAuto::length(m);
        self.modify_cold(|s| { s.margin.left = v; s.margin.right = v; });
    }

    #[inline(always)]
    pub fn set_margin_y(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        let v = LengthPercentageAuto::length(m);
        self.modify_cold(|s| { s.margin.top = v; s.margin.bottom = v; });
    }
}
