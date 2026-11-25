use crate::{parts::DecorationParts, theme};
use std::collections::BTreeMap;
use tiny_skia::{Pixmap, PixmapMut, PixmapRef, Point, PremultipliedColorU8};

// These values were generated from a screenshot of an libadwaita window using a script.
// For more details see: https://github.com/PolyMeilex/sctk-adwaita/pull/43
pub const SHADOW_SIZE: u32 = 43;
const SHADOW_PARAMS_ACTIVE: (f32, f32, f32) = (0.206_505_5, 0.104_617_53, -0.000_542_446_2);
const SHADOW_PARAMS_INACTIVE: (f32, f32, f32) = (0.168_297_29, 0.204_299_8, 0.001_769_798_6);

/// The shadow's alpha at `pixel_dist` from the window's outline: the frame's own model of it,
/// which `tests` reads as its reference.
pub(crate) fn shadow(pixel_dist: f32, scale: u32, active: bool) -> f32 {
    let (a, b, c) = if active {
        SHADOW_PARAMS_ACTIVE
    } else {
        SHADOW_PARAMS_INACTIVE
    };

    a * (-b * (pixel_dist / scale as f32)).exp() + c
}

#[derive(Debug)]
struct RenderedShadow {
    side: Pixmap,
    edges: Pixmap,
}

impl RenderedShadow {
    fn new(scale: u32, active: bool) -> RenderedShadow {
        let shadow_size = SHADOW_SIZE * scale;
        let corner_radius = theme::CORNER_RADIUS * scale;

        #[allow(clippy::unwrap_used)]
        let mut side = Pixmap::new(shadow_size, 1).unwrap();
        for x in 0..side.width() as usize {
            let alpha = (shadow(x as f32 + 0.5, scale, active) * u8::MAX as f32).round() as u8;

            #[allow(clippy::unwrap_used)]
            let color = PremultipliedColorU8::from_rgba(0, 0, 0, alpha).unwrap();
            side.pixels_mut()[x] = color;
        }

        let edges_size = (corner_radius + shadow_size) * 2;
        #[allow(clippy::unwrap_used)]
        let mut edges = Pixmap::new(edges_size, edges_size).unwrap();
        let edges_middle = Point::from_xy(edges_size as f32 / 2.0, edges_size as f32 / 2.0);
        for y in 0..edges_size as usize {
            let y_pos = y as f32 + 0.5;
            for x in 0..edges_size as usize {
                let dist = edges_middle.distance(Point::from_xy(x as f32 + 0.5, y_pos))
                    - corner_radius as f32;
                let alpha = (shadow(dist, scale, active) * u8::MAX as f32).round() as u8;

                #[allow(clippy::unwrap_used)]
                let color = PremultipliedColorU8::from_rgba(0, 0, 0, alpha).unwrap();
                edges.pixels_mut()[y * edges_size as usize + x] = color;
            }
        }

        RenderedShadow { side, edges }
    }

    fn side_draw(
        &self,
        flipped: bool,
        rotated: bool,
        stack: usize,
        dst_pixmap: &mut PixmapMut,
        dst_left: usize,
        dst_top: usize,
    ) {
        fn iter_copy<'a>(
            src: impl Iterator<Item = &'a PremultipliedColorU8>,
            dst: impl Iterator<Item = &'a mut PremultipliedColorU8>,
        ) {
            src.zip(dst).for_each(|(src, dst)| *dst = *src)
        }

        let dst_width = dst_pixmap.width() as usize;
        let dst_pixels = dst_pixmap.pixels_mut();
        match (flipped, rotated) {
            (false, false) => (0..stack).for_each(|i| {
                let dst = dst_pixels
                    .iter_mut()
                    .skip((dst_top + i) * dst_width + dst_left);
                iter_copy(self.side.pixels().iter(), dst);
            }),
            (false, true) => (0..stack).for_each(|i| {
                let dst = dst_pixels
                    .iter_mut()
                    .skip(dst_top * dst_width + dst_left + i)
                    .step_by(dst_width);
                iter_copy(self.side.pixels().iter(), dst);
            }),
            (true, false) => (0..stack).for_each(|i| {
                let dst = dst_pixels
                    .iter_mut()
                    .skip((dst_top + i) * dst_width + dst_left);
                iter_copy(self.side.pixels().iter().rev(), dst);
            }),
            (true, true) => (0..stack).for_each(|i| {
                let dst = dst_pixels
                    .iter_mut()
                    .skip(dst_top * dst_width + dst_left + i)
                    .step_by(dst_width);
                iter_copy(self.side.pixels().iter().rev(), dst);
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn edges_draw(
        &self,
        src_x_offset: isize,
        src_y_offset: isize,
        dst_pixmap: &mut PixmapMut,
        dst_rect_left: usize,
        dst_rect_top: usize,
        dst_rect_width: usize,
        dst_rect_height: usize,
    ) {
        let src_width = self.edges.width() as usize;
        let src_pixels = self.edges.pixels();
        let dst_width = dst_pixmap.width() as usize;
        let dst_pixels = dst_pixmap.pixels_mut();
        for y in 0..dst_rect_height {
            let dst_y = dst_rect_top + y;
            let src_y = y as isize + src_y_offset;
            if src_y < 0 {
                continue;
            }

            let src_y = src_y as usize;
            for x in 0..dst_rect_width {
                let dst_x = dst_rect_left + x;
                let src_x = x as isize + src_x_offset;
                if src_x < 0 {
                    continue;
                }

                let src = src_pixels.get(src_y * src_width + src_x as usize);
                let dst = dst_pixels.get_mut(dst_y * dst_width + dst_x);
                if let (Some(src), Some(dst)) = (src, dst) {
                    *dst = *src;
                }
            }
        }
    }

    fn draw(&self, dst_pixmap: &mut PixmapMut, scale: u32, part_idx: usize, corners: f32) {
        let shadow_size = (SHADOW_SIZE * scale) as usize;
        let visible_border_size = (theme::VISIBLE_BORDER_SIZE * scale) as usize;
        let corner_radius = (theme::CORNER_RADIUS * scale) as usize;
        assert!(corner_radius > visible_border_size);

        let dst_width = dst_pixmap.width() as usize;
        let dst_height = dst_pixmap.height() as usize;
        let edges_half = self.edges.width() as usize / 2;
        match part_idx {
            DecorationParts::TOP => {
                let left_edge_width = edges_half;
                let right_edge_width = edges_half;
                let side_width = dst_width
                    .saturating_sub(left_edge_width)
                    .saturating_sub(right_edge_width);

                self.edges_draw(
                    0,
                    -(visible_border_size as isize),
                    dst_pixmap,
                    0,
                    0,
                    left_edge_width,
                    dst_height,
                );

                self.side_draw(
                    true,
                    true,
                    side_width,
                    dst_pixmap,
                    left_edge_width,
                    visible_border_size,
                );

                self.edges_draw(
                    edges_half as isize,
                    -(visible_border_size as isize),
                    dst_pixmap,
                    left_edge_width + side_width,
                    0,
                    right_edge_width,
                    dst_height,
                );
            }
            DecorationParts::LEFT => {
                let bottom_edge_height = corner_radius - visible_border_size;
                // Without a header (`corners`) the part starts at the window's own rounded top
                // corner: its top piece is the bottom piece's mirror.
                let top_edge_height = if corners > 0. { bottom_edge_height } else { corner_radius };
                let side_height = dst_height
                    .saturating_sub(top_edge_height)
                    .saturating_sub(bottom_edge_height);

                if corners > 0. {
                    self.edges_draw(
                        0,
                        edges_half as isize - top_edge_height as isize,
                        dst_pixmap,
                        0,
                        0,
                        dst_width,
                        top_edge_height,
                    );
                } else {
                    self.edges_draw(
                        0,
                        shadow_size as isize,
                        dst_pixmap,
                        0,
                        0,
                        dst_width.saturating_sub(visible_border_size),
                        top_edge_height,
                    );
                }

                self.side_draw(true, false, side_height, dst_pixmap, 0, top_edge_height);

                // The bottom corner's piece reaches the border's own column: below the arc's
                // tangent the side part draws no border there, and the shadow continues to the
                // window's edge (`draw_side`).
                self.edges_draw(
                    0,
                    edges_half as isize,
                    dst_pixmap,
                    0,
                    top_edge_height + side_height,
                    dst_width,
                    bottom_edge_height,
                );
            }
            DecorationParts::RIGHT => {
                let bottom_edge_height = corner_radius - visible_border_size;
                // As the left part: without a header the top piece mirrors the bottom one.
                let top_edge_height = if corners > 0. { bottom_edge_height } else { corner_radius };
                let side_height = dst_height
                    .saturating_sub(top_edge_height)
                    .saturating_sub(bottom_edge_height);

                if corners > 0. {
                    self.edges_draw(
                        edges_half as isize + (corner_radius - visible_border_size) as isize,
                        edges_half as isize - top_edge_height as isize,
                        dst_pixmap,
                        0,
                        0,
                        dst_width,
                        top_edge_height,
                    );
                } else {
                    self.edges_draw(
                        edges_half as isize + corner_radius as isize,
                        shadow_size as isize,
                        dst_pixmap,
                        visible_border_size,
                        0,
                        dst_width.saturating_sub(visible_border_size),
                        top_edge_height,
                    );
                }

                self.side_draw(
                    false,
                    false,
                    side_height,
                    dst_pixmap,
                    visible_border_size,
                    top_edge_height,
                );

                // The bottom corner's piece reaches the border's own column, where the side part
                // draws no border below the arc's tangent: its source keeps the disc's centre where
                // the top piece puts it (a corner radius outside this part, so `+ corner_radius`,
                // less the column the piece now starts at).
                self.edges_draw(
                    edges_half as isize + (corner_radius - visible_border_size) as isize,
                    edges_half as isize,
                    dst_pixmap,
                    0,
                    top_edge_height + side_height,
                    dst_width,
                    bottom_edge_height,
                );
            }
            DecorationParts::BOTTOM => {
                let left_edge_width = edges_half;
                let right_edge_width = edges_half;
                let side_width = dst_width
                    .saturating_sub(left_edge_width)
                    .saturating_sub(right_edge_width);

                // Below the window's bottom edge the shadow is a square frame's, whatever the
                // corners, and the discs sit `visible_border_size` up from that edge: the source
                // row puts them on the same pixel here and in the square part.
                let corner_src_y =
                    edges_half as isize + (corner_radius - visible_border_size) as isize;
                self.edges_draw(
                    0,
                    corner_src_y,
                    dst_pixmap,
                    0,
                    corner_radius,
                    left_edge_width,
                    dst_height,
                );

                self.side_draw(
                    false,
                    true,
                    side_width,
                    dst_pixmap,
                    left_edge_width,
                    corner_radius + visible_border_size,
                );

                self.edges_draw(
                    edges_half as isize,
                    corner_src_y,
                    dst_pixmap,
                    left_edge_width + side_width,
                    corner_radius,
                    right_edge_width,
                    dst_height,
                );

                if corners > 0. {
                    // The band above that edge carries the crescents the arcs cut out of the
                    // shadow, and only inside the content's own corner squares: outside them the
                    // side parts already draw the shadow down to the edge, and a second copy here
                    // would composite over theirs (the parts stack) and double it.
                    //
                    // The band's source keeps the disc's centre where the corner piece above puts
                    // it: `edges_half` in the image, `centre_x` in the part.
                    let band_src_y = edges_half as isize - visible_border_size as isize;
                    let margin = theme::BORDER_SIZE as usize * scale as usize;
                    let centres = [edges_half, dst_width.saturating_sub(edges_half)];
                    let bands = [margin, dst_width.saturating_sub(margin + corner_radius)];
                    for (centre_x, band_left) in centres.into_iter().zip(bands) {
                        let src_x = band_left as isize - centre_x as isize + edges_half as isize;
                        self.edges_draw(src_x, band_src_y, dst_pixmap, band_left, 0, corner_radius, corner_radius);
                    }
                }
            }
            DecorationParts::HEADER => {
                self.edges_draw(
                    shadow_size as isize,
                    shadow_size as isize,
                    dst_pixmap,
                    0,
                    0,
                    corner_radius,
                    corner_radius,
                );

                self.edges_draw(
                    edges_half as isize,
                    shadow_size as isize,
                    dst_pixmap,
                    dst_width.saturating_sub(corner_radius),
                    0,
                    corner_radius,
                    corner_radius,
                );
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug)]
struct CachedPart {
    pixmap: Pixmap,
    scale: u32,
    active: bool,
    /// Whether it was drawn for round bottom corners, whose shadow reaches over the window.
    corners: bool,
}

impl CachedPart {
    fn new(
        dst_pixmap: &PixmapRef,
        rendered: &RenderedShadow,
        scale: u32,
        active: bool,
        part_idx: usize,
        corners: f32,
    ) -> CachedPart {
        #[allow(clippy::unwrap_used)]
        let mut pixmap = Pixmap::new(dst_pixmap.width(), dst_pixmap.height()).unwrap();
        rendered.draw(&mut pixmap.as_mut(), scale, part_idx, corners);

        CachedPart {
            pixmap,
            scale,
            active,
            corners: corners > 0.,
        }
    }

    fn matches(&self, dst_pixmap: &PixmapRef, dst_scale: u32, dst_active: bool, corners: bool) -> bool {
        self.pixmap.width() == dst_pixmap.width()
            && self.pixmap.height() == dst_pixmap.height()
            && self.scale == dst_scale
            && self.active == dst_active
            && self.corners == corners
    }

    fn draw(&self, dst_pixmap: &mut PixmapMut) {
        let src_data = self.pixmap.data();
        dst_pixmap.data_mut()[..src_data.len()].copy_from_slice(src_data);
    }
}

#[derive(Default, Debug)]
pub struct Shadow {
    part_cache: [Option<CachedPart>; 5],
    // (scale, active) -> RenderedShadow
    rendered: BTreeMap<(u32, bool), RenderedShadow>,
}

impl Shadow {
    /// Draws part `part_idx`'s shadow. `corners` is the radius, in the part's pixels, of the round
    /// bottom corners it is drawn for (zero for square ones): the bottom part reaches that far
    /// above the window's bottom edge, so with round corners its shadow starts at the part's top.
    /// For a side part, a `corners` above zero says the frame has no header: the part then starts
    /// at the window's own rounded top corner, as it ends at its bottom one.
    pub fn draw(&mut self, pixmap: &mut PixmapMut, scale: u32, active: bool, part_idx: usize, corners: f32) {
        let cache = &mut self.part_cache[part_idx];
        let round = corners > 0.;

        if let Some(cache_value) = cache {
            if !cache_value.matches(&pixmap.as_ref(), scale, active, round) {
                *cache = None;
            }
        }

        if cache.is_none() {
            let rendered = self
                .rendered
                .entry((scale, active))
                .or_insert_with(|| RenderedShadow::new(scale, active));

            *cache = Some(CachedPart::new(
                &pixmap.as_ref(),
                rendered,
                scale,
                active,
                part_idx,
                corners,
            ));
        }

        // We filled the cache above.
        #[allow(clippy::unwrap_used)]
        cache.as_ref().unwrap().draw(pixmap);
    }
}
