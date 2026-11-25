//! Horizontal column windowing and frozen leading columns for wide [`Table`](super::Table)s.
//!
//! When [`Table::scroll_wide`](super::Table::scroll_wide) and
//! [`Table::virtual_columns`](super::Table::virtual_columns) are enabled and the visible columns'
//! total width exceeds the viewport width, every column resolves to its exact fixed or minimum
//! width. [`ColumnPlan`] partitions the visible columns into:
//!
//! - Leading frozen columns (`0..frozen`), always mounted with `El::sticky_left(col_x)`.
//! - A horizontal window of non-frozen columns covering the viewport plus overscan on each side,
//!   together with any non-frozen column currently pinned by resize, reorder, or keyboard focus
//!   (including caret browsing).
//! - Fixed-width `a11y_hidden` spacers filling the exact pixel spans of unmounted columns so total
//!   scrollable width and every mounted column's x-position stay identical to a non-virtualised
//!   wide table.
//!
//! Horizontal scroll offsets are observed via `content`'s `on_layout_tx` and checked against
//! [`BandParams`]: offsets staying within the hysteresis band call `Tx::live_only()` so vertical
//! scrolls and small sideways scrolls trigger zero `view` rebuilds.

use rune::{Color, El, Shadow, Tx, el};

use super::{TableColumn, TableState, Width};

/// Assumed viewport width before `viewport` has laid out on frame 0 so very wide tables avoid
/// building every column on launch while narrow tables still mount all columns normally.
const INITIAL_VIEW_W: f32 = 1280.0;
/// Overscan added to each side of the horizontal viewport, as a fraction of viewport width.
const OVERSCAN_RATIO: f32 = 0.5;
/// Minimum horizontal overscan on each side in points.
const MIN_OVERSCAN: f32 = 200.0;
/// Fraction of overscan used as the edge hysteresis trigger (`margin = overscan * (1 - EDGE_RATIO)`).
const EDGE_RATIO: f32 = 0.5;

/// Horizontal column windowing state owned by [`TableState`](super::TableState).
#[derive(Default)]
pub(super) struct ColumnWindow {
    /// Viewport left edge in window points (updated from `viewport`'s `on_layout_tx`).
    /// Never read by `Table::build`, so updating it inside a `live_only` callback never causes
    /// stale-view warnings.
    viewport_x: f32,
    /// Committed horizontal scroll offset the current column window was built around.
    /// Only updated when `scroll_x` leaves the active [`BandParams`] (which triggers a commit).
    anchor_x: f32,
    /// Column id that currently holds keyboard focus (or caret browsing) inside a body cell,
    /// pinned so horizontal scrolling never unmounts the focused cell.
    focused_column: Option<String>,
}

impl ColumnWindow {
    pub(super) fn set_viewport_x(&mut self, x: f32) {
        self.viewport_x = x;
    }

    pub(super) fn anchor_x(&self) -> f32 {
        self.anchor_x
    }

    pub(super) fn focused_column(&self) -> Option<&str> {
        self.focused_column.as_deref()
    }

    /// Updates the focused column pin from a cell's `on_focus_within_change`.
    /// Calls `tx.live_only()` whenever the change does not alter the set of mounted columns.
    pub(super) fn set_focused_column(
        &mut self,
        id: &str,
        within: bool,
        in_base_window: bool,
        tx: &mut Tx,
    ) {
        if within {
            if self.focused_column.as_deref() == Some(id) || in_base_window {
                self.focused_column = Some(id.to_owned());
                tx.live_only();
            } else {
                self.focused_column = Some(id.to_owned());
            }
        } else if self.focused_column.as_deref() == Some(id) {
            self.focused_column = None;
            if in_base_window {
                tx.live_only();
            }
        } else {
            tx.live_only();
        }
    }

    /// Observes `content`'s laid-out `x` coordinate inside `viewport`. Calls `tx.live_only()`
    /// while `scroll_x` stays inside `band`; otherwise updates `anchor_x` and commits instantly
    /// so the column window re-centres around the new scroll position.
    pub(super) fn observe_content_x(&mut self, content_x: f32, band: BandParams, tx: &mut Tx) {
        let max_scroll = (band.total - band.effective_w).max(0.0);
        let scroll_x = (self.viewport_x - content_x).clamp(0.0, max_scroll);
        if band.contains(scroll_x) {
            tx.live_only();
        } else {
            self.anchor_x = scroll_x;
            tx.instant();
        }
    }
}

/// Safe horizontal scroll band for the currently mounted column window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BandParams {
    pub(super) total: f32,
    pub(super) frozen_w: f32,
    pub(super) effective_w: f32,
    pub(super) anchor_x: f32,
    pub(super) win_start_x: f32,
    pub(super) win_end_x: f32,
    pub(super) at_start: bool,
    pub(super) at_end: bool,
    pub(super) margin: f32,
}

impl BandParams {
    pub(super) fn contains(self, scroll_x: f32) -> bool {
        if (scroll_x - self.anchor_x).abs() < 1.0 {
            return true;
        }
        let left_ok = self.at_start || self.win_start_x <= scroll_x + self.frozen_w - self.margin + 0.5;
        let right_ok = self.at_end || self.win_end_x >= scroll_x + self.effective_w + self.margin - 0.5;
        left_ok && right_ok
    }
}

/// One emitted child of a table header row or body row.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ColumnItem {
    /// Fixed-width empty spacer preserving exact total scroll width and column x-positions.
    Spacer { key: String, width: f32 },
    /// Visible column `index` (`0..cols.len()`).
    Col {
        index: usize,
        /// When `Some(x)`, this column is frozen and pinned `x` points from the viewport's left edge.
        sticky_left: Option<f32>,
        /// True for the rightmost frozen column (draws the stuck right divider).
        frozen_last: bool,
        /// True when this column is mounted even without focus pinning (frozen or inside the base
        /// horizontal window), so focusing or blurring it needs no column-window rebuild.
        in_base_window: bool,
    },
}

/// Resolved column mounting plan for a single build of [`Table`](super::Table).
#[derive(Clone, Debug)]
pub(super) struct ColumnPlan {
    pub(super) items: Vec<ColumnItem>,
    pub(super) total_width: f32,
    pub(super) frozen_active: bool,
    pub(super) windowing_active: bool,
    pub(super) grid_coords: bool,
    pub(super) band: Option<BandParams>,
}

/// Resolved non-flex width of a column when a wide table overflows its viewport.
pub(super) fn resolved_min_width(c: &TableColumn, w: Width) -> f32 {
    match w {
        Width::Fixed(fw) => c.clamp(fw),
        Width::Flex(_) => c.min,
    }
}

/// Alpha-composites `top` over opaque `base` so frozen cells never bleed scrolled cells through.
pub(super) fn opaque_over(base: Color, top: Color) -> Color {
    let a = top.a.clamp(0.0, 1.0);
    Color {
        r: base.r + (top.r - base.r) * a,
        g: base.g + (top.g - base.g) * a,
        b: base.b + (top.b - base.b) * a,
        a: 1.0,
    }
}

/// Fixed-width invisible spacer standing in for unmounted columns.
pub(super) fn spacer_el(key: &str, width: f32) -> El {
    el().key(key.to_owned()).w(width).min_w(width).h(0.0).a11y_hidden()
}

/// 1 pt right-edge divider faded in while the rightmost frozen column is stuck.
pub(super) fn frozen_divider(rule: Color) -> Shadow {
    Shadow { x: 1.0, y: 0.0, blur: 0.0, spread: 0.0, color: rule, inset: false }
}

/// Visible indices of non-frozen columns that must remain mounted (resizing, reordering, or focused).
pub(super) fn pinned_indices(state: &TableState, columns: &[TableColumn]) -> Vec<usize> {
    let mut out = Vec::with_capacity(3);
    if let Some(id) = state.resizing_column()
        && let Some(idx) = columns.iter().position(|c| c.id == id)
    {
        out.push(idx);
    }
    if let Some(idx) = state.held_column_index() {
        out.push(idx);
    }
    if let Some(id) = state.columns.focused_column()
        && let Some(idx) = columns.iter().position(|c| c.id == id)
    {
        out.push(idx);
    }
    out
}

impl ColumnPlan {
    /// Standalone header plan (no `scroll_xy` viewport): mounts every visible column in order.
    pub(super) fn all(columns: &[TableColumn], grid_coords: bool) -> ColumnPlan {
        let items = (0..columns.len())
            .map(|index| ColumnItem::Col {
                index,
                sticky_left: None,
                frozen_last: false,
                in_base_window: true,
            })
            .collect();
        ColumnPlan {
            items,
            total_width: 0.0,
            frozen_active: false,
            windowing_active: false,
            grid_coords,
            band: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compute(
        columns: &[TableColumn],
        sizes: &[Width],
        scroll_wide: bool,
        virtual_columns: bool,
        frozen_columns: usize,
        view_w: f32,
        anchor_x: f32,
        pinned: &[usize],
    ) -> ColumnPlan {
        let n = columns.len();
        let grid_coords = virtual_columns || frozen_columns > 0;
        if !scroll_wide || n == 0 {
            return ColumnPlan::all(columns, grid_coords);
        }

        let mut xs = Vec::with_capacity(n + 1);
        xs.push(0.0f32);
        for (c, &w) in columns.iter().zip(sizes) {
            let cw = resolved_min_width(c, w);
            xs.push(xs.last().copied().unwrap_or(0.0) + cw);
        }
        let total = xs[n];
        let effective_w = if view_w > 0.0 { view_w } else { INITIAL_VIEW_W };
        let overflowing = total > effective_w + 0.5;
        let frozen = frozen_columns.min(n);
        let frozen_active = overflowing && frozen > 0;
        let windowing_active = overflowing && virtual_columns && n > frozen;

        if !windowing_active {
            let items = (0..n)
                .map(|index| {
                    let is_frozen = frozen_active && index < frozen;
                    ColumnItem::Col {
                        index,
                        sticky_left: is_frozen.then_some(xs[index]),
                        frozen_last: is_frozen && index + 1 == frozen,
                        in_base_window: true,
                    }
                })
                .collect();
            return ColumnPlan {
                items,
                total_width: total,
                frozen_active,
                windowing_active: false,
                grid_coords,
                band: None,
            };
        }

        let overscan = (effective_w * OVERSCAN_RATIO).max(MIN_OVERSCAN);
        let edge = overscan * EDGE_RATIO;
        let margin = (overscan - edge).max(0.0);
        let max_scroll = (total - effective_w).max(0.0);
        let clamped_anchor = anchor_x.clamp(0.0, max_scroll);
        let frozen_w = xs[frozen];
        let win_left = (clamped_anchor + frozen_w - overscan).max(frozen_w);
        let win_right = (clamped_anchor + effective_w + overscan).min(total).max(win_left);

        // First non-frozen column whose right edge exceeds win_left.
        let win_start = (frozen + xs[frozen + 1..=n].partition_point(|&right| right <= win_left)).min(n);
        // First non-frozen column whose left edge is at or past win_right.
        let win_end = (frozen + xs[frozen..n].partition_point(|&left| left < win_right)).clamp(win_start, n);

        // Merge pinned non-frozen columns (resizing, reordering, focused/caret-browsing) with the
        // contiguous window `win_start..win_end` in strictly increasing column order.
        let mut extra: Vec<usize> = pinned
            .iter()
            .copied()
            .filter(|&idx| idx >= frozen && idx < n && !(win_start..win_end).contains(&idx))
            .collect();
        extra.sort_unstable();
        extra.dedup();

        let mut items = Vec::with_capacity(frozen + (win_end - win_start) + extra.len() + 4);
        for index in 0..frozen {
            items.push(ColumnItem::Col {
                index,
                sticky_left: Some(xs[index]),
                frozen_last: index + 1 == frozen,
                in_base_window: true,
            });
        }

        let mut mounted = Vec::with_capacity((win_end - win_start) + extra.len());
        let mut ei = 0usize;
        while ei < extra.len() && extra[ei] < win_start {
            mounted.push((extra[ei], false));
            ei += 1;
        }
        for idx in win_start..win_end {
            mounted.push((idx, true));
        }
        while ei < extra.len() {
            mounted.push((extra[ei], false));
            ei += 1;
        }

        let mut cur_x = frozen_w;
        let mut spacer_n = 0usize;
        for (index, in_base_window) in mounted {
            let gap = xs[index] - cur_x;
            if gap > 0.01 {
                items.push(ColumnItem::Spacer { key: format!("__rune_sp_{spacer_n}"), width: gap });
                spacer_n += 1;
            }
            items.push(ColumnItem::Col {
                index,
                sticky_left: None,
                frozen_last: false,
                in_base_window,
            });
            cur_x = xs[index + 1];
        }
        let tail = total - cur_x;
        if tail > 0.01 {
            items.push(ColumnItem::Spacer { key: format!("__rune_sp_{spacer_n}"), width: tail });
        }

        let band = BandParams {
            total,
            frozen_w,
            effective_w,
            anchor_x: clamped_anchor,
            win_start_x: xs[win_start],
            win_end_x: xs[win_end],
            at_start: win_start == frozen,
            at_end: win_end == n,
            margin,
        };

        ColumnPlan {
            items,
            total_width: total,
            frozen_active,
            windowing_active: true,
            grid_coords,
            band: Some(band),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_cols(n: usize, w: f32) -> (Vec<TableColumn>, Vec<Width>) {
        let cols: Vec<_> = (0..n)
            .map(|i| TableColumn::new(format!("c{i}"), format!("C{i}")).width(w))
            .collect();
        let sizes = vec![Width::Fixed(w); n];
        (cols, sizes)
    }

    fn plan_width(plan: &ColumnPlan, widths: &[f32]) -> f32 {
        plan.items
            .iter()
            .map(|it| match it {
                ColumnItem::Spacer { width, .. } => *width,
                ColumnItem::Col { index, .. } => widths[*index],
            })
            .sum()
    }

    #[test]
    fn wide_table_mounts_bounded_window_and_preserves_exact_total_width() {
        let (cols, sizes) = fixed_cols(200, 100.0);
        let widths = vec![100.0; 200];
        let plan = ColumnPlan::compute(&cols, &sizes, true, true, 2, 1000.0, 5000.0, &[]);
        assert!(plan.windowing_active && plan.frozen_active && plan.grid_coords);
        assert_eq!(plan.total_width, 20_000.0);
        assert!((plan_width(&plan, &widths) - 20_000.0).abs() < 0.01);

        let mounted_cols: Vec<usize> = plan
            .items
            .iter()
            .filter_map(|it| match it {
                ColumnItem::Col { index, .. } => Some(*index),
                ColumnItem::Spacer { .. } => None,
            })
            .collect();
        assert!(mounted_cols.len() <= 25, "mounted only window + frozen: {}", mounted_cols.len());
        assert_eq!(&mounted_cols[..2], &[0, 1]);
        assert!(mounted_cols.contains(&55));
        assert!(!mounted_cols.contains(&10));
        assert!(!mounted_cols.contains(&190));

        let band = plan.band.expect("windowing produces a band");
        assert!(band.contains(5000.0));
        assert!(band.contains(5100.0));
        assert!(band.contains(4900.0));
        assert!(!band.contains(5400.0));
        assert!(!band.contains(4600.0));
    }

    #[test]
    fn pinned_out_of_window_column_stays_mounted_at_exact_offset() {
        let (cols, sizes) = fixed_cols(200, 100.0);
        let widths = vec![100.0; 200];
        let plan = ColumnPlan::compute(&cols, &sizes, true, true, 1, 1000.0, 5000.0, &[5, 180]);
        assert!((plan_width(&plan, &widths) - 20_000.0).abs() < 0.01);

        let mut x = 0.0f32;
        let mut seen = Vec::new();
        for item in &plan.items {
            match item {
                ColumnItem::Spacer { width, .. } => x += *width,
                ColumnItem::Col { index, .. } => {
                    assert!((x - (*index as f32) * 100.0).abs() < 0.01, "col {index} landed at {x}");
                    seen.push(*index);
                    x += widths[*index];
                }
            }
        }
        assert!(seen.contains(&0), "frozen col 0");
        assert!(seen.contains(&5), "left pinned col 5");
        assert!(seen.contains(&55), "visible col 55");
        assert!(seen.contains(&180), "right pinned col 180");
    }

    #[test]
    fn fitting_empty_and_all_frozen_tables_do_not_window() {
        let (cols, sizes) = fixed_cols(3, 100.0);
        let fits = ColumnPlan::compute(&cols, &sizes, true, true, 1, 800.0, 0.0, &[]);
        assert!(!fits.windowing_active && !fits.frozen_active);
        assert_eq!(fits.items.len(), 3);

        let all_frozen = ColumnPlan::compute(&cols, &sizes, true, true, 10, 200.0, 0.0, &[]);
        assert!(!all_frozen.windowing_active && all_frozen.frozen_active);
        assert_eq!(all_frozen.items.len(), 3);

        let empty = ColumnPlan::compute(&[], &[], true, true, 2, 500.0, 0.0, &[]);
        assert!(!empty.windowing_active && empty.items.is_empty());
    }
}
