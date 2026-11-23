use super::*;
use rune::text::{GlyphQuad, TextSystem};

/// Each code block's syntax spans, in document order, rendered with `style`.
fn spans(src: &str, style: &MarkdownStyle) -> Vec<(String, Vec<Span>)> {
    spans_with(src, style, Highlighter::builtin())
}

/// [`spans`] with a highlighter.
fn spans_with(src: &str, style: &MarkdownStyle, highlighter: Highlighter) -> Vec<(String, Vec<Span>)> {
    fn codes<'a>(blocks: &'a [Block], out: &mut Vec<&'a str>) {
        for b in blocks {
            match b {
                Block::Code(c) | Block::Mermaid(c) => out.push(c),
                Block::Quote(inner) => codes(inner, out),
                Block::List { items, .. } => items.iter().for_each(|i| codes(i, out)),
                _ => {}
            }
        }
    }
    let (blocks, sources, tasks, _, _) = doc_cached(src);
    let r = Renderer { s: style.clone(), highlights: Vec::new(), reveal: 0.0, streaming: false, on_link: None, on_link_secondary: None, link_look: None, link_help: None, link_ring: None, code_link: None, on_task: None,
        tasks_read_only: false, selectable: false, scroll_wide: false, tentative: None, highlighter, sources: sources.clone(),
        next_source: Cell::new(0), tasks, next_task: Cell::new(0), current_block_key: RefCell::new("b0".into()),
        #[cfg(feature = "mermaid")] diagram_frame: None, code_frame: None, quote_frame: None, link_favicon: None, images: None, on_image: None, flow: Cell::default() };
    let mut found = Vec::new();
    codes(&blocks, &mut found);
    assert_eq!(found.len(), sources.len(), "one source per code block");
    found
        .into_iter()
        .zip(sources.iter())
        .map(|(code, s)| {
            assert_eq!(s.len, code.len(), "sources line up with blocks in document order");
            (code.to_owned(), r.syntax_spans(code, Some(s)))
        })
        .collect()
}

fn coloured<'a>(code: &'a str, spans: &[Span], c: Color) -> Vec<&'a str> {
    spans.iter().filter(|s| s.color == Some(c)).map(|s| &code[s.start as usize..s.end as usize]).collect()
}

#[test]
fn known_languages_are_coloured_and_others_stay_plain() {
    let style = MarkdownStyle::neutral(true);
    let src = "```ts\nconst a = 1\n```\n\n```brainfuck\n+[-]\n```\n\n    indented\n\n```\nnone\n```\n\n- ```rust,ignore\n  fn f() {}\n  ```\n\n> ```SQL\n> select 1\n> ```";
    let got = spans(src, &style);
    assert_eq!(got.len(), 6);
    let (ts, s) = &got[0];
    assert_eq!(coloured(ts, s, style.syntax.color(Syntax::Keyword)), ["const "]);
    assert_eq!(coloured(ts, s, style.syntax.color(Syntax::Number)), ["1"]);
    assert!(s.iter().all(|s| s.color.is_some() && s.weight.is_none() && !s.italic && !s.mono && s.size.is_none() && s.bg.is_none()));
    for (code, s) in &got[1..4] {
        assert!(s.is_empty(), "{code:?} stays plain");
    }
    assert_eq!(coloured(&got[4].0, &got[4].1, style.syntax.color(Syntax::Function)), ["f"]);
    assert_eq!(coloured(&got[5].0, &got[5].1, style.syntax.color(Syntax::Keyword)), ["select "]);
}

#[test]
fn a_streaming_fence_colours_only_its_finished_lines() {
    let style = MarkdownStyle::neutral(false);
    let last_end = |src: &str| spans(src, &style)[0].1.last().map_or(0, |s| s.end as usize);
    assert_eq!(last_end("```ts\nconst"), 0, "the first line is still arriving");
    assert_eq!(last_end("```ts\nconst a = 1\nconst b"), "const a = 1".len());
    assert_eq!(last_end("```ts\nconst a = 1\nconst b = 2\n"), "const a = 1\nconst b = 2".len());
    assert_eq!(last_end("```ts\nconst a = 1\nlet b = 2\n```"), "const a = 1\nlet b = 2".len());
}

#[test]
fn syntax_builders_override_and_switch_off_colour() {
    let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    let src = "```rust\nlet x = 1;\n```";
    let style = MarkdownStyle::neutral(true).syntax(Syntax::Keyword, red);
    let (code, s) = &spans(src, &style)[0];
    assert_eq!(coloured(code, s, red), ["let "]);
    assert!(spans(src, &style.syntax_highlight(false))[0].1.is_empty());
}

#[test]
fn a_highlighter_gets_each_fence_language_as_written() {
    use crate::syntax::Paint;
    thread_local!(static SEEN: RefCell<Vec<(Option<String>, String)>> = const { RefCell::new(Vec::new()) });
    let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    let h = Highlighter::new(move |lang: Option<&str>, code: &str| {
        SEEN.with_borrow_mut(|s| s.push((lang.map(str::to_owned), code.to_owned())));
        vec![(2..3, Paint::Color(red)), (0..1, Paint::Kind(Syntax::Keyword))]
    });
    let style = MarkdownStyle::neutral(true);
    let src = "```Kotlin title=\"a.kt\"\nval x = 1\n```\n\n```\nbare fence\n```\n\n    indented code\n\n```rust,ignore\nfn f() {}\n```\n\n```diff\n+a\n```";
    let got = spans_with(src, &style, h);
    let want = [(Some("Kotlin"), "val x = 1"), (None, "bare fence"), (None, "indented code"), (Some("rust"), "fn f() {}")];
    assert_eq!(SEEN.take(), want.map(|(l, c)| (l.map(str::to_owned), c.to_owned())), "diff fences keep their own look");
    for (code, s) in &got {
        assert_eq!(coloured(code, s, style.syntax.color(Syntax::Keyword)), [&code[0..1]]);
        assert_eq!(coloured(code, s, red), [&code[2..3]]);
    }
}

/// Apps that don't pass a highlighter get exactly the built-in one's colours, and with colour
/// off nothing runs.
#[test]
fn without_a_highlighter_the_output_is_the_builtin_ones() {
    let style = MarkdownStyle::neutral(false);
    let src = "```ts\nconst a = 1;\n```\n\n```\nplain\n```\n\n```cobol\nMOVE 1 TO X\n```";
    let got = spans(src, &style);
    let k = |s| style.syntax.color(s);
    let ts: Vec<_> = got[0].1.iter().map(|s| (s.start, s.end, s.color.unwrap())).collect();
    assert_eq!(ts, [(0, 6, k(Syntax::Keyword)), (6, 8, k(Syntax::Variable)), (8, 10, k(Syntax::Operator)), (10, 11, k(Syntax::Number)), (11, 12, k(Syntax::Punctuation))]);
    assert!(got[1].1.is_empty() && got[2].1.is_empty());
    let h = Highlighter::new(|_, _| panic!("colour is off"));
    assert!(spans_with(src, &style.clone().syntax_highlight(false), h).iter().all(|(_, s)| s.is_empty()));
}

/// Rebuilding a view highlights nothing: a highlighter runs once per block however many
/// commits draw it.
#[test]
fn rebuilding_a_view_runs_the_highlighter_once_per_block() {
    thread_local!(static RUNS: Cell<usize> = const { Cell::new(0) });
    fn view(_: &(), _: &mut rune::Cx<()>) -> El {
        let style = MarkdownStyle::neutral(true);
        Markdown::new("```py\nx = 1\n```\n\n```go\nvar y = 2\n```")
            .highlight(|lang, code| {
                RUNS.set(RUNS.get() + 1);
                crate::syntax::builtin(lang, code)
            })
            .render(&style)
    }
    let mut rt = rune::Runtime::new((), view, (400.0, 400.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..3 {
        rt.invalidate();
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(RUNS.get(), 2);
}

/// Colour is the only thing a highlight changes: every glyph lands in the same place, wrapped
/// or not, as in the plain block and in the block mid-stream.
#[test]
fn highlighting_never_moves_a_glyph() {
    const INTER: &[u8] = include_bytes!("../../../../themes/opencode/assets/fonts/InterVariable.ttf");
    const PLEX: &[u8] = include_bytes!("../../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
    let mut ts = TextSystem::new(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));
    let style = MarkdownStyle::neutral(true);
    let font = style.code_block;
    let mut quads = |code: &str, spans: &[Span], width: Option<f32>| {
        let l = ts.layout_rich(code, spans, font, 2.0, width);
        let mut out: Vec<GlyphQuad> = Vec::new();
        ts.glyphs_for_layout(&l, 2.0, [0.0, 0.0], true, &mut out);
        out.iter().map(|q| (q.rect.map(f32::to_bits), q.uv.map(f32::to_bits), q.byte)).collect::<Vec<_>>()
    };
    for (lang, code) in [
        ("tsx", "export function Card({ title }: { title: string }) {\n  return <div className=\"card\">{title.toUpperCase()}</div>; // done\n}"),
        ("rust", "fn main() { let v: Vec<u8> = vec![1, 2, 3]; println!(\"{v:?} → ok\"); }"),
        ("sh", "FOO=1 npm run build --if-present && echo \"$HOME/${X}\" | tee out.txt # café"),
        ("yaml", "name: rune\nlist: [a, 1, {b: c}]\nscript: |\n  echo hi"),
    ] {
        let closed = format!("```{lang}\n{code}\n```");
        let (shown, full) = spans(&closed, &style).remove(0);
        assert_eq!(shown, code);
        assert!(!full.is_empty(), "{lang} gets colour");
        let cut = code.find('\n').map_or(code.len() / 2, |n| n + 4);
        let cut = (cut..).find(|&i| code.is_char_boundary(i)).unwrap();
        let (_, streaming) = spans(&format!("```{lang}\n{}", &code[..cut]), &style).remove(0);
        for width in [None, Some(900.0), Some(420.0), Some(260.0)] {
            let plain = quads(code, &[], width);
            assert_eq!(plain, quads(code, &full, width), "{lang} at {width:?}");
            assert_eq!(plain, quads(code, &streaming, width), "{lang} mid-stream at {width:?}");
        }
    }
}

fn parses() -> (usize, usize) {
    PARSES.get()
}

/// A transcript longer than any fixed entry count re-renders without parsing.
#[test]
fn a_column_of_forty_documents_parses_each_once() {
    fn view(_: &(), _: &mut rune::Cx<()>) -> El {
        let style = MarkdownStyle::neutral(true);
        col().children((0..40).map(|i| Markdown::new(format!("para {i}\n\n- item")).render(&style).key(format!("m{i}"))))
    }
    let before = parses();
    let mut rt = rune::Runtime::new((), view, (400.0, 800.0), rune::Env::default());
    rt.frame(0.0);
    assert_eq!(parses().0 - before.0, 40);
    for _ in 0..3 {
        rt.invalidate();
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(parses().0 - before.0, 40, "later commits parse nothing");
}

/// Feeding a document in a few bytes at a time parses only from its last block, and gives
/// exactly what parsing each string whole gives.
#[test]
fn a_stream_reparses_from_its_last_block_and_matches_a_whole_parse() {
    let docs = [
        "# Title\n\nFirst para with **bold** and `code`.\nSetext\n---\n\n- one\n- two\n\n  more of two\n\n- [ ] todo\n- [x] done\n\n1. a\n2. b\n\nAfter the list.\n\n> quoted\n> lines\n\n```rust\nfn main() {}\n```\n\n    indented code\n\n***\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n\ntail text <span>html</span>\n\n<div>\nblock\n</div>\n\nlast *words*",
        "para one\n\npara two\n```ts\nconst a = 1\n```\n- item\nlazy line\n\n| x | y |\n| - | - |\nend",
        "---\ntitle: Example\n---\n\nPréface\n\n- plain\n  - [ ] nested\n    - [x] deeper\n- [ ] sibling\n\n<div>hidden</div>\n\nAfter\n\n- [x] final task\n\npartial",
    ];
    for doc in docs {
        let before = parses();
        for end in (1..=doc.len()).filter(|&i| doc.is_char_boundary(i)) {
            let src = &doc[..end];
            let (blocks, sources, tasks, starts, maps) = doc_cached(src);
            let (want, want_sources, want_tasks, want_cuts, want_starts, want_maps) = parse_doc(src, true);
            assert_eq!(*blocks, want, "blocks of {src:?}");
            assert_eq!(&*sources, &want_sources[..], "sources of {src:?}");
            assert_eq!(&*tasks, &want_tasks[..], "tasks of {src:?}");
            assert_eq!(&*starts, &want_starts[..], "block starts of {src:?}");
            assert_eq!(&*maps, &want_maps[..], "maps of {src:?}");
            let cuts = CACHE.with_borrow(|c| c.docs.peek(src).unwrap().cuts.clone());
            assert_eq!(&*cuts, &want_cuts[..], "cuts of {src:?}");
        }
        let (whole, tail) = (parses().0 - before.0, parses().1 - before.1);
        assert!(tail > whole * 2, "{doc:?}: {whole} whole parses, {tail} from the last block");
    }
}

/// A long stream keeps only its newest string, so it never pushes other documents out.
#[test]
fn a_stream_drops_the_strings_it_grew_from() {
    let mut rt = rune::Runtime::new((), |_: &(), _: &mut rune::Cx<()>| rune::el(), (10.0, 10.0), rune::Env::default());
    let mut commit = || {
        rt.invalidate();
        rt.frame(0.01);
    };
    commit();
    let docs: Vec<String> = (0..40).map(|i| format!("doc {i}\n\n- item {i}\n- another item")).collect();
    for doc in &docs {
        doc_cached(doc);
    }
    let held = CACHE.with_borrow(|c| c.docs.bytes());
    let mut stream = String::new();
    for i in 0.. {
        stream += &format!("Line {i} of a streamed answer, with a few more words.\n\n");
        if stream.len() >= 50_000 { break; }
    }
    for end in (50..=stream.len()).step_by(50).chain([stream.len()]) {
        commit();
        doc_cached(&stream[..end]);
    }
    CACHE.with_borrow(|c| {
        let last = c.docs.peek(stream.as_str()).expect("the whole stream is cached");
        let bytes = 2 * stream.len() + 64 * last.cuts.len() + 128;
        assert_eq!(c.docs.bytes(), held + bytes, "only the newest string of the stream is held");
        for doc in &docs {
            assert!(c.docs.peek(doc.as_str()).is_some(), "{doc:?} was evicted");
        }
    });
}

#[test]
fn a_reference_definition_parses_whole() {
    let before = parses();
    doc_cached("[x]\n\nsome text");
    doc_cached("[x]\n\nsome text\n\n[x]: https://example.com");
    assert_eq!(parses().0 - before.0, 2);
    assert_eq!(parse_cached("[x]\n\nsome text\n\n[x]: https://example.com")[0], parse("[x]\n\nsome text\n\n[x]: https://example.com")[0]);
}

#[test]
fn link_spans_come_before_inner_inline_styles_so_bold_em_and_code_keep_theirs() {
    let style = MarkdownStyle::neutral(true);
    let bg = Color { r: 1.0, g: 0.0, b: 0.5, a: 1.0 };
    let look: LinkLook = Rc::new(move |url| {
        url.starts_with("session:").then(|| (Span::new(0, 0).color(Color::WHITE).weight(500.0).bg(bg, 3.0), LinkPaint::new()))
    });
    let (blocks, sources, tasks, _, _) = doc_cached("[**bold** *em* `code` plain](session:1)");
    let Block::Para(inl) = &blocks[0] else { panic!() };
    let r = Renderer { s: style.clone(), highlights: Vec::new(), reveal: 0.0, streaming: false, on_link: None, on_link_secondary: None, link_look: Some(look), link_help: None, link_ring: None, code_link: None, on_task: None,
        tasks_read_only: false, selectable: false, scroll_wide: false, tentative: None, highlighter: Highlighter::builtin(), sources,
        next_source: Cell::new(0), tasks, next_task: Cell::new(0), current_block_key: RefCell::new("b0".into()),
        #[cfg(feature = "mermaid")] diagram_frame: None, code_frame: None, quote_frame: None, link_favicon: None, images: None, on_image: None, flow: Cell::default() };
    let spans = r.spans(inl, style.body);
    // The link span is first so `flatten_spans` applies inner Strong/Em/Code over it.
    assert_eq!((spans[0].start, spans[0].end, spans[0].color, spans[0].weight, spans[0].bg, spans[0].bg_pad), (0, inl.text.len() as u32, Some(Color::WHITE), Some(500.0), Some(bg), 3.0));
    assert!(spans.iter().any(|s| &inl.text[s.start as usize..s.end as usize] == "bold" && s.weight == Some(style.strong_weight)));
    assert!(spans.iter().any(|s| &inl.text[s.start as usize..s.end as usize] == "em" && s.italic && s.color.is_none()));
    assert!(spans.iter().any(|s| &inl.text[s.start as usize..s.end as usize] == "code" && s.mono && s.size == Some(style.body.size * style.inline_code_scale) && s.bg.is_none()));
}

#[test]
fn pure_markdown_caches_its_built_subtree_keeps_paths_and_invalidates_on_change() {
    use rune::{Cx, Env, Runtime};

    struct State {
        tick: u32,
        a: String,
        b: String,
        query: &'static str,
        accent: Color,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let _ = s.tick;
        let style = MarkdownStyle::neutral(true).text(s.accent);
        let hl = if s.query.is_empty() {
            Vec::new()
        } else {
            find(&s.a, s.query, Case::Insensitive).into_iter().collect()
        };
        col().w_full().h_full().children([
            // Two unkeyed Markdown siblings (even with identical text initially) keep `#0` and `#1`
            // without duplicate-key warnings, plus a `.key("third")` sibling.
            Markdown::new(&s.a).highlights(hl).selectable(true).render(&style),
            Markdown::new(&s.b).selectable(true).render(&style),
            Markdown::new("Third **card** note").render(&style).key("third"),
        ])
    }

    let mut rt = Runtime::new(
        State {
            tick: 0,
            a: "Hello **world** from `rune`.".into(),
            b: "Hello **world** from `rune`.".into(),
            query: "",
            accent: Color::WHITE,
        },
        view,
        (480.0, 400.0),
        Env::new(true, 1.0),
    );
    RENDERS.with(|n| n.set(0));
    rt.frame(0.0);
    assert_eq!(RENDERS.with(Cell::get), 3);
    let paths: Vec<_> = rt.elements().into_iter().map(|e| e.path).collect();
    assert!(paths.iter().any(|p| p == "#0/#0/b0"), "{paths:?}");
    assert!(paths.iter().any(|p| p == "#0/#1/b0"), "{paths:?}");
    assert!(paths.iter().any(|p| p == "#0/third/b0"), "{paths:?}");

    // Unrelated state commits skip building all three Markdown subtrees.
    RENDERS.with(|n| n.set(0));
    for _ in 0..5 {
        rt.update(|s, _| s.tick += 1);
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(RENDERS.with(Cell::get), 0);

    // Changing only `b` re-renders only `b`; `a` and `third` stay cached.
    RENDERS.with(|n| n.set(0));
    rt.update(|s, _| s.b = "Updated *second* paragraph.".into());
    rt.frame(1.0 / 120.0);
    assert_eq!(RENDERS.with(Cell::get), 1);
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Updated second paragraph.")));

    // Changing highlights on `a` re-renders only `a`.
    RENDERS.with(|n| n.set(0));
    rt.update(|s, _| s.query = "world");
    rt.frame(1.0 / 120.0);
    assert_eq!(RENDERS.with(Cell::get), 1);

    // Changing style invalidates all three.
    RENDERS.with(|n| n.set(0));
    rt.update(|s, _| s.accent = Color { r: 0.2, g: 0.6, b: 1.0, a: 1.0 });
    rt.frame(1.0 / 120.0);
    assert_eq!(RENDERS.with(Cell::get), 3);
}

#[test]
fn link_help_gives_each_link_its_own_tooltip() {
    use rune::{Cx, Env, Runtime};

    fn view(_: &(), _: &mut Cx<()>) -> El {
        Markdown::new("See [the docs](https://example.com/docs) or [here](notes)")
            .on_link(|_: &mut (), _| {})
            .link_help(|url| url.starts_with("https:").then(|| url.trim_start_matches("https://").to_owned()))
            .render(&MarkdownStyle::neutral(true))
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let links: Vec<(Option<String>, Option<String>)> = rt.elements().into_iter().filter(|e| e.role == "link").map(|e| (e.key, e.help)).collect();
    assert_eq!(links, [
        (Some("https://example.com/docs".into()), Some("example.com/docs".into())),
        (Some("notes".into()), None),
    ]);
}

/// A wheel over a wide table's scroll bar scrolls the pane around that table, though every wide
/// table keys its viewport `scroll` (`table::Renderer::wide_frame`): the wheel names the viewport
/// in the pane that holds the bar, not the first element keyed alike in the window, and a
/// sideways table's own bar hands a vertical wheel to the pane as it did before the bar existed.
#[test]
fn a_wheel_over_a_table_s_scroll_bar_scrolls_its_own_pane() {
    use std::cell::Cell;
    use rune::{Cx, Env, Runtime, ScrollPos, ScrollSpec};
    thread_local! {
        static LEFT: Cell<f32> = const { Cell::new(0.0) };
        static RIGHT: Cell<f32> = const { Cell::new(0.0) };
    }
    const DOC: &str = "| one | two | three |
| --- | --- | --- |
| alpha beta gamma delta | epsilon zeta eta theta | iota kappa lambda mu |
| aaaa bbbb cccc dddd | eeee ffff gggg hhhh | iiii jjjj kkkk llll |

Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor.

Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi.

Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore.

Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia.

Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium.

Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit.

Neque porro quisquam est, qui dolorem ipsum quia dolor sit amet, consectetur.

Ut enim ad minima veniam, quis nostrum exercitationem ullam corporis suscipit.

Quis autem vel eum iure reprehenderit qui in ea voluptate velit esse quam.";

    fn panes(_: &(), _: &mut Cx<()>) -> El {
        let style = MarkdownStyle::neutral(true).text_width(120.0).table_scroll_bar(Color::BLACK, Color::BLACK);
        let pane = |key: &str, at: fn(ScrollPos)| col().key(key.to_owned()).w(200.0).h(400.0)
            .child(col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
                .on_scroll_offset(at)
                .child(Markdown::new(DOC).scroll_wide(true).render(&style)));
        row().key("root").w_full().h_full()
            .child(pane("left", |p| LEFT.with(|c| c.set(p.offset[1]))))
            .child(pane("right", |p| RIGHT.with(|c| c.set(p.offset[1]))))
    }
    let mut rt = Runtime::new((), panes, (400.0, 400.0), Env::new(true, 1.0));
    let (left, right) = (|| LEFT.with(Cell::get), || RIGHT.with(Cell::get));
    rt.frame(0.0);
    // The right pane's table bar: the strip under its rows, in the pane that holds it.
    let bars = |rt: &Runtime<()>| -> Vec<([f32; 4], f32)> {
        rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("scroll-bar")).map(|e| (e.rect, e.opacity)).collect()
    };
    let bar = bars(&rt).iter().filter(|(r, _)| r[0] > 200.0).max_by(|a, b| a.0[0].total_cmp(&b.0[0]))
        .map(|(r, _)| *r).unwrap_or_else(|| panic!("both tables scroll sideways: {:?}", bars(&rt)));
    // An overlay bar shows while the pointer is over its table, which is when a wheel there
    // reaches its track at all.
    rt.pointer_move(bar[0] + bar[2] / 2.0, bar[1] + bar[3] / 2.0);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(bars(&rt).iter().all(|(_, o)| *o > 0.9), "the bars are up under the pointer: {:?}", bars(&rt));
    rt.wheel(bar[0] + bar[2] / 2.0, bar[1] + bar[3] / 2.0, -30.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(right(), 30.0, "the wheel over the right pane's table bar scrolled its own pane");
    assert_eq!(left(), 0.0, "the left pane, keyed alike, stayed where it was");
}

#[test]
fn lists_and_quotes_in_fit_content_containers_keep_their_natural_width_and_wrap_when_bounded() {
    use rune::{Cx, Env, Runtime};

    struct FitCase {
        src: &'static str,
        max_w: f32,
        streaming: bool,
    }
    fn fit_view(c: &FitCase, _: &mut Cx<FitCase>) -> El {
        let style = MarkdownStyle::neutral(true);
        col().w_full().items_end().child(
            col().key("bubble").max_w(c.max_w).min_w(0.0).px(12.0).child(
                Markdown::new(c.src).streaming(c.streaming).render(&style).key("md").min_w(0.0),
            ),
        )
    }

    let line = MarkdownStyle::neutral(true).body.line_px();

    // 1. Ordered list in a fit-content container aligned to the end: both items fit on one line
    // each instead of collapsing to one word wide (both memoised and streaming/unmemoised paths).
    let ordered = "1. Yes, they were all through me. Choose option a\n2. Option a";
    for streaming in [false, true] {
        let mut rt = Runtime::new(FitCase { src: ordered, max_w: 680.0, streaming }, fit_view, (800.0, 600.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let els = rt.elements();
        let bubble = els.iter().find(|e| e.key.as_deref() == Some("bubble")).unwrap();
        let first = els.iter().find(|e| e.text.as_deref() == Some("Yes, they were all through me. Choose option a")).unwrap();
        let second = els.iter().find(|e| e.text.as_deref() == Some("Option a")).unwrap();
        assert!(
            first.rect[3] <= line * 1.5,
            "streaming={streaming}: first item should fit on one line (line={line}), got rect={:?}, bubble={:?}",
            first.rect, bubble.rect
        );
        assert!(
            second.rect[3] <= line * 1.5,
            "streaming={streaming}: second item should fit on one line (line={line}), got rect={:?}, bubble={:?}",
            second.rect, bubble.rect
        );
        assert!(
            bubble.rect[2] > 200.0 && bubble.rect[2] < 680.0,
            "streaming={streaming}: bubble should size to content width, got {:?}",
            bubble.rect
        );
        assert!(
            first.rect[0] + first.rect[2] <= bubble.rect[0] + bubble.rect[2] + 0.5,
            "streaming={streaming}: item text must not spill past bubble: first={:?}, bubble={:?}",
            first.rect, bubble.rect
        );
    }

    // 2. Unordered, task, nested lists, and blockquotes also size to their natural width.
    let mixed = "- Unordered item with several words here\n  - Nested child item with several words\n- [ ] Task item with several words here\n\n> Blockquote text with several words here";
    {
        let mut rt = Runtime::new(FitCase { src: mixed, max_w: 680.0, streaming: false }, fit_view, (800.0, 600.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let els = rt.elements();
        let bubble = els.iter().find(|e| e.key.as_deref() == Some("bubble")).unwrap();
        for needle in [
            "Unordered item with several words here",
            "Nested child item with several words",
            "Task item with several words here",
            "Blockquote text with several words here",
        ] {
            let item = els.iter().find(|e| e.text.as_deref() == Some(needle)).unwrap();
            assert!(
                item.rect[3] <= line * 1.5,
                "{needle:?} should fit on one line (line={line}), got rect={:?}, bubble={:?}",
                item.rect, bubble.rect
            );
            assert!(
                item.rect[0] + item.rect[2] <= bubble.rect[0] + bubble.rect[2] + 0.5,
                "{needle:?} must not spill past bubble: item={:?}, bubble={:?}",
                item.rect, bubble.rect
            );
        }
        assert!(bubble.rect[2] < 680.0, "bubble should fit content, got {:?}", bubble.rect);
    }

    // 3. Narrow fit-content container wraps long list items and stays within max_w.
    {
        let mut rt = Runtime::new(FitCase { src: ordered, max_w: 180.0, streaming: false }, fit_view, (800.0, 600.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let els = rt.elements();
        let bubble = els.iter().find(|e| e.key.as_deref() == Some("bubble")).unwrap();
        let first = els.iter().find(|e| e.text.as_deref() == Some("Yes, they were all through me. Choose option a")).unwrap();
        let second = els.iter().find(|e| e.text.as_deref() == Some("Option a")).unwrap();
        assert!(
            bubble.rect[2] <= 180.5,
            "bubble must respect max_w(180.0), got {:?}",
            bubble.rect
        );
        assert!(
            first.rect[3] >= line * 1.9,
            "first item should wrap in narrow container (line={line}), got {:?}",
            first.rect
        );
        assert!(
            second.rect[3] <= line * 1.5,
            "short second item should still fit on one line, got {:?}",
            second.rect
        );
        assert!(
            first.rect[0] + first.rect[2] <= bubble.rect[0] + bubble.rect[2] + 0.5,
            "wrapped text must stay inside bubble: first={:?}, bubble={:?}",
            first.rect, bubble.rect
        );
    }
}

#[test]
fn lists_and_quotes_in_fixed_and_stretched_containers_fill_their_parent_width() {
    use rune::{Cx, Env, Runtime};

    struct HostCase {
        doc: &'static str,
        fixed_w: Option<f32>,
    }
    fn host_view(c: &HostCase, _: &mut Cx<HostCase>) -> El {
        let style = MarkdownStyle::neutral(true);
        let box_el = match c.fixed_w {
            Some(w) => col().key("host").w(w),
            None => col().key("host").w_full(),
        };
        col().w_full().h_full().child(box_el.child(Markdown::new(c.doc).render(&style).key("md")))
    }

    let style = MarkdownStyle::neutral(true);
    let indent = style.list_indent;
    let quote_inset = style.quote.width + style.quote.pad;
    let doc = "1. Short item\n2. A much longer list item that wraps cleanly when the container is narrow enough to force wrapping\n\n- [x] Checked task\n\n> Quoted line";

    // Fixed-width container (320 pt) and stretched container (500 pt window).
    for (label, fixed_w, win_w) in [("fixed", Some(320.0), 600.0), ("stretched", None, 500.0)] {
        let expected_w = fixed_w.unwrap_or(win_w);
        let mut rt = Runtime::new(HostCase { doc, fixed_w }, host_view, (win_w, 600.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let els = rt.elements();

        let short_item = els.iter().find(|e| e.text.as_deref() == Some("Short item")).unwrap();
        let long_item = els.iter().find(|e| e.text.as_deref() == Some("A much longer list item that wraps cleanly when the container is narrow enough to force wrapping")).unwrap();
        let task_item = els.iter().find(|e| e.text.as_deref() == Some("Checked task")).unwrap();
        let quote_item = els.iter().find(|e| e.text.as_deref() == Some("Quoted line")).unwrap();

        let expected_item_w = expected_w - indent;
        assert!(
            (short_item.rect[2] - expected_item_w).abs() < 0.5,
            "{label}: short list item body should span remaining width {expected_item_w}, got {:?}",
            short_item.rect
        );
        assert!(
            (short_item.rect[0] - indent).abs() < 0.5,
            "{label}: short list item text should start after indent {indent}, got {:?}",
            short_item.rect
        );
        assert!(
            (long_item.rect[2] - expected_item_w).abs() < 0.5,
            "{label}: long list item body should span remaining width {expected_item_w}, got {:?}",
            long_item.rect
        );
        assert!(
            (long_item.rect[0] - indent).abs() < 0.5,
            "{label}: long list item text should start after indent {indent}, got {:?}",
            long_item.rect
        );
        let task_inset = indent.max(style.task.left + style.task.size);
        assert!(
            (task_item.rect[2] - (expected_w - task_inset)).abs() < 0.5,
            "{label}: task item body should span remaining width {}, got {:?}",
            expected_w - task_inset, task_item.rect
        );
        assert!(
            (quote_item.rect[2] - (expected_w - quote_inset)).abs() < 0.5,
            "{label}: quote body should span remaining width {}, got {:?}",
            expected_w - quote_inset, quote_item.rect
        );
        assert!(
            (quote_item.rect[0] - quote_inset).abs() < 0.5,
            "{label}: quote text should start after rule+pad {quote_inset}, got {:?}",
            quote_item.rect
        );
    }
}

#[test]
fn wide_gfm_tables_cap_columns_and_hostile_nesting_stays_bounded() {
    use std::fmt::Write;
    let cols = 500usize;
    let (mut head, mut delim, mut row) = (String::from("|"), String::from("|"), String::from("|"));
    for i in 0..cols {
        let _ = write!(head, " h{i} |");
        delim.push_str("---|");
        let _ = write!(row, " v{i} |");
    }
    let table_src = format!("{head}\n{delim}\n{row}\n");
    let blocks = parse(&table_src);
    let [Block::Table(t)] = blocks.as_slice() else {
        panic!("expected a single table block, got {blocks:?}");
    };
    assert_eq!(t.align.len(), 128, "table alignments capped at 128 columns");
    assert_eq!(t.head.len(), 128, "table header capped at 128 columns");
    assert_eq!(t.rows.len(), 1);
    assert_eq!(t.rows[0].len(), 128, "table row capped at 128 columns");
    assert_eq!(t.head[0].text, "h0");
    assert_eq!(t.head[127].text, "h127");
    assert_eq!(t.rows[0][0].text, "v0");
    assert_eq!(t.rows[0][127].text, "v127");

    // Streaming pending table header (before delimiter row arrives) obeys the same column cap.
    let pending_blocks = parse(&head);
    let [Block::Table(pt)] = pending_blocks.as_slice() else {
        panic!("expected a pending table block, got {pending_blocks:?}");
    };
    assert_eq!(pt.align.len(), 128, "pending table alignments capped at 128 columns");
    assert_eq!(pt.head.len(), 128, "pending table header capped at 128 columns");
    assert_eq!(pt.head[127].text, "h127");

    // Deeply nested quotes, lists, and nine-digit numbered lists parse and render without overflowing.
    let deep_quotes = format!("{}leaf", "> ".repeat(2_000));
    let deep_lists: String = (0..2_000).map(|i| format!("{}- item {i}\n", "  ".repeat(i.min(600)))).collect();
    let nine_digit: String = (0..30).map(|i| format!("{}123456789. item {i}\n", " ".repeat(i * 11))).collect();
    for src in [table_src, deep_quotes, deep_lists, nine_digit] {
        let mut rt = rune::Runtime::new(
            src,
            |s: &String, _: &mut rune::Cx<String>| {
                let style = MarkdownStyle::neutral(true);
                Markdown::new(s).render(&style)
            },
            (400.0, 400.0),
            rune::Env::new(true, 1.0),
        );
        rt.frame(0.0);
    }
}
