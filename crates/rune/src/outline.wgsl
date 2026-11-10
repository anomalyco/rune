// The Windfoil variant of the uber shader (render/variant.rs): shader.wgsl with glyphs that
// draw from their outlines (`text::Outline`, `radii.w > 0`) as well as from the atlas.

// Glyph outlines (text/outline.rs): banded records of xy-monotone quadratics in font units.
@group(0) @binding(4) var outlines: texture_2d<u32>;

// Glyph coverage from outlines: Windfoil's closed-form box filter (DesLauriers 2026; the
// integrals are texel-org/windfoil-algorithm's, Apache-2.0) over a record's pieces, each moved
// by CoreGraphics' dilation and y autohint first (text/outline/build.rs has the record layout).

fn outline_texel(i: u32) -> vec4<u32> {
    return textureLoad(outlines, vec2<i32>(i32(i & 1023u), i32(i >> 10u)), 0);
}

// The move of a piece's code in radii: x in -1..1, y in 0..2.
fn outline_move(code: u32) -> vec2<f32> {
    return vec2<f32>(f32(code & 3u) - 1.0, f32((code >> 2u) & 3u));
}

// CoreGraphics' y autohint (`HintTransform::apply`): values [-d, lo, x, xt] [middle, mt, cap, ct]
// [s1, s2, s3].
fn outline_hint(y: f32, a: vec4<f32>, b: vec4<f32>, c: vec4<f32>) -> f32 {
    if (y < a.y) { return y; }
    if (y <= a.z) { return (y - a.y) * c.x + a.y; }
    if (y <= b.x) { return (y - a.z) * c.y + a.w; }
    if (y <= b.z) { return y + (b.y - b.x); }
    return (y - b.z) * c.z + b.w;
}

fn outline_inside(a2: vec2<f32>, a1: vec2<f32>, x0: f32, ta: f32, tb: f32, hx: f32) -> f32 {
    if (tb <= ta) { return 0.0; }
    let tm = 0.5 * (ta + tb);
    let d = 0.5 * (tb - ta);
    let x_mid = (a2.x * tm + a1.x) * tm + x0 + hx;
    let dmid = 2.0 * a2 * tm + a1;
    return 2.0 * d * x_mid * dmid.y + (2.0 * d * d * d / 3.0) * (a2.x * dmid.y + 2.0 * a2.y * dmid.x);
}

// The box-filtered winding of one monotone piece (relative to the pixel centre) between rows
// `lo` and `hi`, `2 * hx` wide.
fn outline_integrate(q1: vec2<f32>, q2: vec2<f32>, q3: vec2<f32>, lo: f32, hi: f32, hx: f32) -> f32 {
    let a2 = q1 - 2.0 * q2 + q3;
    let a1 = 2.0 * (q2 - q1);
    let y_rising = q3.y >= q1.y;
    let x_rising = q3.x >= q1.x;
    let A2 = a2.yyxx;
    let A1 = a1.yyxx;
    let V = vec4<f32>(select(vec2<f32>(hi, lo), vec2<f32>(lo, hi), y_rising), -hx, hx);
    let R = vec2<bool>(y_rising, x_rising).xxyy;
    let SG = select(vec4<f32>(-1.0), vec4<f32>(1.0), R);
    let C = q1.yyxx - V;
    let sat0 = (C * SG) >= vec4<f32>(0.0);
    let sat1 = ((q3.yyxx - V) * SG) <= vec4<f32>(0.0);
    let disc = max(A1 * A1 - 4.0 * A2 * C, vec4<f32>(0.0));
    let sq = sqrt(disc);
    let qq = -0.5 * (A1 + select(-sq, sq, A1 >= vec4<f32>(0.0)));
    let use_r1 = (A1 < vec4<f32>(0.0)) == R;
    let num = select(C, qq, use_r1);
    let den = select(qq, A2, use_r1);
    let valid = den != vec4<f32>(0.0);
    let t_raw = clamp(select(vec4<f32>(0.0), num / select(vec4<f32>(1.0), den, valid), valid), vec4<f32>(0.0), vec4<f32>(1.0));
    let T = select(select(t_raw, vec4<f32>(1.0), sat1), vec4<f32>(0.0), sat0);
    if (T.y <= T.x) { return 0.0; }
    let tc = clamp(T.zw, T.xx, T.yy);
    let t1 = select(tc.y, tc.x, x_rising);
    let t2 = max(select(tc.x, tc.y, x_rising), t1);
    var acc = outline_inside(a2, a1, q1.x, t1, t2, hx);
    let rab = select(vec2<f32>(T.x, t1), vec2<f32>(t2, T.y), x_rising);
    let d = max(rab.y - rab.x, 0.0);
    let tm = 0.5 * (rab.x + rab.y);
    acc = acc + d * (2.0 * a2.y * tm + a1.y) * (2.0 * hx);
    return acc;
}

// How much of a box `2 * hx` wide and `2 * hw` tall (rest px) at `local` (rest px from the pen,
// y up) the glyph of record `rec` covers, `scale` rest px a font unit, dilated by `r` rest px and
// hinted by record `hint` (0: none). A pixel is `hx == hw`; a subpixel a third as wide.
fn outline_coverage(rec: u32, hint: u32, local: vec2<f32>, scale: f32, r: f32, hw: f32, hx: f32) -> f32 {
    let head = outline_texel(rec);
    let count = head.y;
    let y0 = bitcast<f32>(head.z);
    let inv_h = bitcast<f32>(head.w);
    let hinted = hint != 0u;
    var ha = vec4<f32>(0.0);
    var hb = vec4<f32>(0.0);
    var hc = vec4<f32>(0.0);
    if (hinted) {
        ha = bitcast<vec4<f32>>(outline_texel(hint));
        hb = bitcast<vec4<f32>>(outline_texel(hint + 1u));
        hc = bitcast<vec4<f32>>(outline_texel(hint + 2u));
    }
    // Pieces from as far as 2r below move up into this row; the hint moves them up to its reach.
    let reach = select(0.0, OUTLINE_HINT_REACH, hinted);
    var b0 = 0u;
    var b1 = 0u;
    if (count > 1u) {
        let last = f32(count - 1u);
        b0 = u32(clamp(floor(((local.y - hw - 2.0 * r - reach) / scale - y0) * inv_h), 0.0, last));
        b1 = u32(clamp(floor(((local.y + hw + reach) / scale - y0) * inv_h), 0.0, last));
    }
    var acc = 0.0;
    for (var band = b0; band <= b1; band = band + 1u) {
        // A piece counts within its band's rows, moved as it moves; a connector lies in one band.
        var e0 = -3.0e38;
        var e1 = 3.0e38;
        if (count > 1u) {
            e0 = (y0 + f32(band) / inv_h) * scale;
            e1 = (y0 + f32(band + 1u) / inv_h) * scale;
        }
        let entry = outline_texel(head.x + band);
        let sorted = entry.y > 4u;
        for (var k = 0u; k < entry.y; k = k + 1u) {
            let t0 = outline_texel(entry.x + 2u * k);
            let t1 = outline_texel(entry.x + 2u * k + 1u);
            // Sorted by right edge: once one sits left of the pixel, all the rest do.
            if (bitcast<f32>(t1.w) * scale + r - local.x <= -hx) {
                if (sorted) { break; }
                continue;
            }
            let code = t1.z;
            let oa = outline_move(code & 15u) * r;
            let ob = outline_move((code >> 4u) & 15u) * r;
            var p0 = bitcast<vec2<f32>>(t0.xy) * scale + oa;
            var p1 = bitcast<vec2<f32>>(t0.zw) * scale + oa;
            var p2 = bitcast<vec2<f32>>(t1.xy) * scale + ob;
            var wlo = -hw;
            var whi = hw;
            var be0 = e0 + oa.y;
            var be1 = e1 + oa.y;
            if (hinted) {
                p0.y = outline_hint(p0.y, ha, hb, hc);
                p1.y = outline_hint(p1.y, ha, hb, hc);
                p2.y = outline_hint(p2.y, ha, hb, hc);
                be0 = outline_hint(be0, ha, hb, hc);
                be1 = outline_hint(be1, ha, hb, hc);
            }
            if ((code & 256u) != 0u) {
                p1 = (p0 + p2) * 0.5;
            } else {
                wlo = max(wlo, be0 - local.y);
                whi = min(whi, be1 - local.y);
            }
            let q1 = p0 - local;
            let q3 = p2 - local;
            let lo = max(wlo, min(q1.y, q3.y));
            let hi = min(whi, max(q1.y, q3.y));
            if (hi <= lo || max(q1.x, q3.x) <= -hx) { continue; }
            if (min(q1.x, q3.x) >= hx) {
                acc = acc + 2.0 * hx * (clamp(q3.y, wlo, whi) - clamp(q1.y, wlo, whi));
                continue;
            }
            acc = acc + outline_integrate(q1, p1 - local, q3, lo, hi, hx);
        }
    }
    return min(abs(acc) / (4.0 * hx * hw), 1.0);
}

const OUTLINE_HINT_REACH: f32 = 1.5;

// Where along monotone quadratic `p0 p1 p2`'s axis `a` (0: x, 1: y) it reaches `c`, from 0 to 1.
fn outline_root(p0: f32, p1: f32, p2: f32, c: f32) -> f32 {
    let a2 = p0 - 2.0 * p1 + p2;
    let a1 = 2.0 * (p1 - p0);
    let a0 = p0 - c;
    if (abs(a2) < 1.0e-6) {
        return clamp(-a0 / select(a1, 1.0e-6, abs(a1) < 1.0e-6), 0.0, 1.0);
    }
    let sq = sqrt(max(a1 * a1 - 4.0 * a2 * a0, 0.0));
    let t0 = (-a1 - sq) / (2.0 * a2);
    let t1 = (-a1 + sq) / (2.0 * a2);
    return clamp(select(t1, t0, t0 >= -1.0e-4 && t0 <= 1.0 + 1.0e-4), 0.0, 1.0);
}

// Slug's coverage (Lengyel 2017): the winding along a horizontal and a vertical ray through the
// pixel's centre, each crossing spread over the pixel's width, the two averaged. Same record,
// moves and hint as `outline_coverage`. The vertical ray is the centre's winding (from the
// horizontal one) changed by the crossings within the pixel's height.
fn outline_slug(rec: u32, hint: u32, local: vec2<f32>, scale: f32, r: f32, hw: f32) -> f32 {
    let head = outline_texel(rec);
    let count = head.y;
    let y0 = bitcast<f32>(head.z);
    let inv_h = bitcast<f32>(head.w);
    let hinted = hint != 0u;
    var ha = vec4<f32>(0.0);
    var hb = vec4<f32>(0.0);
    var hc = vec4<f32>(0.0);
    if (hinted) {
        ha = bitcast<vec4<f32>>(outline_texel(hint));
        hb = bitcast<vec4<f32>>(outline_texel(hint + 1u));
        hc = bitcast<vec4<f32>>(outline_texel(hint + 2u));
    }
    // Only the bands about the pixel's rows hold what either ray meets within it.
    let reach = select(0.0, OUTLINE_HINT_REACH, hinted);
    var b0 = 0u;
    var b1 = 0u;
    if (count > 1u) {
        let last = f32(count - 1u);
        b0 = u32(clamp(floor(((local.y - hw - 2.0 * r - reach) / scale - y0) * inv_h), 0.0, last));
        b1 = u32(clamp(floor(((local.y + hw + reach) / scale - y0) * inv_h), 0.0, last));
    }
    var across = 0.0;
    var winding = 0.0;
    var down = 0.0;
    for (var band = b0; band <= b1; band = band + 1u) {
        var e0 = -3.0e38;
        var e1 = 3.0e38;
        if (count > 1u) {
            e0 = (y0 + f32(band) / inv_h) * scale;
            e1 = (y0 + f32(band + 1u) / inv_h) * scale;
        }
        let entry = outline_texel(head.x + band);
        for (var k = 0u; k < entry.y; k = k + 1u) {
            let t0 = outline_texel(entry.x + 2u * k);
            let t1 = outline_texel(entry.x + 2u * k + 1u);
            let code = t1.z;
            let oa = outline_move(code & 15u) * r;
            let ob = outline_move((code >> 4u) & 15u) * r;
            var p0 = bitcast<vec2<f32>>(t0.xy) * scale + oa;
            var p1 = bitcast<vec2<f32>>(t0.zw) * scale + oa;
            var p2 = bitcast<vec2<f32>>(t1.xy) * scale + ob;
            var be0 = e0 + oa.y;
            var be1 = e1 + oa.y;
            if (hinted) {
                p0.y = outline_hint(p0.y, ha, hb, hc);
                p1.y = outline_hint(p1.y, ha, hb, hc);
                p2.y = outline_hint(p2.y, ha, hb, hc);
                be0 = outline_hint(be0, ha, hb, hc);
                be1 = outline_hint(be1, ha, hb, hc);
            }
            // A connector lies in one band; a piece counts only within its band's rows.
            if ((code & 256u) != 0u) {
                p1 = (p0 + p2) * 0.5;
                be0 = -3.0e38;
                be1 = 3.0e38;
            }
            let q0 = p0 - local;
            let q1 = p1 - local;
            let q2 = p2 - local;
            let ylo = max(min(q0.y, q2.y), be0 - local.y);
            let yhi = min(max(q0.y, q2.y), be1 - local.y);
            // Horizontal ray: where the piece crosses the centre's row, spread across the pixel;
            // and the centre's winding, which the vertical ray starts from.
            if (q0.y != q2.y && ylo <= 0.0 && 0.0 < yhi) {
                let t = outline_root(q0.y, q1.y, q2.y, 0.0);
                let x = mix(mix(q0.x, q1.x, t), mix(q1.x, q2.x, t), t);
                let s = sign(q2.y - q0.y);
                across = across + s * clamp(x / (2.0 * hw) + 0.5, 0.0, 1.0);
                winding = winding + s * select(0.0, 1.0, x > 0.0);
            }
            // Vertical ray: only crossings within the pixel's height change it from the centre's
            // winding, each counted in its band's rows.
            if (q0.x != q2.x && min(q0.x, q2.x) <= 0.0 && 0.0 < max(q0.x, q2.x)) {
                let t = outline_root(q0.x, q1.x, q2.x, 0.0);
                let y = mix(mix(q0.y, q1.y, t), mix(q1.y, q2.y, t), t);
                if (abs(y) < hw && y >= be0 - local.y && y < be1 - local.y) {
                    down = down - sign(q2.x - q0.x) * (clamp(y / (2.0 * hw) + 0.5, 0.0, 1.0) - select(0.0, 1.0, y > 0.0));
                }
            }
        }
    }
    return 0.5 * (min(abs(across), 1.0) + min(abs(winding + down), 1.0));
}

// The subpixel coverage (red, green, blue; w = 1) of the glyph this fragment draws, or zero
// for any other: the dual-source entry (render/variant.rs) blends each channel by its own.
var<private> lcd_cov: vec4<f32>;

// A glyph instance drawn from its outline (`text::Outline`): radii = pen, scale, zoom; border =
// record, hint, radius, coverage (0: Windfoil's box, 1: Slug's rays, 2: Windfoil's box about
// each subpixel, 3: one subpixel per texel, filling a subpixel mask three texels a pixel). Rest px,
// y up, a device pixel 1 / zoom of them across.
fn outline_glyph(i: Out, screen: vec2<f32>) -> f32 {
    let z = i.radii.w;
    let rec = u32(i.border.x);
    let hint = u32(i.border.y);
    let hw = 0.5 / z;
    let mode = u32(i.border.w + 0.5);
    // A subpixel mask's texels are thirds of a pixel; its boxes are still a pixel wide.
    let across = select(1.0, 3.0, mode == 3u);
    let local = vec2<f32>((screen.x - i.radii.x) / across, i.radii.y - screen.y) / z;
    if (mode == 1u) {
        return outline_slug(rec, hint, local, i.radii.z, i.border.z, hw);
    }
    if (mode == 2u && OUTLINE_SUBPIXEL) {
        return outline_rgb(rec, hint, local, i.radii.z, i.border.z, hw);
    }
    return outline_coverage(rec, hint, local, i.radii.z, i.border.z, hw, hw);
}

// Whether this shader blends subpixel text by channel: false here, so a subpixel glyph draws
// its grayscale coverage; the subpixel variant (render/variant.rs) sets it.
const OUTLINE_SUBPIXEL: bool = false;

// A subpixel glyph's three coverages, red to blue, each a pixel-wide box about its subpixel's
// centre (FreeType's light LCD filter, thirds, which keeps the colour fringes faint).
fn outline_rgb(rec: u32, hint: u32, local: vec2<f32>, scale: f32, r: f32, hw: f32) -> f32 {
    let third = vec2<f32>(2.0 * hw / 3.0, 0.0);
    let c = vec3<f32>(
        outline_coverage(rec, hint, local - third, scale, r, hw, hw),
        outline_coverage(rec, hint, local, scale, r, hw, hw),
        outline_coverage(rec, hint, local + third, scale, r, hw, hw));
    lcd_cov = vec4<f32>(c, 1.0);
    return (c.x + c.y + c.z) / 3.0;
}

// A subpixel atlas mask (`radii.x == 1`): three texels a pixel, red to blue.
fn lcd_mask(i: Out, screen: vec2<f32>) -> f32 {
    let p = floor(screen - i.rect.xy);
    let t = vec2<i32>(i.uv.xy) + vec2<i32>(i32(p.x) * 3, i32(p.y));
    let c = vec3<f32>(textureLoad(atlas, t, 0).r, textureLoad(atlas, t + vec2<i32>(1, 0), 0).r, textureLoad(atlas, t + vec2<i32>(2, 0), 0).r);
    lcd_cov = vec4<f32>(c, 1.0);
    return (c.x + c.y + c.z) / 3.0;
}
