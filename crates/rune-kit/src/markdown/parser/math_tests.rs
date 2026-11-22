use super::*;
use crate::markdown::parser::parse_doc;

fn take(i: &Inline, out: &mut Vec<String>) {
    for &(a, b, st) in &i.spans {
        if st == Style::Math {
            out.push(i.text[a..b].to_owned());
        }
    }
}

fn walk(bs: &[Block], out: &mut Vec<String>) {
    for b in bs {
        match b {
            Block::Para(i) | Block::Heading(_, i) => take(i, out),
            Block::List { items, .. } => items.iter().for_each(|it| walk(it, out)),
            Block::Quote(inner) => walk(inner, out),
            Block::Table(t) => t.head.iter().chain(t.rows.iter().flatten()).for_each(|i| take(i, out)),
            _ => {}
        }
    }
}

/// The inline math in `blocks`, as written.
fn maths_in(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out
}

fn maths(src: &str) -> Vec<String> {
    maths_in(&parse(src))
}

fn streamed(src: &str) -> Vec<String> {
    maths_in(&crate::markdown::stream::parse_streaming(src, true).0)
}

fn para(src: &str) -> Inline {
    match &parse(src)[..] {
        [Block::Para(i)] => i.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn dollars_around_tex_are_math_and_keep_their_source() {
    assert_eq!(maths("so $e^x$ grows"), ["$e^x$"]);
    assert_eq!(para("so $e^x$ grows").text, "so $e^x$ grows");
    assert_eq!(maths(r"where $e \approx 2.71828$ holds"), [r"$e \approx 2.71828$"]);
    assert_eq!(maths(r"paren \(a+b\) and display $$x^2$$ inline"), [r"\(a+b\)", "$$x^2$$"]);
    // The source shows as written, backslashes and all, until the math renders.
    assert_eq!(para(r"paren \(a_1\) here").text, r"paren \(a_1\) here");
    assert_eq!(maths(r"$a\$b$ ok"), [r"$a\$b$"]);
}

#[test]
fn the_synthetic_paragraph_finds_its_four_equations_and_no_prices() {
    let src = r"Using math.exp(x) computes $e^x$, where $e \approx 2.71828$ (Euler's number). For logits $z_i$, softmax is $\frac{e^{z_i}}{\sum_j e^{z_j}}$, and costs stay at $0.60 to $12.50 with $HOME unchanged.";
    assert_eq!(maths(src), ["$e^x$", r"$e \approx 2.71828$", "$z_i$", r"$\frac{e^{z_i}}{\sum_j e^{z_j}}$"]);
    assert_eq!(para(src).text, src);
}

#[test]
fn prices_shell_variables_and_lone_dollars_stay_text() {
    for src in ["$0.60", "$11,884.62", "$10 to $20", "costs $5 and $6.", "$HOME", "$1", "$5.00/$6.00", "$HOME/$USER", "a$x$ b", "$x$y", "$ x$", "$x $", "$$", "$$$x$$$", r"\$x$ and \$y\$"] {
        assert!(maths(src).is_empty(), "{src:?}: {:?}", maths(src));
    }
    assert_eq!(para("$5.00/$6.00").text, "$5.00/$6.00");
    assert_eq!(maths("$HOME/$USER and $x$"), ["$x$"]);
}

#[test]
fn code_spans_and_fences_hold_no_math() {
    assert!(maths("run `$x$` now").is_empty());
    assert_eq!(maths("`$a` then $b$"), ["$b$"]);
    // A span can't reach into code either.
    assert!(maths("$a `b$`").is_empty());
    let (blocks, ..) = parse_doc("```\n$x$\n```\n", true);
    assert_eq!(blocks, [Block::Code("$x$".into())]);
}

#[test]
fn math_still_arriving_stays_text() {
    assert!(maths("Using $e^x").is_empty());
    assert!(maths(r"so \(a+b").is_empty());
    assert!(maths("so $$x^2").is_empty());
    assert_eq!(para("Using $e^x").text, "Using $e^x");
    // A stream's tail closes an emphasis still arriving, never math.
    assert!(streamed("so *a $e^x").is_empty());
    assert_eq!(streamed("so $e^x$ and *b"), ["$e^x$"]);
}

#[test]
fn several_in_a_line_and_around_emphasis_and_links() {
    assert_eq!(maths(r"$a$ and $b$, then \(c\)"), ["$a$", "$b$", r"\(c\)"]);
    assert_eq!(maths("*$a$* and **b $c$**"), ["$a$", "$c$"]);
    assert_eq!(para("$a$ **b**").text, "$a$ b");
    assert_eq!(maths("[$x$](https://a.b)"), ["$x$"]);
    // Never across an emphasis's edge.
    assert!(maths("*x $a* b$").is_empty());
    // Emphasis markers inside math are TeX.
    assert_eq!(maths("$a*b*c$ d"), ["$a*b*c$"]);
    assert_eq!(para("$a*b*c$ d").text, "$a*b*c$ d");
    assert_eq!(maths("$a_1 + b_2$ x"), ["$a_1 + b_2$"]);
}

#[test]
fn headings_lists_quotes_and_tables_find_math() {
    assert_eq!(maths("# Euler $e^{i\\pi}$"), [r"$e^{i\pi}$"]);
    assert_eq!(maths("- one $a$\n- two $b$\n"), ["$a$", "$b$"]);
    assert_eq!(maths("> quoted $q$\n"), ["$q$"]);
    assert_eq!(maths("| a | b |\n|---|---|\n| $x$ | $y$ |\n"), ["$x$", "$y$"]);
}

fn display(src: &str) -> Vec<(String, Option<String>)> {
    let (blocks, sources, ..) = parse_doc(src, true);
    let mut out = Vec::new();
    let mut n = 0;
    fn go(bs: &[Block], srcs: &[Source], n: &mut usize, out: &mut Vec<(String, Option<String>)>) {
        for b in bs {
            match b {
                Block::Code(c) => {
                    out.push((c.clone(), srcs.get(*n).and_then(|s| s.lang.as_deref().map(str::to_owned))));
                    *n += 1;
                }
                Block::Mermaid(_) => *n += 1,
                Block::Quote(inner) => go(inner, srcs, n, out),
                Block::List { items, .. } => items.iter().for_each(|it| go(it, srcs, n, out)),
                _ => {}
            }
        }
    }
    go(&blocks, &sources, &mut n, &mut out);
    out
}

#[test]
fn a_paragraph_that_is_one_equation_is_display_math() {
    let math = |t: &str| vec![(t.to_owned(), Some("math".to_owned()))];
    assert_eq!(display("$$\nx^2 + y^2\n$$\n"), math("x^2 + y^2"));
    assert_eq!(display("before\n\n$$\na\nb\n$$\n\nafter"), math("a\nb"));
    assert_eq!(display("\\[\n\\int_0^1 x\\,dx\n\\]\n"), math(r"\int_0^1 x\,dx"));
    assert_eq!(display("$$x^2$$\n"), math("x^2"));
    assert_eq!(display("> $$\n> a\n> $$\n"), math("a"));
    // No inline math left behind.
    assert!(maths("$$\nx^2\n$$\n").is_empty());
    // Not closed, or with words around it: text.
    assert!(display("$$\nx^2\n").is_empty());
    assert!(display("so $$x^2$$ here").is_empty());
    assert!(display("$$\nx\n$$ more").is_empty());
}

#[test]
fn display_math_maps_back_to_its_source() {
    let src = "> $$\n> a\n> $$\n";
    let (blocks, _, _, _, _, maps) = parse_doc(src, true);
    let Block::Quote(inner) = &blocks[0] else { panic!("{blocks:?}") };
    assert_eq!(inner[..], [Block::Code("a".into())]);
    let BlockMap::Quote(m) = &maps[0] else { panic!() };
    let BlockMap::Code(m) = &m[0] else { panic!() };
    assert_eq!(m.0, [(7..8, 0..1)]);
}

#[test]
fn inline_math_maps_to_its_source() {
    let src = "x *y* $a^2$ z";
    let (_, _, _, _, _, maps) = parse_doc(src, true);
    let BlockMap::Inline(m) = &maps[0] else { panic!() };
    let p = para(src);
    let at = p.text.find('$').unwrap();
    assert!(m.0.iter().any(|(s, o)| s.contains(&src.find('$').unwrap()) && o.contains(&at)), "{m:?}");
}

/// `scan` and `paren_at` as they were before the one-pass change (#1350), kept to check that
/// the change finds exactly the same math.
fn scan_before(src: &str, run: Range<usize>, opaque: &[Range<usize>], out: &mut Vec<Range<usize>>) {
    fn paren_at_before(s: &[u8], i: usize, end: usize) -> Option<usize> {
        let mut j = i + 2;
        while j < end {
            match s[j] {
                b'\n' => return None,
                b'\\' if s.get(j + 1) == Some(&b')') => return s[i + 2..j].iter().any(|c| !c.is_ascii_whitespace()).then_some(j + 2),
                b'\\' => j += 2,
                _ => j += 1,
            }
        }
        None
    }
    let s = src.as_bytes();
    let end = run.end.min(s.len());
    let mut i = run.start;
    while i < end {
        if let Some(o) = opaque.iter().find(|o| o.start <= i && i < o.end) {
            i = o.end;
            continue;
        }
        if i == run.start && escaped(s, i) {
            i += 1;
            continue;
        }
        let found = match s[i] {
            b'\\' if s.get(i + 1) == Some(&b'(') => paren_at_before(s, i, end),
            b'\\' => {
                i += if s.get(i + 1).is_some_and(u8::is_ascii_punctuation) { 2 } else { 1 };
                continue;
            }
            b'$' if s.get(i + 1) == Some(&b'$') => display_at(s, i, end),
            b'$' => inline_at(s, i, end),
            _ => None,
        };
        match found {
            Some(e) if !opaque.iter().any(|o| o.start < e && i < o.end) => {
                out.push(i..e);
                i = e;
            }
            _ if s[i] == b'$' => i += s[i..end].iter().take_while(|&&c| c == b'$').count(),
            _ => i += 1,
        }
    }
}

#[test]
fn the_one_pass_scan_finds_what_the_old_scan_found() {
    let alphabet = b"\\\\\\(()$$ a1\n`x";
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for _ in 0..3000 {
        let len = next(160) + 1;
        let src: String = (0..len).map(|_| alphabet[next(alphabet.len())] as char).collect();
        // Code spans, raw HTML and autolinks: in source order, never overlapping.
        let mut opaque = Vec::new();
        let mut at = 0;
        loop {
            let start = at + next(14);
            let stop = start + next(7) + 1;
            if stop > len {
                break;
            }
            opaque.push(start..stop);
            at = stop;
        }
        let run = next(len)..len;
        let (mut now, mut before) = (Vec::new(), Vec::new());
        scan(&src, run.clone(), &opaque, &mut now);
        scan_before(&src, run, &opaque, &mut before);
        assert_eq!(now, before, "{src:?} {opaque:?}");
    }
}

#[test]
fn many_code_spans_and_long_lines_keep_their_math() {
    // Prices stay text among code spans; a `$…$` inside a code span is code; real math is math.
    assert_eq!(maths("It costs $5 today, `x` and $7 tomorrow, `$y$` is code, $12 later; $x^2$ is math."), ["$x^2$"]);
    // A thousand code spans with one `$PATH`, and math between them.
    let mut many = String::from("Add it to your $PATH first.\n\n");
    for k in 0..1000 {
        many.push_str(&format!("- call `f_{k}($a$)` then \\(a_{k}\\) done.\n"));
    }
    let expected: Vec<String> = (0..1000).map(|k| format!("\\(a_{k}\\)")).collect();
    assert_eq!(maths(&many), expected);
    // A line of `\(` that never closes, a line where only a later one closes, then a closed one.
    // (A paragraph's text starts after an escape's backslash, so its first `\(` opens nothing.)
    let src = format!("{}\n\n\\( \\( b \\) and \\( \\)\n\nthen \\(x\\) here", "\\( ".repeat(1000));
    assert_eq!(maths(&src), ["\\( b \\)", "\\(x\\)"]);
}

#[test]
fn a_mebibyte_of_code_spans_or_open_parens_scans_in_one_pass() {
    // Before #1350's follow-up these took seconds (release) to minutes (debug).
    let mut spans = String::from("Set $HOME first.\n\n");
    let mut k = 0;
    while spans.len() < 1 << 20 {
        spans.push_str(&format!("- item {k}: call `fn_{k}(arg)` and keep going with prose here.\n"));
        k += 1;
    }
    let parens = "\\( ".repeat((1 << 20) / 3);
    let t = std::time::Instant::now();
    assert!(maths(&spans).is_empty());
    assert!(maths(&parens).is_empty());
    assert!(t.elapsed() < std::time::Duration::from_secs(30), "{:?}", t.elapsed());
}
