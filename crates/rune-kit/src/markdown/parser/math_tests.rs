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
