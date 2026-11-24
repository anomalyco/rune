//! Rune's own `Table` scene showing `Table::row` (right-click context menu and custom selection
//! accent), `Table::stick_bottom`, and `Table::cursor_outline`.

use std::borrow::Cow;

use rune::{Align, El, Tx, col, el, row, text};
use rune_kit::{Button, Cell, Kit, MenuItem, Switch, Table, TableColumn, context_menu};

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
            s.table_menu = Some((id.clone(), p));
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

    let target = s.table_menu.as_ref().map(|(id, _)| id.clone()).unwrap_or_default();
    let copy_id = target.clone();
    let solo_id = target.clone();
    let table_with_menu = context_menu("table-row-menu", table, s.table_menu.as_ref().map(|(_, p)| *p))
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
