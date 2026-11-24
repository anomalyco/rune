//! Uniform 2D CSR spatial grid over axis-aligned boxes `[x0, y0, x1, y1]`, used by
//! [`super::route`] to bound obstacle clearance queries on large layered diagrams.

#[inline]
pub(super) fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

/// Uniform 2D CSR grid over axis-aligned boxes `[x0, y0, x1, y1]`.
pub(super) struct Grid {
    min: [f32; 2],
    max: [f32; 2],
    inv_cell: [f32; 2],
    nx: usize,
    ny: usize,
    offsets: Vec<usize>,
    entries: Vec<usize>,
}

impl Grid {
    pub(super) fn new(boxes: &[[f32; 4]]) -> Self {
        Self::new_mapped(boxes, |b| *b)
    }

    pub(super) fn new_mapped<T>(items: &[T], rect: impl Fn(&T) -> [f32; 4]) -> Self {
        if items.is_empty() {
            return Grid { min: [0.0; 2], max: [0.0; 2], inv_cell: [1.0; 2], nx: 0, ny: 0, offsets: Vec::new(), entries: Vec::new() };
        }
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for it in items {
            let b = rect(it);
            if b.iter().all(|v| v.is_finite()) {
                min = [min[0].min(b[0]), min[1].min(b[1])];
                max = [max[0].max(b[2]), max[1].max(b[3])];
            }
        }
        if min[0] > max[0] {
            return Grid { min: [0.0; 2], max: [0.0; 2], inv_cell: [1.0; 2], nx: 0, ny: 0, offsets: Vec::new(), entries: Vec::new() };
        }
        let w = (max[0] - min[0]).max(1.0);
        let h = (max[1] - min[1]).max(1.0);
        let nx = ((w / 128.0).ceil() as usize).clamp(1, 64);
        let ny = ((h / 128.0).ceil() as usize).clamp(1, 128);
        let inv_cell = [1.0 / (w / nx as f32).max(1.0), 1.0 / (h / ny as f32).max(1.0)];
        let cells = nx * ny;
        let range = |b: [f32; 4]| {
            let cx0 = (((b[0] - min[0]) * inv_cell[0]).floor() as isize).clamp(0, nx as isize - 1) as usize;
            let cx1 = (((b[2] - min[0]) * inv_cell[0]).floor() as isize).clamp(0, nx as isize - 1) as usize;
            let cy0 = (((b[1] - min[1]) * inv_cell[1]).floor() as isize).clamp(0, ny as isize - 1) as usize;
            let cy1 = (((b[3] - min[1]) * inv_cell[1]).floor() as isize).clamp(0, ny as isize - 1) as usize;
            (cx0, cx1, cy0, cy1)
        };
        let mut counts = vec![0usize; cells];
        for it in items {
            let b = rect(it);
            if !b.iter().all(|v| v.is_finite()) {
                continue;
            }
            let (cx0, cx1, cy0, cy1) = range(b);
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    counts[cy * nx + cx] += 1;
                }
            }
        }
        let mut offsets = vec![0usize; cells + 1];
        for i in 0..cells {
            offsets[i + 1] = offsets[i] + counts[i];
        }
        let mut entries = vec![0usize; offsets[cells]];
        let mut cursor = offsets[..cells].to_vec();
        for (idx, it) in items.iter().enumerate() {
            let b = rect(it);
            if !b.iter().all(|v| v.is_finite()) {
                continue;
            }
            let (cx0, cx1, cy0, cy1) = range(b);
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    let cell = cy * nx + cx;
                    entries[cursor[cell]] = idx;
                    cursor[cell] += 1;
                }
            }
        }
        Grid { min, max, inv_cell, nx, ny, offsets, entries }
    }

    pub(super) fn query(&self, q: [f32; 4], mut f: impl FnMut(usize)) {
        if self.nx == 0 || self.ny == 0 || q[2] < self.min[0] || q[0] > self.max[0] || q[3] < self.min[1] || q[1] > self.max[1] {
            return;
        }
        let cx0 = (((q[0] - self.min[0]) * self.inv_cell[0]).floor() as isize).clamp(0, self.nx as isize - 1) as usize;
        let cx1 = (((q[2] - self.min[0]) * self.inv_cell[0]).floor() as isize).clamp(0, self.nx as isize - 1) as usize;
        let cy0 = (((q[1] - self.min[1]) * self.inv_cell[1]).floor() as isize).clamp(0, self.ny as isize - 1) as usize;
        let cy1 = (((q[3] - self.min[1]) * self.inv_cell[1]).floor() as isize).clamp(0, self.ny as isize - 1) as usize;
        for cy in cy0..=cy1 {
            let row = cy * self.nx;
            for cx in cx0..=cx1 {
                let cell = row + cx;
                for &idx in &self.entries[self.offsets[cell]..self.offsets[cell + 1]] {
                    f(idx);
                }
            }
        }
    }
}
