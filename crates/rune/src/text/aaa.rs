//! Skia's analytic anti-aliasing scan converter, which Chromium's raster fills an icon's path
//! with (anything its rect op doesn't take: more than one subpath, a polyline, a curve), ported
//! step for step from `SkScan_AAAPath.cpp`, `SkAnalyticEdge.cpp` and `SkEdgeBuilder.cpp` so its
//! pixels come out as Chromium's do. Its coverage is the path's area once every edge end is
//! snapped to a quarter pixel in y (a curve's mid points to a whole pixel where it runs steeply),
//! computed in 16.16 fixed point with Skia's own shortcuts: x snapped to 1/16 inside a row,
//! triangles to 5 bits, a pixel two edges cross in one row set rather than added. Masks up to 32
//! pixels wide and 1024 in all take Skia's mask blitter; larger ones its run blitter, which pins
//! alphas within 8 of 0 or 255.
//!
//! What's left out: the convex walk (Skia takes it for a path it knows is convex; an icon's
//! stroked outline or multi-part fill never is), inverse fills and the edge clipper (the whole
//! path is covered and the canvas cut from it).

mod blit;
mod edge;

use tiny_skia_path::Path;

use blit::{blit_trapezoid_row, fixed_to_alpha};
use edge::{ACCURACY, Edge, Fixed, MAX, ONE, build_edges, ceil_to_fixed, ceil_to_int, floor_to_int};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Rule {
    NonZero,
    EvenOdd,
}

/// Covers `out` (`w` × `h`, row-major, cleared first) with `path` in device pixels.
pub(super) fn fill(path: &Path, rule: Rule, w: usize, h: usize, out: &mut [u8]) {
    out.iter_mut().for_each(|b| *b = 0);
    let b = path.bounds();
    let ir = [b.left().floor() as i32, b.top().floor() as i32, b.right().ceil() as i32, b.bottom().ceil() as i32];
    let clip = [ir[0].max(0), ir[1].max(0), ir[2].min(w as i32), ir[3].min(h as i32)];
    if clip[2] <= clip[0] || clip[3] <= clip[1] {
        return;
    }
    let mut edges = build_edges(path);
    if edges.len() <= 2 {
        return;
    }
    // Skia's mask blitter takes a region up to 32 wide and 1024 in all; its run blitter the rest.
    let (cw, ch) = (clip[2] - clip[0], clip[3] - clip[1]);
    let masked = cw <= 32 && ((cw + 3) & !3) * ch <= 1024;
    // Over the whole path, not just the canvas: Skia clips its edges to the canvas first, which
    // leaves the cover inside it as it was. Rows `iw` apart with a spare byte before the first
    // and after the last, as Skia's mask is: a write a pixel past a row's end lands on the next
    // row's first.
    let (iw, ih) = (ir[2] - ir[0], ir[3] - ir[1]);
    let mut mask = Mask { w: iw as usize, left: ir[0], top: ir[1], data: vec![0u8; (iw * ih + 2) as usize] };
    sort_and_link(&mut edges);
    let skip_intersect = path.points().len() > (ch * 2) as usize;
    walk_edges(&mut edges, rule, &mut mask, ir[1], ir[3], ir[0] << 16, ir[2] << 16, skip_intersect);
    for y in clip[1]..clip[3] {
        for x in clip[0]..clip[2] {
            let a = mask.data[1 + ((y - ir[1]) * iw + x - ir[0]) as usize];
            out[y as usize * w + x as usize] = if masked { a } else { snap_alpha(a) };
        }
    }
}

/// `RunBasedAdditiveBlitter::snapAlpha`.
fn snap_alpha(a: u8) -> u8 {
    if a > 247 {
        255
    } else if a < 8 {
        0
    } else {
        a
    }
}

pub(super) struct Mask {
    pub(super) w: usize,
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) data: Vec<u8>,
}

impl Mask {
    pub(super) fn at(&mut self, y: i32, x: i32) -> Option<&mut u8> {
        let i = 1 + (y - self.top) as i64 * self.w as i64 + (x - self.left) as i64;
        usize::try_from(i).ok().and_then(|i| self.data.get_mut(i))
    }
}

/// Sorts the edges after the sentinels by top, x and slope, and links them between the head
/// (0) and the tail (1).
fn sort_and_link(edges: &mut [Edge]) {
    let mut order: Vec<usize> = (2..edges.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&edges[a], &edges[b]);
        (a.upper_y, a.x, a.dx).cmp(&(b.upper_y, b.x, b.dx))
    });
    let mut prev = 0;
    for &i in &order {
        edges[prev].next = i;
        edges[i].prev = prev;
        prev = i;
    }
    edges[prev].next = 1;
    edges[1].prev = prev;
    edges[0].prev = usize::MAX;
    edges[1].next = usize::MAX;
}

fn remove_edge(e: &mut [Edge], i: usize) {
    let (p, n) = (e[i].prev, e[i].next);
    e[p].next = n;
    e[n].prev = p;
}

fn insert_edge_after(e: &mut [Edge], i: usize, after: usize) {
    let n = e[after].next;
    e[i].prev = after;
    e[i].next = n;
    e[n].prev = i;
    e[after].next = i;
}

fn has_prev(e: &[Edge], i: usize) -> bool {
    e[i].prev != usize::MAX
}

fn backward_insert_edge_based_on_x(e: &mut [Edge], i: usize) {
    let x = e[i].x;
    let mut prev = e[i].prev;
    while has_prev(e, prev) && e[prev].x > x {
        prev = e[prev].prev;
    }
    if e[prev].next != i {
        remove_edge(e, i);
        insert_edge_after(e, i, prev);
    }
}

fn backward_insert_start(e: &[Edge], mut prev: usize, x: Fixed) -> usize {
    while has_prev(e, prev) && e[prev].x > x {
        prev = e[prev].prev;
    }
    prev
}

fn update_next_next_y(y: Fixed, next_y: Fixed, nn: &mut Fixed) {
    if y > next_y && y < *nn {
        *nn = y;
    }
}

fn check_intersection(e: &[Edge], i: usize, next_y: Fixed, nn: &mut Fixed) {
    let p = e[i].prev;
    if has_prev(e, p) && e[p].x.wrapping_add(e[p].dx) > e[i].x.wrapping_add(e[i].dx) {
        *nn = next_y + (ONE >> ACCURACY);
    }
}

fn check_intersection_fwd(e: &[Edge], i: usize, next_y: Fixed, nn: &mut Fixed) {
    let n = e[i].next;
    if n != usize::MAX && e[n].next != usize::MAX && e[i].x.wrapping_add(e[i].dx) > e[n].x.wrapping_add(e[n].dx) {
        *nn = next_y + (ONE >> ACCURACY);
    }
}

fn insert_new_edges(e: &mut [Edge], mut new: usize, y: Fixed, nn: &mut Fixed) {
    if e[new].upper_y > y {
        update_next_next_y(e[new].upper_y, y, nn);
        return;
    }
    let prev = e[new].prev;
    if e[prev].x <= e[new].x {
        while e[new].upper_y <= y {
            check_intersection(e, new, y, nn);
            update_next_next_y(e[new].lower_y, y, nn);
            new = e[new].next;
        }
        update_next_next_y(e[new].upper_y, y, nn);
        return;
    }
    let mut start = backward_insert_start(e, prev, e[new].x);
    loop {
        let next = e[new].next;
        let mut placed = false;
        loop {
            if e[start].next == new {
                placed = true;
                break;
            }
            let after = e[start].next;
            if e[after].x >= e[new].x {
                break;
            }
            start = after;
        }
        if !placed {
            remove_edge(e, new);
            insert_edge_after(e, new, start);
        }
        check_intersection(e, new, y, nn);
        check_intersection_fwd(e, new, y, nn);
        update_next_next_y(e[new].lower_y, y, nn);
        start = new;
        new = next;
        if e[new].upper_y > y {
            break;
        }
    }
    update_next_next_y(e[new].upper_y, y, nn);
}

fn edges_too_close(e: &[Edge], prev: usize, next: usize, lower_y: Fixed) -> bool {
    next != usize::MAX && prev != usize::MAX && e[next].upper_y < lower_y && e[prev].x.saturating_add(ONE) >= e[next].x.saturating_sub(e[next].dx.saturating_abs())
}

fn edges_too_close_rite(prev_rite: i32, ul: Fixed, ll: Fixed) -> bool {
    prev_rite > floor_to_int(ul) || prev_rite > floor_to_int(ll)
}

/// `aaa_walk_edges`, for a non-inverse fill into a mask.
#[allow(clippy::too_many_arguments)]
fn walk_edges(e: &mut [Edge], rule: Rule, mask: &mut Mask, start_y: i32, stop_y: i32, left_clip: Fixed, right_clip: Fixed, skip_intersect: bool) {
    const HEAD: usize = 0;
    e[HEAD].x = left_clip;
    e[HEAD].upper_x = left_clip;
    e[1].x = right_clip;
    e[1].upper_x = right_clip;
    let mut y = e[e[HEAD].next].upper_y.max(start_y << 16);
    let mut nn = MAX;
    {
        let mut i = e[HEAD].next;
        while e[i].upper_y <= y {
            e[i].go_y(y);
            update_next_next_y(e[i].lower_y, y, &mut nn);
            i = e[i].next;
        }
        update_next_next_y(e[i].upper_y, y, &mut nn);
    }
    let winding_mask = if rule == Rule::EvenOdd { 1 } else { -1 };
    // Each row advances y by at least a quarter pixel; this only stops a malformed list.
    for _ in 0..(stop_y - start_y + 2) * 64 {
        let mut w = 0;
        let mut in_interval = false;
        let mut prev_x = e[HEAD].x;
        let mut next_y = nn.min(ceil_to_fixed(y + 1));
        let mut curr = e[HEAD].next;
        let mut left_e = HEAD;
        let mut left = left_clip;
        let mut left_dy = 0;
        let mut prev_rite = floor_to_int(left_clip);
        nn = MAX;
        let mut y_shift = 0;
        if (next_y - y) & (ONE >> 2) != 0 {
            y_shift = 2;
            next_y = y + (ONE >> 2);
        } else if (next_y - y) & (ONE >> 1) != 0 {
            y_shift = 1;
        }
        let full_alpha = fixed_to_alpha(next_y - y);
        let row = floor_to_int(y);
        while e[curr].upper_y <= y {
            w += e[curr].winding;
            let prev_in = in_interval;
            in_interval = (w & winding_mask) != 0;
            let is_left = in_interval && !prev_in;
            let is_rite = !in_interval && prev_in;
            if is_rite {
                let rite = e[curr].x;
                e[curr].go_y_shift(next_y, y_shift);
                let next_left = left_clip.max(e[left_e].x);
                let rite = right_clip.min(rite);
                let next_rite = right_clip.min(e[curr].x);
                let close = full_alpha == 0xFF
                    && (edges_too_close_rite(prev_rite, left, e[left_e].x) || edges_too_close(e, curr, e[curr].next, next_y));
                blit_trapezoid_row(mask, row, left, rite, next_left, next_rite, left_dy, e[curr].dy, full_alpha, close);
                prev_rite = ceil_to_int(rite.max(e[curr].x));
            } else {
                if is_left {
                    left = e[curr].x.max(left_clip);
                    left_dy = e[curr].dy;
                    left_e = curr;
                }
                e[curr].go_y_shift(next_y, y_shift);
            }
            let next = e[curr].next;
            while e[curr].lower_y <= next_y {
                if e[curr].curve_count == 0 {
                    break;
                }
                e[curr].keep_continuous();
                if !e[curr].update() {
                    break;
                }
            }
            if e[curr].lower_y <= next_y {
                remove_edge(e, curr);
            } else {
                update_next_next_y(e[curr].lower_y, next_y, &mut nn);
                let new_x = e[curr].x;
                if new_x < prev_x {
                    backward_insert_edge_based_on_x(e, curr);
                } else {
                    prev_x = new_x;
                }
                if !skip_intersect {
                    check_intersection(e, curr, next_y, &mut nn);
                }
            }
            curr = next;
        }
        if in_interval {
            let close = full_alpha == 0xFF && edges_too_close(e, e[left_e].prev, left_e, next_y);
            blit_trapezoid_row(mask, row, left, right_clip, left_clip.max(e[left_e].x), right_clip, left_dy, 0, full_alpha, close);
        }
        y = next_y;
        if y >= stop_y << 16 {
            break;
        }
        insert_new_edges(e, curr, y, &mut nn);
    }
}
