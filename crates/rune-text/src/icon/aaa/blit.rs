//! `SkScan_AAAPath.cpp`'s alpha arithmetic and blitters, into a mask.

use super::Mask;
use super::edge::{Fixed, MAX, ONE, ceil_to_fixed, ceil_to_int, fixed_mul, floor_to_fixed, floor_to_int, sat_add};

pub(super) fn get_partial_alpha_fixed(alpha: u8, partial: Fixed) -> u8 {
    ((alpha as i32 * partial + 0x8000) >> 16) as u8
}

pub(super) fn get_partial_alpha(alpha: u8, full: u8) -> u8 {
    ((alpha as i32 * full as i32) >> 8) as u8
}

pub(super) fn fixed_to_alpha(f: Fixed) -> u8 {
    get_partial_alpha_fixed(0xFF, f)
}

pub(super) fn trapezoid_to_alpha(l1: Fixed, l2: Fixed) -> u8 {
    (((l1 + l2) / 2) >> 8) as u8
}

pub(super) fn partial_triangle_to_alpha(a: Fixed, b: Fixed) -> u8 {
    let area = (a >> 11).wrapping_mul(a >> 11).wrapping_mul(b >> 11);
    ((area >> 8) & 0xFF) as u8
}

pub(super) fn safely_add(p: Option<&mut u8>, delta: u8) {
    if let Some(p) = p {
        *p = (*p as u32 + delta as u32).min(255) as u8;
    }
}

pub(super) fn blit_single_alpha(mask: &mut Mask, y: i32, x: i32, alpha: u8, full: u8, no_real: bool) {
    if full == 0xFF && !no_real {
        if let Some(p) = mask.at(y, x) {
            *p = alpha;
        }
    } else {
        safely_add(mask.at(y, x), get_partial_alpha(alpha, full));
    }
}

pub(super) fn blit_two_alphas(mask: &mut Mask, y: i32, x: i32, a1: u8, a2: u8) {
    safely_add(mask.at(y, x), a1);
    safely_add(mask.at(y, x + 1), a2);
}

pub(super) fn blit_full_alpha(mask: &mut Mask, y: i32, x: i32, len: i32, full: u8) {
    for i in 0..len {
        safely_add(mask.at(y, x + i), full);
    }
}

pub(super) fn compute_alpha_above_line(alphas: &mut [u8], l: Fixed, r: Fixed, dy: Fixed, full: u8) {
    let rr = ceil_to_int(r);
    if rr == 0 {
        return;
    }
    if rr == 1 {
        alphas[0] = get_partial_alpha(((((rr << 17) - l - r) >> 9) & 0xFF) as u8, full);
        return;
    }
    let first = ONE - l;
    let last = r - ((rr - 1) << 16);
    let first_h = fixed_mul(first, dy);
    alphas[0] = (fixed_mul(first, first_h) >> 9) as u8;
    let mut alpha16 = sat_add(first_h, dy >> 1);
    for a in alphas.iter_mut().take((rr - 1) as usize).skip(1) {
        *a = (alpha16 >> 8) as u8;
        alpha16 = sat_add(alpha16, dy);
    }
    alphas[(rr - 1) as usize] = full.wrapping_sub(partial_triangle_to_alpha(last, dy));
}

pub(super) fn compute_alpha_below_line(alphas: &mut [u8], l: Fixed, r: Fixed, dy: Fixed, full: u8) {
    let rr = ceil_to_int(r);
    if rr == 0 {
        return;
    }
    if rr == 1 {
        alphas[0] = get_partial_alpha(trapezoid_to_alpha(l, r), full);
        return;
    }
    let first = ONE - l;
    let last = r - ((rr - 1) << 16);
    let last_h = fixed_mul(last, dy);
    alphas[(rr - 1) as usize] = (fixed_mul(last, last_h) >> 9) as u8;
    let mut alpha16 = sat_add(last_h, dy >> 1);
    for i in (1..(rr - 1) as usize).rev() {
        alphas[i] = ((alpha16 >> 8) & 0xFF) as u8;
        alpha16 = sat_add(alpha16, dy);
    }
    alphas[0] = full.wrapping_sub(partial_triangle_to_alpha(first, dy));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn blit_aaa_trapezoid_row(mask: &mut Mask, y: i32, ul: Fixed, ur: Fixed, ll: Fixed, lr: Fixed, l_dy: Fixed, r_dy: Fixed, full: u8, no_real: bool) {
    let (l, r) = (floor_to_int(ul), ceil_to_int(lr));
    let len = r - l;
    if len == 1 {
        let alpha = trapezoid_to_alpha(ur - ul, lr - ll);
        blit_single_alpha(mask, y, l, alpha, full, no_real);
        return;
    }
    if len <= 0 {
        return;
    }
    let n = len as usize;
    let mut alphas = vec![full; n + 1];
    let mut temp = vec![0u8; n + 1];
    let (u_l, l_l) = (floor_to_int(ul), ceil_to_int(ll));
    if u_l + 2 == l_l {
        let first = (u_l << 16) + ONE - ul;
        let second = ll - ul - first;
        let a1 = full.wrapping_sub(partial_triangle_to_alpha(first, l_dy));
        let a2 = partial_triangle_to_alpha(second, l_dy);
        alphas[0] = alphas[0].saturating_sub(a1);
        alphas[1] = alphas[1].saturating_sub(a2);
    } else {
        let off = (u_l - l) as usize;
        compute_alpha_below_line(&mut temp[off..], ul - (u_l << 16), ll - (u_l << 16), l_dy, full);
        for i in u_l..l_l {
            let k = (i - l) as usize;
            alphas[k] = alphas[k].saturating_sub(temp[k]);
        }
    }
    let (u_r, l_r) = (floor_to_int(ur), ceil_to_int(lr));
    if u_r + 2 == l_r {
        let first = (u_r << 16) + ONE - ur;
        let second = lr - ur - first;
        let a1 = partial_triangle_to_alpha(first, r_dy);
        let a2 = full.wrapping_sub(partial_triangle_to_alpha(second, r_dy));
        alphas[n - 2] = alphas[n - 2].saturating_sub(a1);
        alphas[n - 1] = alphas[n - 1].saturating_sub(a2);
    } else {
        let off = (u_r - l) as usize;
        temp.iter_mut().for_each(|t| *t = 0);
        compute_alpha_above_line(&mut temp[off..], ur - (u_r << 16), lr - (u_r << 16), r_dy, full);
        for i in u_r..l_r {
            let k = (i - l) as usize;
            alphas[k] = alphas[k].saturating_sub(temp[k]);
        }
    }
    for (i, &a) in alphas.iter().take(n).enumerate() {
        safely_add(mask.at(y, l + i as i32), a);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn blit_trapezoid_row(mask: &mut Mask, y: i32, mut ul: Fixed, mut ur: Fixed, mut ll: Fixed, mut lr: Fixed, l_dy: Fixed, r_dy: Fixed, full: u8, no_real: bool) {
    if ul > ur {
        return;
    }
    if ll > lr {
        let (a, b, c, d) = (ul.min(ll), ul.max(ll), ur.min(lr), ur.max(lr));
        let x = ((a.max(c) as i64 + b.min(d) as i64) / 2) as i32;
        ll = x;
        lr = x;
    }
    if ul == ur && ll == lr {
        return;
    }
    if ul > ll {
        std::mem::swap(&mut ul, &mut ll);
    }
    if ur > lr {
        std::mem::swap(&mut ur, &mut lr);
    }
    let join_left = ceil_to_fixed(ll);
    let join_rite = floor_to_fixed(ur);
    if join_left <= join_rite {
        if ul < join_left {
            let len = ceil_to_int(join_left - ul);
            if len == 1 {
                let alpha = trapezoid_to_alpha(join_left - ul, join_left - ll);
                blit_single_alpha(mask, y, ul >> 16, alpha, full, no_real);
            } else if len == 2 {
                let first = join_left - ONE - ul;
                let second = ll - ul - first;
                let a1 = partial_triangle_to_alpha(first, l_dy);
                let a2 = full.wrapping_sub(partial_triangle_to_alpha(second, l_dy));
                blit_two_alphas(mask, y, ul >> 16, a1, a2);
            } else {
                blit_aaa_trapezoid_row(mask, y, ul, join_left, ll, join_left, l_dy, MAX, full, no_real);
            }
        }
        if join_left < join_rite {
            blit_full_alpha(mask, y, floor_to_int(join_left), floor_to_int(join_rite - join_left), full);
        }
        if lr > join_rite {
            let len = ceil_to_int(lr - join_rite);
            if len == 1 {
                let alpha = trapezoid_to_alpha(ur - join_rite, lr - join_rite);
                blit_single_alpha(mask, y, join_rite >> 16, alpha, full, no_real);
            } else if len == 2 {
                let first = join_rite + ONE - ur;
                let second = lr - ur - first;
                let a1 = full.wrapping_sub(partial_triangle_to_alpha(first, r_dy));
                let a2 = partial_triangle_to_alpha(second, r_dy);
                blit_two_alphas(mask, y, join_rite >> 16, a1, a2);
            } else {
                blit_aaa_trapezoid_row(mask, y, join_rite, ur, join_rite, lr, MAX, r_dy, full, no_real);
            }
        }
    } else {
        blit_aaa_trapezoid_row(mask, y, ul, ur, ll, lr, l_dy, r_dy, full, no_real);
    }
}
