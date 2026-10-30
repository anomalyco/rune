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
                Block::Code(c) => out.push(c),
                Block::Quote(inner) => codes(inner, out),
                Block::List { items, .. } => items.iter().for_each(|i| codes(i, out)),
                _ => {}
            }
        }
    }
    let (blocks, sources, tasks, _, _) = doc_cached(src);
    let r = Renderer { s: style.clone(), highlights: Vec::new(), reveal: 0.0, streaming: false, on_link: None, on_link_secondary: None, link_look: None, link_ring: None, on_task: None,
        tasks_read_only: false, selectable: false, scroll_wide: false, highlighter, sources: sources.clone(),
        next_source: Cell::new(0), tasks, next_task: Cell::new(0), current_block_key: RefCell::new("b0".into()) };
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
    assert_eq!(coloured(ts, s, style.syntax.color(Syntax::Keyword)), ["const"]);
    assert_eq!(coloured(ts, s, style.syntax.color(Syntax::Number)), ["1"]);
    assert!(s.iter().all(|s| s.color.is_some() && s.weight.is_none() && !s.italic && !s.mono && s.size.is_none() && s.bg.is_none()));
    for (code, s) in &got[1..4] {
        assert!(s.is_empty(), "{code:?} stays plain");
    }
    assert_eq!(coloured(&got[4].0, &got[4].1, style.syntax.color(Syntax::Function)), ["f"]);
    assert_eq!(coloured(&got[5].0, &got[5].1, style.syntax.color(Syntax::Keyword)), ["select"]);
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
    assert_eq!(coloured(code, s, red), ["let"]);
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
    let src = "```ts\nconst a = 1;\n```\n\n```\nplain\n```\n\n```kotlin\nval x = 1\n```";
    let got = spans(src, &style);
    let k = |s| style.syntax.color(s);
    let ts: Vec<_> = got[0].1.iter().map(|s| (s.start, s.end, s.color.unwrap())).collect();
    assert_eq!(ts, [(0, 5, k(Syntax::Keyword)), (8, 9, k(Syntax::Punctuation)), (10, 11, k(Syntax::Number)), (11, 12, k(Syntax::Punctuation))]);
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
    const INTER: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/InterVariable.ttf");
    const PLEX: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
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
    let r = Renderer { s: style.clone(), highlights: Vec::new(), reveal: 0.0, streaming: false, on_link: None, on_link_secondary: None, link_look: Some(look), link_ring: None, on_task: None,
        tasks_read_only: false, selectable: false, scroll_wide: false, highlighter: Highlighter::builtin(), sources,
        next_source: Cell::new(0), tasks, next_task: Cell::new(0), current_block_key: RefCell::new("b0".into()) };
    let spans = r.spans(inl, style.body.size);
    // The link span is first so `flatten_spans` applies inner Strong/Em/Code over it.
    assert_eq!((spans[0].start, spans[0].end, spans[0].color, spans[0].weight, spans[0].bg, spans[0].bg_pad), (0, inl.text.len() as u32, Some(Color::WHITE), Some(500.0), Some(bg), 3.0));
    assert!(spans.iter().any(|s| &inl.text[s.start as usize..s.end as usize] == "bold" && s.weight == Some(style.strong_weight)));
    assert!(spans.iter().any(|s| &inl.text[s.start as usize..s.end as usize] == "em" && s.italic && s.color.is_none()));
    assert!(spans.iter().any(|s| &inl.text[s.start as usize..s.end as usize] == "code" && s.mono && s.size == Some(style.body.size * style.inline_code_scale) && s.bg.is_none()));
}
