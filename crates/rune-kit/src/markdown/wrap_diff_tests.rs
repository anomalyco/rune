use std::cell::RefCell;
use std::rc::Rc;

use rune::render::KIND_GLYPH;
use rune::{Cx, Env, Runtime};

use super::*;
use crate::{Button, Kit};

fn glyph_bounds_in_y<T: 'static>(rt: &Runtime<T>, scale: f32, y_min: f32, y_max: f32) -> (usize, f32, f32) {
    let mut count = 0usize;
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for inst in &rt.instances {
        if inst.params[1] != KIND_GLYPH {
            continue;
        }
        let gx0 = inst.rect[0] / scale;
        let gy = (inst.rect[1] + inst.rect[3] * 0.5) / scale;
        let gx1 = (inst.rect[0] + inst.rect[2]) / scale;
        if gy >= y_min && gy <= y_max {
            count += 1;
            min_x = min_x.min(gx0);
            max_x = max_x.max(gx1);
        }
    }
    (count, min_x, max_x)
}

fn find_by_suffix<T: 'static>(rt: &Runtime<T>, suffix: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element ending with {suffix:?}"))
}

#[test]
fn wrap_anywhere_wraps_long_tokens_in_paragraphs_headings_code_diffs_and_tables() {
    struct State {
        doc: String,
        wrap_anywhere: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = MarkdownStyle::neutral(true).table_min_column(80.0);
        col().w_full().child(
            col().w(260.0).child(
                Markdown::new(s.doc.clone())
                    .wrap_anywhere(s.wrap_anywhere)
                    .render(&style)
                    .key("md"),
            ),
        )
    }

    let token = "x".repeat(96);
    let doc = format!(
        "# {token}\n\n{token}\n\n- {token}\n\n> {token}\n\n```rust\n{token}\n```\n\n```diff\n+{token}\n```\n\n| Key | Token |\n|---|---|\n| id | {token} |\n"
    );

    let mut off = Runtime::new(State { doc: doc.clone(), wrap_anywhere: false }, view, (680.0, 1800.0), Env::new(true, 1.0));
    off.frame(0.0);

    let mut on = Runtime::new(State { doc, wrap_anywhere: true }, view, (680.0, 1800.0), Env::new(true, 1.0));
    on.frame(0.0);

    let col_w = 260.0;
    let (_, _, off_max_x) = glyph_bounds_in_y(&off, 1.0, 0.0, 1800.0);
    assert!(off_max_x > col_w + 50.0, "default wrap_anywhere(false) must let long unbroken tokens overflow: max_x={off_max_x}");

    let (on_count, on_min_x, on_max_x) = glyph_bounds_in_y(&on, 1.0, 0.0, 1800.0);
    assert!(on_count > 400, "expected painted glyphs for all blocks, got {on_count}");
    assert!(on_min_x >= -1.5, "glyphs must stay inside column left edge: min_x={on_min_x}");
    assert!(on_max_x <= col_w + 1.0, "wrap_anywhere(true) must keep every glyph within {col_w} pt, got max_x={on_max_x}");

    // Every block (heading, paragraph, list, quote, code, diff, table) grows vertically when wrapped.
    for block_key in ["b0", "b1", "b2", "b3", "b4", "b5", "b6"] {
        let off_rect = find_by_suffix(&off, &format!("md/{block_key}")).rect;
        let on_rect = find_by_suffix(&on, &format!("md/{block_key}")).rect;
        assert!(
            on_rect[3] > off_rect[3] * 1.5,
            "block {block_key} should wrap onto multiple lines with wrap_anywhere(true): off={off_rect:?} on={on_rect:?}"
        );
    }

    // Accessibility roles remain intact under wrap_anywhere(true) in both Runtime elements and the web a11y mirror.
    let roles: Vec<&str> = on.elements().into_iter().map(|e| e.role).collect();
    for expected in ["heading", "list", "blockquote", "code", "table", "cell"] {
        assert!(roles.iter().any(|r| *r == expected), "element tree missing role {expected:?}: {roles:?}");
    }
    let a11y = on.a11y_json();
    for expected in ["\"heading\"", "\"ul\"", "\"li\"", "\"table\"", "\"cell\""] {
        assert!(a11y.contains(expected), "web a11y mirror missing {expected}: {a11y}");
    }
}

#[test]
fn wrap_anywhere_keeps_normal_word_breaks_and_handles_empty_and_huge_tokens() {
    struct State {
        src: String,
        wrap_anywhere: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().child(
            col().w(240.0).child(
                Markdown::new(s.src.clone())
                    .wrap_anywhere(s.wrap_anywhere)
                    .render(&MarkdownStyle::neutral(true))
                    .key("md"),
            ),
        )
    }

    // 1. Normal words separated by spaces produce identical heights with wrap_anywhere off vs on.
    let prose = "Normal words break at spaces even in a narrow column when no single word exceeds the available measure.";
    let mut off = Runtime::new(State { src: prose.into(), wrap_anywhere: false }, view, (480.0, 600.0), Env::new(true, 1.0));
    off.frame(0.0);
    let mut on = Runtime::new(State { src: prose.into(), wrap_anywhere: true }, view, (480.0, 600.0), Env::new(true, 1.0));
    on.frame(0.0);

    let off_b0 = find_by_suffix(&off, "md/b0").rect;
    let on_b0 = find_by_suffix(&on, "md/b0").rect;
    assert!((off_b0[3] - on_b0[3]).abs() < 0.5, "normal word wrapping must stay identical: off={off_b0:?} on={on_b0:?}");

    // 2. Empty document does not panic and has zero height.
    let mut empty = Runtime::new(State { src: String::new(), wrap_anywhere: true }, view, (480.0, 400.0), Env::new(true, 1.0));
    empty.frame(0.0);
    assert_eq!(find_by_suffix(&empty, "md").rect[3], 0.0);

    // 3. Huge unbroken token (500 characters) stays inside 240 pt column.
    let huge = "a".repeat(500);
    let mut big = Runtime::new(State { src: huge, wrap_anywhere: true }, view, (640.0, 1200.0), Env::new(true, 1.0));
    big.frame(0.0);
    let (count, _, max_x) = glyph_bounds_in_y(&big, 1.0, 0.0, 1200.0);
    assert!(count >= 500, "all glyphs of huge token should paint, got {count}");
    assert!(max_x <= 241.0, "huge unbroken token must stay within 240 pt column, got max_x={max_x}");
}

#[test]
fn wrap_anywhere_with_scroll_wide_keeps_code_scrolling_while_wrapping_prose_and_diffs() {
    fn view(doc: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(
            col().w(240.0).child(
                Markdown::new(doc.clone())
                    .scroll_wide(true)
                    .wrap_anywhere(true)
                    .render(&MarkdownStyle::neutral(true))
                    .key("md"),
            ),
        )
    }

    let token = "z".repeat(100);
    let doc = format!("{token}\n\n```rust\n{token}\n```\n\n```diff\n+{token}\n```\n");

    let mut rt = Runtime::new(doc, view, (640.0, 1000.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Paragraph (b0) wraps inside 240 pt.
    let para = find_by_suffix(&rt, "md/b0").rect;
    assert!(para[3] > 50.0, "paragraph must wrap under scroll_wide + wrap_anywhere: {para:?}");
    let (_, _, para_max_x) = glyph_bounds_in_y(&rt, 1.0, para[1], para[1] + para[3]);
    assert!(para_max_x <= 241.0, "paragraph glyphs must fit 240 pt column: max_x={para_max_x}");

    // Code block (b1) keeps horizontal scroll viewport and does NOT wrap its long line.
    let scroll = find_by_suffix(&rt, "md/b1/scroll");
    let code_text = rt.elements().into_iter().find(|e| e.text.as_deref() == Some(token.as_str()) && e.path.contains("/b1/")).unwrap();
    assert!(code_text.rect[2] > 400.0, "code block under scroll_wide(true) must stay single-line and scrollable: {:?}", code_text.rect);
    assert!(scroll.rect[2] <= 241.0, "scroll viewport stays within 240 pt column: {:?}", scroll.rect);

    // Diff block (b2) wraps its long line inside 240 pt.
    let diff = find_by_suffix(&rt, "md/b2").rect;
    assert!(diff[3] > 50.0, "diff block must wrap under wrap_anywhere(true): {diff:?}");
    let (_, _, diff_max_x) = glyph_bounds_in_y(&rt, 1.0, diff[1], diff[1] + diff[3]);
    assert!(diff_max_x <= 241.0, "diff glyphs must fit 240 pt column: max_x={diff_max_x}");
}

#[test]
fn wrap_anywhere_wraps_rtl_paragraph_within_column() {
    struct State {
        doc: String,
        wrap: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().child(
            col().w(240.0).child(
                Markdown::new(s.doc.clone())
                    .wrap_anywhere(s.wrap)
                    .render(&MarkdownStyle::neutral(true))
                    .key("md"),
            ),
        )
    }

    let url = format!("https://example.org/{}", "pathsegment".repeat(8));
    let doc = format!("مرحبا بالعالم {url}");

    let mut off = Runtime::new(State { doc: doc.clone(), wrap: false }, view, (640.0, 600.0), Env::new(true, 1.0));
    off.frame(0.0);
    let mut on = Runtime::new(State { doc: doc.clone(), wrap: true }, view, (640.0, 600.0), Env::new(true, 1.0));
    on.frame(0.0);

    let blocks = parse(&doc);
    assert_eq!(bidi::Dirs::new(&blocks).of_slice(&blocks), Some(rune::Direction::Rtl));

    let off_b0 = find_by_suffix(&off, "md/b0").rect;
    let on_b0 = find_by_suffix(&on, "md/b0").rect;
    assert!(on_b0[3] > off_b0[3] * 1.5, "RTL paragraph with long token must wrap: off={off_b0:?} on={on_b0:?}");

    let (count, min_x, max_x) = glyph_bounds_in_y(&on, 1.0, on_b0[1], on_b0[1] + on_b0[3]);
    assert!(count > 40, "expected RTL + token glyphs, got {count}");
    assert!(min_x >= -1.0 && max_x <= 241.0, "RTL wrapped glyphs must stay in [0, 240]: [{min_x}, {max_x}]");
}

#[test]
fn wrap_anywhere_toggle_invalidates_whole_doc_and_settled_memo_caches() {
    struct State {
        doc: String,
        wrap_anywhere: bool,
        streaming: bool,
    }

    fn pure_view(s: &State, _: &mut Cx<State>) -> El {
        let style = MarkdownStyle::neutral(true).table_min_column(80.0);
        col().w_full().child(
            col().w(240.0).child(Markdown::new(s.doc.clone()).wrap_anywhere(s.wrap_anywhere).render(&style).key("md")),
        )
    }

    fn stream_view(s: &State, _: &mut Cx<State>) -> El {
        let style = MarkdownStyle::neutral(true).table_min_column(80.0);
        col().w_full().child(
            col().w(240.0).child(
                Markdown::new(s.doc.clone())
                    .streaming(s.streaming)
                    .wrap_anywhere(s.wrap_anywhere)
                    .render(&style)
                    .key("md"),
            ),
        )
    }

    let token = "m".repeat(96);
    let doc = format!("{token}\n\n```rust\n{token}\n```\n\n```diff\n+{token}\n```\n\n| K | V |\n|---|---|\n| a | {token} |\n\ntail");

    // 1. Pure markdown (uses whole-doc memo cache in Markdown::render).
    let mut rt = Runtime::new(State { doc: doc.clone(), wrap_anywhere: false, streaming: false }, pure_view, (480.0, 1400.0), Env::new(true, 1.0));
    RENDERS.with(|n| n.set(0));
    rt.frame(0.0);
    let r0 = RENDERS.with(Cell::get);
    let h_off = find_by_suffix(&rt, "md/b0").rect[3];
    let table_off = find_by_suffix(&rt, "md/b3").rect[3];

    rt.update(|s, tx| { tx.instant(); s.wrap_anywhere = true; });
    rt.frame(1.0 / 60.0);
    assert!(RENDERS.with(Cell::get) > r0, "toggling wrap_anywhere must miss whole-doc memo cache");
    let h_on = find_by_suffix(&rt, "md/b0").rect[3];
    let table_on = find_by_suffix(&rt, "md/b3").rect[3];
    assert!(h_on > h_off * 1.5, "whole-doc memo must invalidate paragraph on wrap_anywhere toggle: {h_off} -> {h_on}");
    assert!(table_on > table_off * 1.5, "whole-doc memo must invalidate table on wrap_anywhere toggle: {table_off} -> {table_on}");

    rt.update(|s, tx| { tx.instant(); s.wrap_anywhere = false; });
    rt.frame(1.0 / 60.0);
    assert!((find_by_suffix(&rt, "md/b0").rect[3] - h_off).abs() < 0.5, "toggling back off must restore original height");

    // 2. Streaming markdown (uses settled_block_digest and settled_table_row_digest).
    let mut srt = Runtime::new(State { doc, wrap_anywhere: false, streaming: true }, stream_view, (480.0, 1400.0), Env::new(true, 1.0));
    reset_block_builds();
    srt.frame(0.0);
    let builds_off = block_builds();
    let sb0_off = find_by_suffix(&srt, "md/b0").rect[3];
    let sb1_off = find_by_suffix(&srt, "md/b1").rect[3];
    let sb2_off = find_by_suffix(&srt, "md/b2").rect[3];
    let sb3_off = find_by_suffix(&srt, "md/b3").rect[3];

    srt.update(|s, tx| { tx.instant(); s.wrap_anywhere = true; });
    srt.frame(1.0 / 60.0);
    assert!(block_builds() > builds_off, "toggling wrap_anywhere must invalidate settled-block memo");
    let sb0_on = find_by_suffix(&srt, "md/b0").rect[3];
    let sb1_on = find_by_suffix(&srt, "md/b1").rect[3];
    let sb2_on = find_by_suffix(&srt, "md/b2").rect[3];
    let sb3_on = find_by_suffix(&srt, "md/b3").rect[3];
    assert!(sb0_on > sb0_off * 1.5, "settled para must invalidate: {sb0_off} -> {sb0_on}");
    assert!(sb1_on > sb1_off * 1.5, "settled code must invalidate: {sb1_off} -> {sb1_on}");
    assert!(sb2_on > sb2_off * 1.5, "settled diff must invalidate: {sb2_off} -> {sb2_on}");
    assert!(sb3_on > sb3_off * 1.5, "settled table row must invalidate: {sb3_off} -> {sb3_on}");
}

#[test]
fn diff_frame_wraps_diff_and_patch_fences_and_leaves_code_frame_unchanged() {
    struct State {
        copied: Vec<String>,
        diffs: Rc<RefCell<Vec<(usize, String, bool)>>>,
        codes: Rc<RefCell<Vec<(usize, Option<String>, String, bool)>>>,
    }

    const DOC: &str = concat!(
        "```rust\nlet a = 1;\n```\n\n",
        "```diff\n@@ -1 +1 @@\n-old\n+new\n```\n\n",
        "```js\nconst b = 2;\n```\n\n",
        "```patch title=\"fix.patch\"\n-foo\n+bar\n```\n\n",
        "> ```DIFF\n> +in_quote\n> ```\n\n",
        "- item\n\n  ```patch\n  -in_list\n  ```\n\n",
        "```\nplain\n```\n",
    );

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let d_log = s.diffs.clone();
        let c_log = s.codes.clone();
        col().w(480.0).child(
            Markdown::new(DOC)
                .code_frame(move |card, c| {
                    c_log.borrow_mut().push((c.index, c.language.map(str::to_owned), c.source.to_owned(), c.open));
                    col().key(format!("code-wrap-{}", c.index)).w_full().child(card)
                })
                .diff_frame(move |card, d| {
                    d_log.borrow_mut().push((d.index, d.source.to_owned(), d.open));
                    let patch = d.source.to_owned();
                    let label = format!("Copy patch {}", d.index);
                    col()
                        .key(format!("diff-wrap-{}", d.index))
                        .w_full()
                        .child(
                            Button::new(label)
                                .on_press(move |s: &mut State| s.copied.push(patch.clone()))
                                .build(&Kit::neutral(true))
                                .key(format!("copy-diff-{}", d.index)),
                        )
                        .child(card)
                })
                .build(&k)
                .key("md"),
        )
    }

    let diffs = Rc::new(RefCell::new(Vec::new()));
    let codes = Rc::new(RefCell::new(Vec::new()));
    let mut rt = Runtime::new(
        State { copied: Vec::new(), diffs: diffs.clone(), codes: codes.clone() },
        view,
        (520.0, 1200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    assert_eq!(
        *diffs.borrow(),
        vec![
            (0, "@@ -1 +1 @@\n-old\n+new".to_owned(), false),
            (1, "-foo\n+bar".to_owned(), false),
            (2, "+in_quote".to_owned(), false),
            (3, "-in_list".to_owned(), false),
        ],
        "diff_frame must wrap diff and patch fences (including nested ones) with 0-based index and exact source"
    );
    assert_eq!(
        *codes.borrow(),
        vec![
            (0, Some("rust".to_owned()), "let a = 1;".to_owned(), false),
            (1, Some("js".to_owned()), "const b = 2;".to_owned(), false),
            (2, None, "plain".to_owned(), false),
        ],
        "code_frame indices and callbacks must stay untouched by diff fences"
    );

    // Inner diff card keeps key("card") and role("code"), and button inside diff_frame is interactive & accessible.
    let card0 = find_by_suffix(&rt, "md/b1/card");
    assert_eq!(card0.role, "code");
    let a11y = rt.a11y_json();
    assert!(a11y.contains("Copy patch 0"), "diff_frame button must reach accessibility tree: {a11y}");
    assert!(a11y.contains("Copy patch 1"), "second diff_frame button must reach accessibility tree: {a11y}");

    let btn1 = find_by_suffix(&rt, "copy-diff-1").rect;
    let (bx, by) = (btn1[0] + btn1[2] * 0.5, btn1[1] + btn1[3] * 0.5);
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.0);
    assert_eq!(rt.state.copied, vec!["-foo\n+bar".to_owned()]);
}

#[test]
fn diff_frame_tracks_streaming_open_state() {
    let seen = Rc::new(RefCell::new(Vec::<(usize, String, bool)>::new()));
    let run = |src: &str| {
        seen.borrow_mut().clear();
        let s = seen.clone();
        let _ = Markdown::new(src)
            .streaming(true)
            .diff_frame(move |card, d| {
                s.borrow_mut().push((d.index, d.source.to_owned(), d.open));
                card
            })
            .render(&MarkdownStyle::neutral(true));
        seen.borrow().clone()
    };

    assert_eq!(run("```diff\n"), vec![(0, String::new(), true)]);
    assert_eq!(run("```diff\n-old\n+new"), vec![(0, "-old\n+new".to_owned(), true)]);
    assert_eq!(run("```diff\n-old\n+new\n```"), vec![(0, "-old\n+new".to_owned(), false)]);
    assert_eq!(
        run("```diff\n-a\n+b\n```\n\n```patch\n-c\n+d"),
        vec![(0, "-a\n+b".to_owned(), false), (1, "-c\n+d".to_owned(), true)]
    );
    assert_eq!(
        run("```diff\n-a\n+b\n```\n\n```patch\n-c\n+d\n```"),
        vec![(0, "-a\n+b".to_owned(), false), (1, "-c\n+d".to_owned(), false)]
    );
}

#[test]
fn diff_frame_never_skipped_by_memo_while_inner_card_and_siblings_memoize_and_highlights_work() {
    struct State {
        label: &'static str,
        wrap_anywhere: bool,
        calls: Rc<Cell<usize>>,
    }

    const DOC: &str = "Before paragraph.\n\n```diff\n-needle_old\n+needle_new\n```\n\nAfter paragraph.\n";

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let calls_view = s.calls.clone();
        let header = s.label;
        let matches = find(DOC, "needle", Case::Sensitive);
        col().w(360.0).child(
            Markdown::new(DOC)
                .wrap_anywhere(s.wrap_anywhere)
                .highlights(matches)
                .diff_frame(move |card, d| {
                    calls_view.set(calls_view.get() + 1);
                    col()
                        .key(format!("df-{}", d.index))
                        .w_full()
                        .child(text(header).key("hdr"))
                        .child(card)
                })
                .render(&MarkdownStyle::neutral(true))
                .key("md"),
        )
    }

    let calls = Rc::new(Cell::new(0usize));
    reset_block_builds();
    let mut rt = Runtime::new(State { label: "Copy", wrap_anywhere: false, calls: calls.clone() }, view, (420.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(calls.get(), 1);
    assert_eq!(find_by_suffix(&rt, "hdr").text.as_deref(), Some("Copy"));
    // Highlights inside Block::Diff still map through BlockMap::Diff.
    let marks: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("md-mark-"))).collect();
    assert_eq!(marks.len(), 2, "both diff lines containing 'needle' must receive highlight marks: {marks:?}");

    // Changing only external state re-runs diff_frame (updating the header), while settled siblings
    // and the inner diff card hit their memo caches (0 fresh block builds).
    reset_block_builds();
    rt.update(|s, _| s.label = "Copied");
    rt.frame(0.0);
    assert_eq!(calls.get(), 2, "memoization must never skip diff_frame");
    assert_eq!(find_by_suffix(&rt, "hdr").text.as_deref(), Some("Copied"));
    assert_eq!(block_builds(), 0, "settled siblings and closed inner diff card should hit memo cache");

    // Changing wrap_anywhere invalidates the inner diff card memo digest.
    reset_block_builds();
    rt.update(|s, _| s.wrap_anywhere = true);
    rt.frame(0.0);
    assert_eq!(calls.get(), 3);
    assert!(block_builds() > 0, "toggling wrap_anywhere must invalidate cached blocks and diff card");
}

#[test]
fn cjk_inline_code_chains_wrap_between_spans_and_keep_find_copy_and_streaming_clean() {
    use rune::render::KIND_RECT;

    const CODE_BG: Color = Color { r: 0.2, g: 0.4, b: 0.8, a: 1.0 };
    const CHAIN: &str = "主要源码入口：`src/net.rs`、`src/updater.rs`、`src/transcription/definitions.rs`、`src/desktop/opencode_service.rs`、`crates/router/src/routing/jev.rs`、`src/personal_commands/fetch.rs`。本轮是静态源码审计，没有对用户机器抓包或读取实际配置。";

    struct State {
        src: String,
        col_w: f32,
        wrap_anywhere: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = MarkdownStyle::neutral(true).inline_code_bg(CODE_BG);
        col().w_full().child(
            col().w(s.col_w).child(
                Markdown::new(s.src.clone())
                    .selectable(true)
                    .wrap_anywhere(s.wrap_anywhere)
                    .render(&style)
                    .key("md"),
            ),
        )
    }

    let chip_rects = |rt: &Runtime<State>| -> Vec<[f32; 4]> {
        let want = CODE_BG.premul(1.0);
        rt.instances
            .iter()
            .filter(|i| {
                i.params[1] == KIND_RECT
                    && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)
            })
            .map(|i| i.rect)
            .collect()
    };

    // 1. Both wrap_anywhere(false) and wrap_anywhere(true) wrap the chain between inline code
    // spans without overflowing or splitting any individual code span across lines.
    let col_w = 420.0;
    for wrap_anywhere in [false, true] {
        let mut rt = Runtime::new(
            State { src: CHAIN.into(), col_w, wrap_anywhere },
            view,
            (800.0, 600.0),
            Env::new(true, 1.0),
        );
        rt.set_srgb_blend(false);
        rt.frame(0.0);
        let (count, min_x, max_x) = glyph_bounds_in_y(&rt, 1.0, 0.0, 600.0);
        assert!(count > 100, "expected painted glyphs, got {count}");
        assert!(min_x >= -1.5, "glyphs must stay inside left edge: min_x={min_x}");
        assert!(
            max_x <= col_w + 2.0,
            "wrap_anywhere={wrap_anywhere}: CJK inline code chain must stay within {col_w} pt, got max_x={max_x}"
        );
        let chips = chip_rects(&rt);
        assert_eq!(
            chips.len(),
            6,
            "wrap_anywhere={wrap_anywhere}: all 6 inline code spans must stay intact (1 chip each), got {chips:?}"
        );
        for chip in &chips {
            assert!(
                chip[0] + chip[2] <= col_w + 2.0,
                "wrap_anywhere={wrap_anywhere}: chip {chip:?} overflows {col_w} pt"
            );
        }
        // Multiple lines must be used and the first chip should sit on line 1 beside `主要源码入口：`.
        let distinct_rows: Vec<i32> = {
            let mut ys: Vec<i32> = chips.iter().map(|r| (r[1] / 10.0).round() as i32).collect();
            ys.dedup();
            ys
        };
        assert!(
            distinct_rows.len() >= 3,
            "wrap_anywhere={wrap_anywhere}: expected chips across >= 3 lines, got {chips:?}"
        );
        assert!(
            chips[0][1] < 16.0 && chips[0][0] > 70.0,
            "wrap_anywhere={wrap_anywhere}: first chip should start on line 1 after `主要源码入口：`, got {:?}",
            chips[0]
        );

        // Selecting all and copying omits U+200B, U+202F and bidi isolates completely.
        rt.update(|_, tx| tx.select_all_text("md"));
        rt.frame(1.0 / 60.0);
        let copied = rt.selected_text().unwrap_or_default();
        assert_eq!(
            copied,
            "主要源码入口：src/net.rs、src/updater.rs、src/transcription/definitions.rs、src/desktop/opencode_service.rs、crates/router/src/routing/jev.rs、src/personal_commands/fetch.rs。本轮是静态源码审计，没有对用户机器抓包或读取实际配置。",
            "copied text must strip code padding, isolates, and break opportunities"
        );
    }

    // 2. Directly flanked CJK text (`查看`a`和`b`文件` and ``a`（说明）`) wraps cleanly too.
    let flanked = "查看`src/net.rs`和`src/updater.rs`以及`crates/router/src/routing/jev.rs`（说明）文件";
    let mut rt = Runtime::new(
        State { src: flanked.into(), col_w: 280.0, wrap_anywhere: false },
        view,
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    let (_, _, max_x) = glyph_bounds_in_y(&rt, 1.0, 0.0, 400.0);
    assert!(max_x <= 282.0, "CJK-flanked code chain must stay within 280 pt, got max_x={max_x}");
    assert_eq!(chip_rects(&rt).len(), 3, "all 3 CJK-flanked code spans stay intact");

    // 3. Opening and closing punctuation stay attached to inline code (no U+200B inserted there).
    let brackets = "See (`alpha`), `beta`. 中文（`gamma`）「`delta`」。";
    let parsed = parse(brackets);
    let Block::Para(p) = &parsed[0] else { panic!("expected para") };
    assert!(!p.text.contains("(\u{200b}"), "opening '(' must stay attached to code: {:?}", p.text);
    assert!(!p.text.contains("（\u{200b}"), "opening '（' must stay attached to code: {:?}", p.text);
    assert!(!p.text.contains("「\u{200b}"), "opening '「' must stay attached to code: {:?}", p.text);
    assert!(!p.text.contains("\u{200b})"), "closing ')' must stay attached to code: {:?}", p.text);
    assert!(!p.text.contains("\u{200b}）"), "closing '）' must stay attached to code: {:?}", p.text);
    assert!(!p.text.contains("\u{200b}」"), "closing '」' must stay attached to code: {:?}", p.text);
    assert!(!p.text.contains("\u{200b}."), "period must stay attached to code: {:?}", p.text);
    assert!(!p.text.contains("\u{200b}。"), "CJK period must stay attached to code: {:?}", p.text);
    assert_eq!(unpadded(&p.text), "See (alpha), beta. 中文（gamma）「delta」。");

    // 4. Find across CJK punctuation + inline code merges highlight ranges cleanly through BREAK_OPEN.
    let matches = Markdown::find(CHAIN, "入口：src/net.rs、src/updater.rs", Case::Sensitive);
    assert_eq!(matches.len(), 1, "find must match across CJK punctuation and inline code");
    let (blocks, _, _, _, maps) = doc_cached(CHAIN);
    let (Block::Para(p), BlockMap::Inline(map)) = (&blocks[0], &maps[0]) else { panic!("expected para") };
    let mapped = map.map_ranges(&matches[0], &p.text);
    assert_eq!(mapped.len(), 1, "highlight across BREAK_OPEN/CLOSE must merge into one range: {mapped:?}");

    // 5. Link immediately following inline code does not absorb the closing U+200B into its range.
    let with_link = "`src/net.rs`[文档](https://example.com)";
    let link_blocks = parse(with_link);
    let Block::Para(lp) = &link_blocks[0] else { panic!("expected para") };
    assert_eq!(lp.links.len(), 1);
    let (la, lb, ref url) = lp.links[0];
    assert_eq!(url, "https://example.com");
    assert_eq!(&lp.text[la..lb], "文档", "link text must not include closing U+200B");

    // 6. Streaming prefix stability: growing character by character never alters earlier bytes.
    let sample = "入口：`src/net.rs`、`src/updater.rs`和`a.rs`。";
    let mut prev_text = String::new();
    for (idx, ch) in sample.char_indices() {
        let prefix = &sample[..idx + ch.len_utf8()];
        let blocks = stream::parse_streaming(prefix, true).0;
        if let Some(Block::Para(inl)) = blocks.first() {
            if !prev_text.is_empty() && !inl.text.starts_with(&prev_text) {
                // Only backticks closing a code span can drop backtick text; once parsed as code,
                // growing text should keep earlier code spans intact.
                assert!(
                    prefix.ends_with('`') || prev_text.ends_with('`'),
                    "unexpected non-monotonic stream change at {prefix:?}: {prev_text:?} -> {:?}",
                    inl.text
                );
            }
            prev_text = inl.text.clone();
        }
    }
}
