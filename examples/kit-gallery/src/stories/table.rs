//! Rune's own `Table` scenes:
//! - `rune-table--options`: `Table::row` (right-click context menu and custom selection accent),
//!   `Table::stick_bottom`, and `Table::cursor_outline`.
//! - `rune-table--wide`: `Table::scroll_wide`, column visibility (`TableState::hide_column` /
//!   `show_column`), `Table::on_header_secondary_press`, `Table::on_column_fit`, and
//!   `TableColumn` getters.
//! - `rune-table--overscan`: `Table::virtual_overscan` presets on a 10,000-row fixed-height
//!   telemetry table (default `2.0 / 1.75`, tight `0.5 / 0.25`, viewport-only `0.0 / 0.0`).
//! - `rune-table--virtual-columns`: `Table::virtual_columns` and `Table::frozen_columns` on a
//!   10,000-row × 200-column wide sheet with resizable, reorderable columns.

use std::borrow::Cow;

use rune::{Align, El, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Cell, Kit, MenuItem, Switch, Table, TableColumn, context_menu};

use crate::G;

const EVENTS: [&str; 6] = [
    "connected to worker pool on loopback:8443",
    "indexed 1,024 rows in 1.8 ms (virtual_rows mounted 14)",
    "cache miss for asset bundle — streaming response from origin server",
    "rebuilt layout tree after viewport resize",
    "flushed batch checkpoint to local journal",
    "received keep-alive frame with zero dropped samples",
];

fn row_id(i: usize) -> String {
    format!("log-{i}")
}

pub(super) fn options(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let stick_bottom = !s.is("table-no-stick");
    let cursor_outline = s.is("table-cursor-outline");
    let accent = c.text_accent;
    let warn = c.warning.fg;
    let ok = c.success.fg;

    let table = Table::new(
        |s: &G| &s.table,
        |s: &mut G| &mut s.table,
        |s: &G| s.table_rows,
        |_: &G, i| Cow::Owned(row_id(i)),
    )
    .name("Live event log")
    .column(TableColumn::new("seq", "#").width(52.0).align(Align::End).resizable(false))
    .column(TableColumn::new("level", "Level").width(64.0).resizable(false))
    .column(TableColumn::new("event", "Event").flex(1.0).min(180.0))
    .cell(move |_: &G, i, col| match col {
        "seq" => Cell::text(format!("{:03}", i + 1)).mono().muted(),
        "level" => match i % 5 {
            2 => Cell::text("WARN").mono().color(warn),
            4 => Cell::text("OK").mono().color(ok),
            _ => Cell::text("INFO").mono().muted(),
        },
        _ => Cell::text(format!("#{} {}", i + 1, EVENTS[i % EVENTS.len()])),
    })
    .row(move |s: &G, i, r| {
        let id = row_id(i);
        let selected = s.table.is_selected(&id);
        let mut r = r.on_secondary_press(move |s: &mut G, p, _| {
            if !s.table.is_selected(&id) {
                s.table.select(id.clone());
            }
            s.table_menu = Some(("options", id.clone(), p));
        });
        if !cursor_outline && selected {
            r = r.child(
                el().key("accent")
                    .pin(Some(0.0), Some(0.0), None, Some(0.0))
                    .w(2.0)
                    .fill(accent)
                    .a11y_hidden(),
            );
        }
        r
    })
    .stick_bottom(stick_bottom)
    .cursor_outline(cursor_outline)
    .zebra(true)
    .build(s, k)
    .grow(1.0)
    .min_h(0.0);

    let menu = s.table_menu.as_ref().filter(|(which, _, _)| *which == "options");
    let target = menu.map(|(_, id, _)| id.clone()).unwrap_or_default();
    let copy_id = target.clone();
    let solo_id = target.clone();
    let table_with_menu = context_menu("table-row-menu", table, menu.map(|(_, _, p)| *p))
        .item(
            MenuItem::new(format!("Copy {target}")).on_select_tx(move |s: &mut G, tx: &mut Tx| {
                tx.copy(&copy_id);
                s.pick.insert("table-last".into(), format!("Copied {copy_id}"));
                s.table_menu = None;
            }),
        )
        .item(MenuItem::new("Select only this row").on_select(move |s: &mut G| {
            s.table.select(solo_id.clone());
            s.pick.insert("table-last".into(), format!("Selected {solo_id}"));
            s.table_menu = None;
        }))
        .separator()
        .item(MenuItem::new("Clear selection").on_select(|s: &mut G| {
            s.table.clear_selection();
            s.pick.insert("table-last".into(), "Cleared selection".into());
            s.table_menu = None;
        }))
        .on_close(|s: &mut G| s.table_menu = None)
        .build(k, layer);

    let last = s.pick.get("table-last").map(String::as_str);
    let status = match last {
        Some(msg) => format!("{} rows · {} selected · {msg}", s.table_rows, s.table.selected_len()),
        None => format!("{} rows · {} selected · right-click any row", s.table_rows, s.table.selected_len()),
    };

    col()
        .key("table-options")
        .w(640.0)
        .gap(10.0)
        .child(
            row()
                .key("table-controls")
                .gap(14.0)
                .center_y()
                .child(
                    Button::new("Append row")
                        .on_press(|s: &mut G| s.table_rows += 1)
                        .build(k)
                        .key("append-row"),
                )
                .child(
                    Switch::new(stick_bottom)
                        .label("Stick bottom")
                        .on_toggle(|s: &mut G| s.flip("table-no-stick"))
                        .build(k)
                        .key("toggle-stick-bottom"),
                )
                .child(
                    Switch::new(cursor_outline)
                        .label("Cursor outline")
                        .on_toggle(|s: &mut G| s.flip("table-cursor-outline"))
                        .build(k)
                        .key("toggle-cursor-outline"),
                ),
        )
        .child(
            col()
                .key("table-frame")
                .w_full()
                .h(240.0)
                .radius(k.radius.md)
                .fill(c.bg_base)
                .border(1.0, c.border_weak)
                .clip_rounded(k.radius.md)
                .child(table_with_menu),
        )
        .child(text(status).font(k.ty.small).color(c.text_muted))
}

fn wide_columns() -> Vec<TableColumn> {
    vec![
        TableColumn::new("id", "ID").width(64.0).align(Align::End).sortable(),
        TableColumn::new("status", "Status").width(84.0).sortable(),
        TableColumn::new("region", "Region").width(96.0).sortable(),
        TableColumn::new("latency", "Latency").width(96.0).min(72.0).max(180.0).align(Align::End).sortable(),
        TableColumn::new("bytes", "Bytes").width(104.0).min(72.0).max(180.0).align(Align::End).sortable(),
        TableColumn::new("service", "Service").flex(1.0).min(150.0).sortable(),
        TableColumn::new("endpoint", "Endpoint").flex(1.5).min(220.0),
        TableColumn::new("trace", "Trace ID").flex(1.0).min(160.0),
    ]
}

fn fit_width_for(col: &str) -> f32 {
    match col {
        "id" => 76.0,
        "status" => 92.0,
        "region" => 112.0,
        "latency" => 108.0,
        "bytes" => 116.0,
        "service" => 176.0,
        "endpoint" => 260.0,
        _ => 188.0,
    }
}

pub(super) fn wide(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let scroll_wide = !s.is("wide-no-scroll");
    let cols = wide_columns();
    let region_shown = !s.table.is_column_hidden("region");
    let ok = c.success.fg;
    let warn = c.warning.fg;

    let mut t = Table::new(
        |s: &G| &s.table,
        |s: &mut G| &mut s.table,
        |_: &G| 40,
        |_: &G, i| Cow::Owned(format!("req-{i}")),
    )
    .name("Wide query results")
    .scroll_wide(scroll_wide)
    .zebra(true);
    for col in &cols {
        t = t.column(col.clone());
    }
    let table = t
        .cell(move |_: &G, i, col| match col {
            "id" => Cell::text(format!("{:04}", i + 1001)).mono().muted(),
            "status" => match i % 6 {
                4 => Cell::text("429 RATE").mono().color(warn),
                _ => Cell::text("200 OK").mono().color(ok),
            },
            "region" => Cell::text(["us-east-1", "eu-west-1", "ap-south-1"][i % 3]).mono(),
            "latency" => Cell::text(format!("{:.1} ms", 4.2 + (i % 19) as f32 * 3.7)).mono(),
            "bytes" => Cell::text(format!("{} B", 512 + (i * 389) % 16_384)).mono(),
            "service" => Cell::text(["gateway-edge", "query-planner", "index-shard", "auth-token"][i % 4]),
            "endpoint" => Cell::text(format!("/v2/projects/{}/queries/run?limit=100", 10 + (i % 8))),
            _ => Cell::text(format!("tr_{:08x}{:08x}", i * 2654435761, i + 97)).mono().muted(),
        })
        .on_header_secondary_press(|s: &mut G, col, press, _| {
            s.table_menu = Some(("wide", col.to_owned(), press));
        })
        .on_column_fit(|s: &mut G, col, _| {
            let w = fit_width_for(col);
            s.table.set_column_width(col, w);
            s.pick.insert("wide-last".into(), format!("Fitted {col} to {w:.0} pt"));
        })
        .build(s, k)
        .grow(1.0)
        .min_h(0.0);

    let menu_state = s.table_menu.as_ref().filter(|(which, _, _)| *which == "wide");
    let target_id = menu_state.map(|(_, id, _)| id.clone()).unwrap_or_default();
    let target_title = cols
        .iter()
        .find(|col| col.id() == target_id)
        .map_or(target_id.as_str(), TableColumn::title)
        .to_owned();
    let fit_id = target_id.clone();
    let reset_id = target_id.clone();
    let hide_id = target_id.clone();
    let mut menu = context_menu("table-header-menu", table, menu_state.map(|(_, _, p)| *p))
        .item(MenuItem::new(format!("Fit {target_title} to content")).on_select(move |s: &mut G| {
            let w = fit_width_for(&fit_id);
            s.table.set_column_width(fit_id.clone(), w);
            s.pick.insert("wide-last".into(), format!("Fitted {fit_id} to {w:.0} pt"));
            s.table_menu = None;
        }))
        .item(MenuItem::new(format!("Reset {target_title} width")).on_select(move |s: &mut G| {
            s.table.reset_column_width(&reset_id);
            s.pick.insert("wide-last".into(), format!("Reset {reset_id} width"));
            s.table_menu = None;
        }))
        .item(MenuItem::new(format!("Hide {target_title}")).on_select(move |s: &mut G| {
            s.table.hide_column(hide_id.clone());
            s.pick.insert("wide-last".into(), format!("Hid {hide_id}"));
            s.table_menu = None;
        }))
        .separator();
    for col in &cols {
        let cid = col.id().to_owned();
        let title = col.title().to_owned();
        let hidden = s.table.is_column_hidden(&cid);
        let label = if hidden { format!("Show {title}") } else { format!("Hide {title}") };
        menu = menu.item(MenuItem::new(label).on_select(move |s: &mut G| {
            let now_hidden = !s.table.is_column_hidden(&cid);
            s.table.set_column_hidden(cid.clone(), now_hidden);
            s.pick.insert("wide-last".into(), format!("{} {cid}", if now_hidden { "Hid" } else { "Showed" }));
            s.table_menu = None;
        }));
    }
    let table_with_menu = menu.on_close(|s: &mut G| s.table_menu = None).build(k, layer);

    let visible = cols.iter().filter(|col| !s.table.is_column_hidden(col.id())).count();
    let hidden_count = cols.len() - visible;
    let last = s.pick.get("wide-last").map(String::as_str);
    let status = match last {
        Some(msg) => format!("{visible}/{} columns visible ({hidden_count} hidden) · {msg}", cols.len()),
        None => format!(
            "{visible}/{} columns visible ({hidden_count} hidden) · right-click any header or double-click a column edge",
            cols.len()
        ),
    };

    col()
        .key("table-wide")
        .w(640.0)
        .gap(10.0)
        .child(
            row()
                .key("wide-controls")
                .gap(14.0)
                .center_y()
                .child(
                    Switch::new(scroll_wide)
                        .label("Scroll wide")
                        .on_toggle(|s: &mut G| s.flip("wide-no-scroll"))
                        .build(k)
                        .key("toggle-scroll-wide"),
                )
                .child(
                    Switch::new(region_shown)
                        .label("Show Region")
                        .on_toggle(|s: &mut G| {
                            let hide = !s.table.is_column_hidden("region");
                            s.table.set_column_hidden("region", hide);
                        })
                        .build(k)
                        .key("toggle-region"),
                )
                .child(
                    Button::new("Reset columns")
                        .on_press(|s: &mut G| {
                            for col in wide_columns() {
                                s.table.show_column(col.id());
                                s.table.reset_column_width(col.id());
                            }
                            s.pick.insert("wide-last".into(), "Reset all columns".into());
                        })
                        .build(k)
                        .key("reset-columns"),
                ),
        )
        .child(
            col()
                .key("wide-frame")
                .w_full()
                .h(240.0)
                .radius(k.radius.md)
                .fill(c.bg_base)
                .border(1.0, c.border_weak)
                .clip_rounded(k.radius.md)
                .child(table_with_menu),
        )
        .child(text(status).font(k.ty.small).color(c.text_muted))
}

pub(super) fn overscan(s: &G, k: &Kit, _layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let mode = s.pick.get("table-overscan").map(String::as_str).unwrap_or("tight");
    let ok = c.success.fg;
    let warn = c.warning.fg;

    let mut t = Table::new(
        |s: &G| &s.table,
        |s: &mut G| &mut s.table,
        |_: &G| 10_000,
        |_: &G, i| Cow::Owned(format!("sig-{i}")),
    )
    .name("Telemetry signals (10,000 rows)")
    .column(TableColumn::new("id", "Signal").width(92.0).resizable(false))
    .column(TableColumn::new("value", "Value").width(96.0).align(Align::End))
    .column(TableColumn::new("state", "State").width(76.0).resizable(false))
    .column(TableColumn::new("bus", "Bus").flex(1.0).min(160.0))
    .cell(move |_: &G, i, col| match col {
        "id" => Cell::text(format!("sig-{i:05}")).mono().muted(),
        "value" => Cell::text(format!("{:.3}", (i as f64 * 0.731).sin() * 100.0)).mono(),
        "state" => match i % 4 {
            2 => Cell::text("FAULT").mono().color(warn),
            1 => Cell::text("RUN").mono().color(ok),
            0 => Cell::text("IDLE").mono().muted(),
            _ => Cell::text("HOLD").mono().muted(),
        },
        _ => Cell::text(format!("can{}.node_{:02}.ch_{}", i % 4, (i / 4) % 32, i % 8)).mono(),
    })
    .zebra(true);
    match mode {
        "tight" => t = t.virtual_overscan(0.5, 0.25),
        "zero" => t = t.virtual_overscan(0.0, 0.0),
        _ => {}
    }
    let table = t.build(s, k).grow(1.0).min_h(0.0);

    let preset = |id: &'static str, label: &'static str| {
        let variant = if mode == id { ButtonVariant::Contrast } else { ButtonVariant::Neutral };
        Button::new(label)
            .variant(variant)
            .on_press(move |s: &mut G| {
                s.pick.insert("table-overscan".into(), id.into());
            })
            .build(k)
            .key(format!("overscan-{id}"))
    };

    let summary = match mode {
        "tight" => "10,000 rows · virtual_overscan(0.5, 0.25) · ~2 viewports mounted, more frequent window moves",
        "zero" => "10,000 rows · virtual_overscan(0.0, 0.0) · viewport rows only, commits at every row edge",
        _ => "10,000 rows · default (2.0, 1.75) · ~5 viewports mounted, fewer window moves while scrolling",
    };

    col()
        .key("table-overscan")
        .w(640.0)
        .gap(10.0)
        .child(
            row()
                .key("overscan-controls")
                .gap(8.0)
                .center_y()
                .child(preset("default", "Default (2.0 / 1.75)"))
                .child(preset("tight", "Tight (0.5 / 0.25)"))
                .child(preset("zero", "Viewport (0.0 / 0.0)")),
        )
        .child(
            col()
                .key("overscan-frame")
                .w_full()
                .h(260.0)
                .radius(k.radius.md)
                .fill(c.bg_base)
                .border(1.0, c.border_weak)
                .clip_rounded(k.radius.md)
                .child(table),
        )
        .child(text(summary).font(k.ty.small).color(c.text_muted))
}

pub(super) fn virtual_columns(s: &G, k: &Kit, _layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let virtual_cols = !s.is("vc-off");
    let frozen: usize = s.pick.get("table-frozen").and_then(|v| v.parse().ok()).unwrap_or(2);

    let mut t = Table::new(
        |s: &G| &s.table,
        |s: &mut G| &mut s.table,
        |_: &G| 10_000,
        |_: &G, i| Cow::Owned(format!("row-{i}")),
    )
    .name("Wide sheet (10,000 rows × 200 columns)")
    .scroll_wide(true)
    .virtual_columns(virtual_cols)
    .frozen_columns(frozen)
    .virtual_overscan(0.5, 0.25)
    .zebra(true)
    .on_reorder(|s: &mut G, from, to, _| {
        s.pick.insert("vc-last".into(), format!("Moved column {from} -> {to}"));
    });
    t = t
        .column(TableColumn::new("id", "Row").width(76.0).align(Align::End))
        .column(TableColumn::new("label", "Series").width(116.0));
    for col in 2..200 {
        t = t.column(TableColumn::new(format!("m{col}"), format!("Metric {col}")).width(96.0).align(Align::End));
    }
    let table = t
        .cell(|_: &G, i, col| match col {
            "id" => Cell::text(format!("{:05}", i + 1)).mono().muted(),
            "label" => Cell::text(format!("series_{:03}", i % 256)).mono(),
            other => {
                let n: usize = other.strip_prefix('m').and_then(|s| s.parse().ok()).unwrap_or(0);
                Cell::text(format!("{:.2}", ((i * 31 + n * 17) % 10_000) as f32 / 100.0)).mono()
            }
        })
        .build(s, k)
        .grow(1.0)
        .min_h(0.0);

    let freeze_btn = |count: usize, label: &'static str| {
        let variant = if frozen == count { ButtonVariant::Contrast } else { ButtonVariant::Neutral };
        Button::new(label)
            .variant(variant)
            .on_press(move |s: &mut G| {
                s.pick.insert("table-frozen".into(), count.to_string());
            })
            .build(k)
            .key(format!("freeze-{count}"))
    };

    let last = s.pick.get("vc-last").map(String::as_str);
    let summary = match last {
        Some(msg) => format!(
            "10,000 rows × 200 columns · virtual_columns({virtual_cols}) · frozen_columns({frozen}) · {msg}"
        ),
        None => format!(
            "10,000 rows × 200 columns · virtual_columns({virtual_cols}) · frozen_columns({frozen}) · scroll sideways or drag headers"
        ),
    };

    col()
        .key("table-virtual-columns")
        .w(680.0)
        .gap(10.0)
        .child(
            row()
                .key("vc-controls")
                .gap(10.0)
                .center_y()
                .child(
                    Switch::new(virtual_cols)
                        .label("Virtual columns")
                        .on_toggle(|s: &mut G| s.flip("vc-off"))
                        .build(k)
                        .key("toggle-virtual-columns"),
                )
                .child(freeze_btn(0, "0 frozen"))
                .child(freeze_btn(1, "1 frozen"))
                .child(freeze_btn(2, "2 frozen")),
        )
        .child(
            col()
                .key("vc-frame")
                .w_full()
                .h(260.0)
                .radius(k.radius.md)
                .fill(c.bg_base)
                .border(1.0, c.border_weak)
                .clip_rounded(k.radius.md)
                .child(table),
        )
        .child(text(summary).font(k.ty.small).color(c.text_muted))
}
