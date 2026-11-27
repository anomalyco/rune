//! Header and body row rendering for [`Table`](super::Table).

use rune::{Align, Color, Cursor, El, el, icon, row, text};

use super::view::{Layout, RowSpec, TableBind};
use super::window::{self, ColumnItem, ColumnPlan};
use super::{Cell, CellContent, SortDir, TableColumn, TableState, TableStyle, Width};
use crate::Kit;

pub(super) fn build_row_el(
    st: &TableState,
    id: &str,
    i: usize,
    spec: &RowSpec,
    cell: &dyn Fn(&str) -> Cell,
    bind_focus: &dyn Fn(El, &str, bool) -> El,
) -> El {
    let style = &spec.style;
    let selected = st.is_selected(id);
    let mut r = row()
        .key(id.to_owned())
        .role("row")
        .focus_selected(selected)
        .w_full()
        .h(style.row_height)
        .center_y();
    if spec.plan.grid_coords {
        r = r.row_index(i + 1);
    }
    let frozen_fill = if selected {
        r = r.fill(style.selected);
        window::opaque_over(spec.bg, style.selected)
    } else if spec.zebra && i % 2 == 1 {
        r = r.fill(style.stripe).hover(style.stripe.mix(style.selected, 0.5));
        if let Some((enter, exit)) = spec.hover_motion {
            r = r.token_hover_motion(enter, exit);
        }
        window::opaque_over(spec.bg, style.stripe)
    } else {
        r = r.hover(style.stripe.mix(style.selected, 0.5));
        if let Some((enter, exit)) = spec.hover_motion {
            r = r.token_hover_motion(enter, exit);
        }
        Color { a: 1.0, ..spec.bg }
    };
    if spec.cursor_outline && st.multi && st.cursor.as_deref() == Some(id) {
        r = r.outline(1.0, -1.0, spec.focus);
    }
    for item in &spec.plan.items {
        match item {
            ColumnItem::Spacer { key, width } => {
                r = r.child(window::spacer_el(key, *width));
            }
            &ColumnItem::Col { index: j, sticky_left, frozen_last, in_base_window } => {
                let c = &spec.cols[j];
                let mut el = cell_el(c, spec.sizes[j], cell(&c.id), style).key(c.id.clone());
                if spec.plan.grid_coords {
                    el = el.column_index(j + 1);
                }
                if let Some(inset) = sticky_left {
                    el = el.sticky_left(inset).fill(frozen_fill);
                    if frozen_last {
                        el = el.stuck_shadow(window::frozen_divider(style.rule));
                    }
                }
                if spec.plan.windowing_active {
                    el = bind_focus(el, &c.id, in_base_window);
                }
                r = r.child(el);
            }
        }
    }
    r
}

/// Sizes a header or body cell to its column.
fn sized(e: El, c: &TableColumn, w: Width) -> El {
    c.apply_width(e, w)
}

fn justify(e: El, a: Align) -> El {
    match a {
        Align::Start => e,
        Align::Center => e.justify_center(),
        Align::End => e.justify_end(),
    }
}

fn cell_el(c: &TableColumn, w: Width, cell: Cell, style: &TableStyle) -> El {
    let inner = match cell.content {
        CellContent::Text(t) => {
            let color = cell.color.unwrap_or(if cell.muted { style.muted } else { style.text });
            text(t.clone())
                .font(if cell.mono { style.mono } else { style.font })
                .color(color)
                .align(c.align)
                .truncate()
                .shrink()
                .min_w(0.0)
                .help_when_truncated(t)
        }
        CellContent::El(e) => e,
    };
    justify(sized(row().role("gridcell").h_full().px(style.cell_pad).center_y().clip_soft(0.0), c, w), c.align).child(inner)
}

pub(super) fn header_row(
    k: &Kit,
    state: &TableState,
    layout: &Layout,
    plan: &ColumnPlan,
    style: &TableStyle,
    bind: &dyn TableBind,
) -> El {
    let (columns, sizes) = (&layout.columns, &layout.sizes);
    let (dragged, held) = (state.dragged_column(), state.holding_column());
    let mut rule = el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(style.rule).a11y_hidden();
    if plan.frozen_active {
        rule = rule.paint_order(1);
    }
    let mut h = row()
        .key("header")
        .role("row")
        .w_full()
        .h(style.header_height)
        .child(rule);
    let last = columns.len().saturating_sub(1);
    let header_bg = Color { a: 1.0, ..k.c.bg_base };
    for item in &plan.items {
        let &ColumnItem::Col { index: j, sticky_left, frozen_last, .. } = item else {
            if let ColumnItem::Spacer { key, width } = item {
                h = h.child(window::spacer_el(key, *width));
            }
            continue;
        };
        let c = &columns[j];
        let sorted = state.sort().filter(|(id, _)| *id == c.id).map(|(_, d)| d);
        let title = text(c.title.clone())
            .key("title")
            .font(style.header_font)
            .color(if sorted.is_some() { style.text } else { style.header_text })
            .truncate()
            .shrink()
            .min_w(0.0)
            .help_when_truncated(c.title.clone());
        let mut inner = row().key("inner").gap(2.0).center_y().min_w(0.0).shrink().child(title).a11y_hidden();
        if let Some(d) = sorted {
            let glyph = if d == SortDir::Ascending { k.glyph.chevron_up } else { k.glyph.chevron_down };
            inner = inner.child(icon(glyph, 14.0).key("sort").color(style.text));
        }
        let mut cell = justify(
            sized(
                row().role("columnheader").label(c.title.clone()).sorted(sorted.map(|d| d == SortDir::Ascending)).h_full().px(style.cell_pad).center_y(),
                c,
                sizes[j],
            ),
            c.align,
        )
        .key(c.id.clone())
        .child(inner);
        if plan.grid_coords {
            cell = cell.column_index(j + 1);
        }
        cell = bind.bind_header_layout(cell, c.id.clone(), plan.windowing_active);
        if let Some(inset) = sticky_left {
            cell = cell.sticky_left(inset).fill(header_bg);
            if frozen_last {
                cell = cell.stuck_shadow(window::frozen_divider(style.rule));
            }
        }
        if c.sortable {
            let hover = if sticky_left.is_some() { window::opaque_over(header_bg, k.c.hover) } else { k.c.hover };
            cell = bind.bind_header_sort(k.apply_hover_motion(cell.cursor(Cursor::Pointer).hover(hover)), c.id.clone());
        }
        cell = bind.bind_header_secondary_press(cell, c.id.clone());
        cell = bind.bind_header_reorder(cell, j, layout.declared.clone(), held);
        if let Some((_, dx)) = dragged.filter(|&(from, _)| from == j) {
            // The dragged header follows the pointer over its neighbours.
            cell = cell.offset(dx, 0.0).raise().fill(k.c.bg_base).layer_opacity(0.9);
        }
        if c.resizable {
            cell = cell.child(resize_handle(c.clone(), style.rule, j == last, bind));
        } else if j != last {
            cell = cell.child(el().key("rule").pin(None, Some(8.0), Some(0.0), Some(8.0)).w(1.0).fill(style.rule).a11y_hidden());
        }
        h = h.child(cell);
    }
    h
}

/// The drag target on a header cell's right edge: a hairline that widens the column as it's
/// dragged, clamped to its bounds. A double click puts the column back.
fn resize_handle(c: TableColumn, rule: Color, last: bool, bind: &dyn TableBind) -> El {
    let line = el().key("line").w(1.0).h_full().fill(if last { Color::TRANSPARENT } else { rule }).a11y_hidden();
    let handle = row()
        .key("resize")
        .role("separator")
        .label(format!("Resize {}", c.title))
        .pin(None, Some(6.0), Some(-4.0), Some(6.0))
        .w(9.0)
        .justify_center()
        .cursor(Cursor::ResizeColumn);
    bind.bind_resize(handle, c).child(line)
}
