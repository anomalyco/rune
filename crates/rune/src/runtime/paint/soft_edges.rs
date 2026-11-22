//! Soft clip edges: how a fade inherits down the tree, and whether a rect reaches into one.

/// Device-pixel extent of a tagged fade width (`< 0` = linear, `> STOP_FADE_BIAS` = explicit stops).
#[inline]
pub(in crate::runtime) fn fade_extent(w: f32) -> f32 {
    if w > crate::render::STOP_FADE_BIAS {
        w - crate::render::STOP_FADE_BIAS
    } else {
        w.abs()
    }
}

// Carry the declaring edge with the inherited width. Descendant clipping must not restart
// its ramp; an inner fade takes over once the ancestor's ramp is fully opaque there. Widths
// compare by size: a negative one is a linear ramp (`El::clip_fade_linear`), and one above
// `STOP_FADE_BIAS` uses explicit stops (`ScrollFade::stops`).
pub(in crate::runtime) fn inherit_fade(width: &mut [f32; 4], origin: &mut [f32; 4], rect: [f32; 4], own: [f32; 4]) {
    for edge in 0..4 {
        let inset = if edge < 2 { rect[edge] - origin[edge] } else { origin[edge] - rect[edge] };
        let (o, w) = (fade_extent(own[edge]), fade_extent(width[edge]));
        if o > 0.0 && (o >= w || inset >= w) {
            width[edge] = own[edge];
            origin[edge] = rect[edge];
        }
    }
}

/// Whether `r` (x0, y0, x1, y1) reaches into a soft edge of `clip`: one fading over `width`
/// from `origin`, edge by edge (left, top, right, bottom).
pub(in crate::runtime) fn in_fade(r: [f32; 4], clip: [f32; 4], width: [f32; 4], origin: [f32; 4]) -> bool {
    let width = width.map(fade_extent);
    (width[0] > 0.0 && r[0] < origin[0] + width[0] && r[2] > clip[0])
        || (width[1] > 0.0 && r[1] < origin[1] + width[1] && r[3] > clip[1])
        || (width[2] > 0.0 && r[2] > origin[2] - width[2] && r[0] < clip[2])
        || (width[3] > 0.0 && r[3] > origin[3] - width[3] && r[1] < clip[3])
}
