//! 100,000 build artifacts in a sortable, resizable, striped table. `cargo run -p rune-kit
//! --example table` opens the window. Click a header to sort, drag its edge to resize
//! (double-click the edge to put it back), and use the arrows, Shift and Cmd-A to select.
//! The stills in `demo/kit/table-{dark,light}.png`:
//! `-- --snap table.png --theme dark --size 760x480 --scale 2`.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use std::borrow::Cow;

use rune::{Align, AppSpec, Cx, El, col, el, row, text};
use rune_kit::{Cell, Kit, SortDir, Table, TableColumn, TableState};

struct Artifact {
    id: String,
    name: String,
    target: &'static str,
    size: u64,
    ms: u32,
    cached: bool,
}

struct State {
    rows: Vec<Artifact>,
    table: TableState,
}

const TARGETS: [&str; 5] = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "wasm32-unknown-unknown", "aarch64-apple-ios"];
const CRATES: [&str; 8] = ["rune", "rune-kit", "rune-motion", "rune-fx", "rune-light", "rune-ref", "taffy", "wgpu-core"];
const UNITS: [&str; 6] = ["lib", "build-script", "test", "bench", "example", "doc"];

fn state() -> State {
    // A small xorshift keeps the data the same on every run.
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let rows = (0..100_000)
        .map(|i| {
            let r = next();
            Artifact {
                id: format!("a{i}"),
                name: format!("{} {} #{}", CRATES[(r % 8) as usize], UNITS[(r >> 3) as usize % 6], i),
                target: TARGETS[(r >> 7) as usize % 5],
                size: (r >> 11) % 48_000_000 + 4_096,
                ms: ((r >> 37) % 90_000) as u32,
                cached: (r >> 59) % 3 != 0,
            }
        })
        .collect();
    let mut table = TableState::new().multi_select(true);
    table.set_sort("name", SortDir::Ascending);
    let mut s = State { rows, table };
    sort(&mut s, "name", SortDir::Ascending);
    let third = s.rows[2].id.clone();
    s.table.select(third);
    s
}

fn size(b: u64) -> String {
    match b {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b => format!("{:.0} KB", b as f64 / 1024.0),
    }
}

fn thousands(n: usize) -> String {
    let d = n.to_string();
    d.chars().enumerate().fold(String::new(), |mut out, (i, ch)| {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
        out
    })
}

fn sort(s: &mut State, col: &str, dir: SortDir) {
    let by = |a: &Artifact, b: &Artifact| match col {
        "name" => rune_kit::natural_cmp(&a.name, &b.name),
        "target" => a.target.cmp(b.target),
        "size" => a.size.cmp(&b.size),
        "time" => a.ms.cmp(&b.ms),
        _ => a.cached.cmp(&b.cached),
    };
    s.rows.sort_by(|a, b| dir.apply(by(a, b)));
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let badge_k = k.clone();
    let table = Table::new(|s: &State| &s.table, |s: &mut State| &mut s.table, |s: &State| s.rows.len(), |s: &State, i| Cow::Borrowed(s.rows[i].id.as_str()))
        .name("Build artifacts")
        .column(TableColumn::new("name", "Artifact").flex(2.0).min(160.0).sortable())
        .column(TableColumn::new("target", "Target").flex(1.0).min(110.0).max(260.0).sortable())
        .column(TableColumn::new("size", "Size").width(90.0).min(70.0).max(160.0).align(Align::End).sortable())
        .column(TableColumn::new("time", "Time").width(80.0).min(64.0).max(140.0).align(Align::End).sortable())
        .column(TableColumn::new("cache", "Cache").width(84.0).resizable(false).sortable())
        .cell(move |s: &State, i, col| {
            let a = &s.rows[i];
            match col {
                "name" => Cell::text(&a.name),
                "target" => Cell::text(a.target).muted(),
                "size" => Cell::text(size(a.size)).mono(),
                "time" => Cell::text(format!("{:.1}s", a.ms as f64 / 1000.0)).mono(),
                _ => {
                    let (dot, label) = if a.cached { (badge_k.c.success.icon, "Hit") } else { (badge_k.c.icon_faint, "Miss") };
                    Cell::el(
                        row()
                            .gap(6.0)
                            .center_y()
                            .child(el().size(6.0, 6.0).radius(3.0).fill(dot))
                            .child(text(label).font(badge_k.ty.text_small).color(badge_k.c.text_muted)),
                    )
                }
            }
        })
        .on_sort(|s: &mut State, col, dir, _| sort(s, col, dir))
        .zebra(true)
        .build(s, &k)
        .grow(1.0)
        .min_h(0.0);
    let count = s.table.selected_len();
    let status = text(format!("{} artifacts · {count} selected", thousands(s.rows.len()))).font(k.ty.text_small).color(k.c.text_muted);
    col()
        .w_full()
        .h_full()
        .fill(k.c.page)
        .padding(12.0)
        .gap(8.0)
        .child(col().key("frame").w_full().grow(1.0).min_h(0.0).radius(k.radius.md).fill(k.c.bg_base).clip_rounded(k.radius.md).child(table))
        .child(row().key("status").w_full().child(status))
}

fn main() {
    rune::run(AppSpec::new("Table", (760.0, 480.0), state, view));
}
