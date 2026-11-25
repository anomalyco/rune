//! Read-only document and source code preview demonstrating opt-in `El::caret_browsing(true)`
//! on both mounted `El::selection_group(true)` blocks and multi-thousand-row `El::virtual_rows`:
//! visible 1.5 pt caret, arrow/word/line/page/document navigation, `Shift`+selection across
//! unmounted virtual rows, plain Left/Right collapse, scroll-to-caret, and Copy.

use rune::{El, GrowthSpec, ScrollSpec, Tx, col, row, text};
use rune_kit::*;

use crate::G;

const SAMPLE_CODE: &[&str] = &[
    "pub fn render_document(lines: &[&str]) -> El {",
    "    let mut group = col()",
    "        .selection_group(true)",
    "        .caret_browsing(true);",
    "    for (idx, line) in lines.iter().enumerate() {",
    "        group = group.child(",
    "            row()",
    "                .gap(12.0)",
    "                .child(text(format!(\"{idx:>2}\")).color(gutter))",
    "                .child(text(*line).selectable(true)),",
    "        );",
    "    }",
    "    group",
    "}",
];

const VIRTUAL_LINE_COUNT: usize = 2_000;

pub(super) fn text_caret_browsing(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let status = s.pick.get("cb-status").map_or(
        "Click or Tab into either preview. Arrows, Ctrl/Option+arrows, Home/End, Ctrl/Cmd+Home/End, PageUp/PageDown, Shift+selection, and Cmd/Ctrl+C work across mounted and unmounted virtual_rows.",
        String::as_str,
    );

    let mut code = col()
        .key("cb-code")
        .label("Mounted source preview")
        .w_full()
        .gap(2.0)
        .selection_group(true)
        .caret_browsing(true)
        .on_selection_change(|s: &mut G, change, tx: &mut Tx| {
            let msg = match change.selection() {
                Some(sel) => format!("Mounted: selected {} bytes ({} lines)", sel.text().len(), sel.text().lines().count()),
                None => "Mounted: caret browsing active (selection collapsed or cleared)".into(),
            };
            s.pick.insert("cb-status".into(), msg);
            tx.instant();
        });

    for (idx, line) in SAMPLE_CODE.iter().enumerate() {
        code = code.child(
            row()
                .w_full()
                .gap(12.0)
                .child(
                    text(format!("{:>2}", idx + 1))
                        .w(20.0)
                        .font(k.ty.mono)
                        .color(k.c.text_muted),
                )
                .child(
                    text(*line)
                        .key(format!("cb-line-{idx}"))
                        .font(k.ty.mono)
                        .color(k.c.std_text)
                        .caret(k.c.text_strong, k.selection())
                        .selectable(true),
                ),
        );
    }

    let mono = k.ty.mono;
    let gutter_c = k.c.text_muted;
    let text_c = k.c.std_text;
    let caret_c = k.c.text_strong;
    let sel_c = k.selection();

    let virtual_code = col()
        .key("cb-virtual")
        .label("2,000-line virtual_rows source preview")
        .w_full()
        .growth(GrowthSpec::default())
        .selection_group(true)
        .caret_browsing(true)
        .on_selection_change(|s: &mut G, change, tx: &mut Tx| {
            let msg = match change.selection() {
                Some(sel) => format!(
                    "Virtual (2,000 rows): selected {} bytes across {} lines{}",
                    sel.text().len(),
                    sel.text().lines().count(),
                    if sel.truncated() { " [truncated]" } else { "" },
                ),
                None => "Virtual (2,000 rows): caret browsing active (selection collapsed or cleared)".into(),
            };
            s.pick.insert("cb-status".into(), msg);
            tx.instant();
        })
        .virtual_rows(
            VIRTUAL_LINE_COUNT,
            20.0,
            |_: &G, i| format!("vr-{i}").into(),
            move |_: &G, i| {
                let sample = SAMPLE_CODE[i % SAMPLE_CODE.len()];
                row()
                    .w_full()
                    .h(20.0)
                    .gap(12.0)
                    .child(text(format!("{:>4}", i + 1)).w(36.0).font(mono).color(gutter_c))
                    .child(
                        text(format!("// line {:04}  {sample}", i + 1))
                            .key(format!("cb-vline-{i}"))
                            .font(mono)
                            .color(text_c)
                            .caret(caret_c, sel_c)
                            .selectable(true),
                    )
            },
        );

    col()
        .w(560.0)
        .gap(12.0)
        .child(text("Opt-in caret browsing (El::caret_browsing)").font(k.ty.ui_medium).color(k.c.text_strong))
        .child(
            col()
                .key("cb-scroll")
                .w_full()
                .h(140.0)
                .padding(12.0)
                .fill(k.c.layer_01)
                .border(1.0, k.c.border_weak)
                .radius(6.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(code),
        )
        .child(text("2,000-row virtual_rows preview (try Shift+PageDown, Shift+Ctrl/Cmd+End, Cmd/Ctrl+C)").font(k.ty.small_medium).color(k.c.text_strong))
        .child(
            col()
                .key("cb-vscroll")
                .w_full()
                .h(160.0)
                .padding(12.0)
                .fill(k.c.layer_01)
                .border(1.0, k.c.border_weak)
                .radius(6.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(virtual_code),
        )
        .child(text(status).key("cb-status").font(k.ty.small).color(k.c.text_muted).wrap())
}
