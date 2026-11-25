//! Read-only document and source code preview demonstrating opt-in `El::caret_browsing(true)`
//! on `El::selection_group(true)`: visible 1.5 pt caret, arrow/word/line/page/document navigation,
//! `Shift`+selection across sibling blocks, plain Left/Right collapse, scroll-to-caret, and Copy.

use rune::{El, ScrollSpec, Tx, col, row, text};
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

pub(super) fn text_caret_browsing(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let status = s.pick.get("cb-status").map_or(
        "Click or Tab into the preview. Arrows, Ctrl/Option+arrows, Home/End, PageUp/PageDown, Shift+arrows, Cmd/Ctrl+A, and Cmd/Ctrl+C work.",
        String::as_str,
    );

    let mut code = col()
        .key("cb-code")
        .label("Read-only source preview")
        .w_full()
        .gap(2.0)
        .selection_group(true)
        .caret_browsing(true)
        .on_selection_change(|s: &mut G, change, tx: &mut Tx| {
            let msg = match change.selection() {
                Some(sel) => format!("Selected {} bytes: {:?}", sel.text().len(), sel.text()),
                None => "Caret browsing active (selection collapsed or cleared)".into(),
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

    col()
        .w(540.0)
        .gap(12.0)
        .child(text("Opt-in caret browsing (El::caret_browsing)").font(k.ty.ui_medium).color(k.c.text_strong))
        .child(
            col()
                .key("cb-scroll")
                .w_full()
                .h(180.0)
                .padding(12.0)
                .fill(k.c.layer_01)
                .border(1.0, k.c.border_weak)
                .radius(6.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(code),
        )
        .child(text(status).key("cb-status").font(k.ty.small).color(k.c.text_muted).wrap())
}
