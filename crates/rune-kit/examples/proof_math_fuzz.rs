//! Proof and fuzz program for anomalyco/rune#1294 (proof branch only, never merged): hostile TeX,
//! adversarial RM01 guest output, and the async Markdown path with Find.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rune::{Color, Cx, El, Env, Runtime, col};
use rune_kit::markdown::{Case, Markdown, MarkdownStyle, find};
use rune_kit::math;

struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn usize(&mut self, n: usize) -> usize {
        (self.next_u64() as usize) % n
    }
    fn u8(&mut self) -> u8 {
        self.next_u64() as u8
    }
}

fn build_adversarial_wire_cases() -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();
    // 1. Empty / short / bad magic
    out.push(("empty", vec![]));
    out.push(("short", vec![0u8; 19]));
    out.push(("bad-magic", b"RM99\0\0\0\0\0\0\0\0\0\0\0\0\x01\0\0\0".to_vec()));
    // 2. Massive item count (u32::MAX)
    let mut b = b"RM01".to_vec();
    b.extend_from_slice(&10.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&0.5f32.to_le_bytes());
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    out.push(("u32-max-items", b));
    // 3. NaN / Inf dimensions
    for &(name, val) in &[("nan-w", f32::NAN), ("inf-w", f32::INFINITY), ("neg-w", -5.0f32)] {
        let mut b = b"RM01".to_vec();
        b.extend_from_slice(&val.to_le_bytes());
        b.extend_from_slice(&1.0f32.to_le_bytes());
        b.extend_from_slice(&0.5f32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(1); // Line
        b.extend_from_slice(&0.0f32.to_le_bytes());
        b.extend_from_slice(&0.0f32.to_le_bytes());
        b.extend_from_slice(&1.0f32.to_le_bytes());
        b.extend_from_slice(&0.04f32.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 255]);
        b.push(0);
        out.push((name, b));
    }
    // 4. Forged 1e8 dashed line directly on wire (bypassing guest!)
    let mut b = b"RM01".to_vec();
    b.extend_from_slice(&10.0f32.to_le_bytes());
    b.extend_from_slice(&2.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.push(1); // Line
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&15_000.0f32.to_le_bytes());
    b.extend_from_slice(&0.04f32.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 255]);
    b.push(1); // dashed = true
    out.push(("wire-huge-dashed-line", b));
    // 5. Forged >4096 px canvas width (300 em * 16 pt * 2x > 4096 px)
    let mut b = b"RM01".to_vec();
    b.extend_from_slice(&300.0f32.to_le_bytes());
    b.extend_from_slice(&2.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.push(2); // Rect
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 255]);
    out.push(("wire-over-4096-px", b));
    // 6. Forged >1M px (2000x1000 @ 2x -> 4000x2000 = 8M px > 1M cap)
    let mut b = b"RM01".to_vec();
    b.extend_from_slice(&100.0f32.to_le_bytes());
    b.extend_from_slice(&40.0f32.to_le_bytes());
    b.extend_from_slice(&40.0f32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.push(2);
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 255]);
    out.push(("wire-over-1M-px", b));
    // 7. Forged huge Path cmd_count
    let mut b = b"RM01".to_vec();
    b.extend_from_slice(&5.0f32.to_le_bytes());
    b.extend_from_slice(&2.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.push(3); // Path
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.push(1);
    b.extend_from_slice(&[0, 0, 0, 255]);
    b.extend_from_slice(&100_000u32.to_le_bytes());
    out.push(("wire-huge-path-cmds", b));
    // 8. Forged invalid surrogate char_code
    let mut b = b"RM01".to_vec();
    b.extend_from_slice(&5.0f32.to_le_bytes());
    b.extend_from_slice(&2.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.push(0); // Glyph
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&0.0f32.to_le_bytes());
    b.extend_from_slice(&1.0f32.to_le_bytes());
    b.extend_from_slice(&0xD800u32.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 255]);
    b.push(12);
    b.extend_from_slice(b"Main-Regular");
    out.push(("wire-surrogate-char", b));
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let wire_iters: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let tex_iters: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4_000);

    let ink = Color { r: 0.9, g: 0.9, b: 0.92, a: 1.0 };

    println!("=== Stage 1: Valid corpus + ALL READER-4 hostile TeX inputs ===");
    let valid_corpus: &[(&str, &str)] = &[
        ("simple", r"x^2 + y^2 = z^2"),
        ("quadratic", r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}"),
        ("integral", r"\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}"),
        ("sum_matrix", r"\sum_{k=1}^{n} k^2 = \frac{n(n+1)(2n+1)}{6}, \quad A = \begin{pmatrix} 1 & 0 \\ 0 & 1 \end{pmatrix}"),
        ("bb_hat", r"\forall \mathbf{v} \in \mathbb{R}^n, \quad \widehat{f}(\xi) = \int f(x) e^{-2\pi i x \cdot \xi}\,dx"),
        ("cases", r"f(x) = \begin{cases} x^2 & x \ge 0 \\ -x & x < 0 \end{cases}"),
        ("aligned", r"\begin{aligned} a &= b + c \\ d &= e + f \end{aligned}"),
        ("color_box", r"\color{red}{\boxed{E = mc^2}}"),
    ];
    let mut real_wire_seeds: Vec<Vec<u8>> = Vec::new();
    for (name, tex) in valid_corpus {
        let t0 = Instant::now();
        let wire = math::eval_wire_for_proof(tex).unwrap_or_else(|| panic!("valid formula failed guest: {name}"));
        let dims = math::decode_and_raster_for_proof(&wire, 14.0, ink, 2.0)
            .unwrap_or_else(|| panic!("valid formula failed host decode/raster: {name}"));
        let us = t0.elapsed().as_secs_f64() * 1e6;
        println!("  VALID   {name:14} -> {us:8.1} us | wire={} B | px={}x{}", wire.len(), dims.0, dims.1);
        real_wire_seeds.push(wire);
    }

    let hostile_tex: Vec<(&str, String)> = vec![
        ("alignat-9999999999", r"\begin{alignat}{9999999999}x\end{alignat}".into()),
        ("alignat-50M-oom", r"\begin{alignat}{50000000}x\end{alignat}".into()),
        ("alignat-2M-oom", r"\begin{alignat}{2000000}x\end{alignat}".into()),
        ("macro-alignat-oom", r"\def\n{999999999}\begin{alignat}{\n}x\end{alignat}".into()),
        ("stacked-delim-i64", r"\left(\rule{1em}{10000000000000000em}\right)".into()),
        ("widehat-inf", format!(r"\widehat{{\rule{{{}em}}{{1em}}}}", "9".repeat(320))),
        ("15k-ligatures", format!(r"\text{{{}}}", "-".repeat(15_000))),
        ("15k-kern", format!(r"\kern {}", "1".repeat(15_000))),
        ("macro-budget-bypass", format!(r"\def\f#1{{\f{{#1}}}}\f{{{}}}", "x ".repeat(7_000))),
        ("72B-mathrlap-hdashline", r"x\mathrlap{\begin{array}{c}\hdashline\rule{100000000em}{1em}\end{array}}".into()),
        ("2k-braces", format!("{}{}{}", "{".repeat(2000), "x", "}".repeat(2000))),
        ("500-frac", format!("{}1{}", r"\frac{1}{".repeat(500), "}".repeat(500))),
        ("over-16KiB", "x + ".repeat(5000)),
        ("non-latin-cjk", r"\text{你好世界}".into()),
        ("invalid-macro", r"\frac{1}{\invalidmacro{".into()),
        ("wide-over-4096px", (0..600).map(|i| format!("x_{{{i}}}")).collect::<Vec<_>>().join(" + ")),
    ];
    for (name, tex) in &hostile_tex {
        let t0 = Instant::now();
        let outcome = math::eval_wire_for_proof(tex)
            .and_then(|w| math::decode_and_raster_for_proof(&w, 14.0, ink, 2.0));
        let us = t0.elapsed().as_secs_f64() * 1e6;
        println!("  HOSTILE {name:24} -> {us:9.1} us | result={outcome:?}");
        // Verify instant clean recovery after every hostile input
        let rec = math::eval_wire_for_proof(r"\frac{1}{2}")
            .and_then(|w| math::decode_and_raster_for_proof(&w, 14.0, ink, 2.0));
        assert!(rec.is_some(), "failed recovery after {name}");
    }

    println!("\n=== Stage 2: Adversarial RM01 wire fuzzer ({wire_iters} mutated payloads) ===");
    let t_wire = Instant::now();
    for (name, bytes) in build_adversarial_wire_cases() {
        let r = math::decode_and_raster_for_proof(&bytes, 14.0, ink, 2.0);
        println!("  WIRE-CASE {name:24} -> {r:?}");
        if name != "wire-huge-dashed-line" {
            assert!(r.is_none(), "expected rejection for {name}");
        }
    }
    // Truncation prefixes of all real seeds
    let mut trunc_ok = 0usize;
    for seed in &real_wire_seeds {
        for cut in 0..seed.len() {
            if math::decode_and_raster_for_proof(&seed[..cut], 14.0, ink, 2.0).is_some() {
                trunc_ok += 1;
            }
        }
    }
    assert_eq!(trunc_ok, 0, "truncated wire accepted");

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut wire_accepted = 0usize;
    let mut wire_rejected = 0usize;
    let mut max_wire_us = 0.0f64;
    let nasty_floats: [f32; 8] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30, -0.0, 0.0, 1e-38];
    for i in 0..wire_iters {
        let mut buf = if i % 5 == 0 {
            let len = rng.usize(512);
            (0..len).map(|_| rng.u8()).collect::<Vec<u8>>()
        } else {
            real_wire_seeds[rng.usize(real_wire_seeds.len())].clone()
        };
        let mutations = 1 + rng.usize(6);
        for _ in 0..mutations {
            if buf.is_empty() {
                buf.push(rng.u8());
                continue;
            }
            match rng.usize(5) {
                0 => {
                    let idx = rng.usize(buf.len());
                    buf[idx] ^= 1u8 << (rng.usize(8));
                }
                1 => {
                    let idx = rng.usize(buf.len());
                    buf[idx] = rng.u8();
                }
                2 if buf.len() >= 4 => {
                    let idx = rng.usize(buf.len() - 3);
                    let f = nasty_floats[rng.usize(nasty_floats.len())];
                    buf[idx..idx + 4].copy_from_slice(&f.to_le_bytes());
                }
                3 if buf.len() > 8 => {
                    let new_len = rng.usize(buf.len());
                    buf.truncate(new_len);
                }
                _ => {
                    for _ in 0..rng.usize(16) {
                        buf.push(rng.u8());
                    }
                }
            }
        }
        let t0 = Instant::now();
        match math::decode_and_raster_for_proof(&buf, 14.0, ink, 2.0) {
            Some(_) => wire_accepted += 1,
            None => wire_rejected += 1,
        }
        let us = t0.elapsed().as_secs_f64() * 1e6;
        if us > max_wire_us {
            max_wire_us = us;
        }
    }
    println!(
        "  WIRE FUZZ DONE: {wire_iters} iters in {:.2}s | accepted={wire_accepted} rejected={wire_rejected} | max_iter={max_wire_us:.1} us",
        t_wire.elapsed().as_secs_f64()
    );

    println!("\n=== Stage 3: Full sandbox TeX fuzzer ({tex_iters} mutated TeX inputs) ===");
    let tokens: &[&str] = &[
        r"\frac{", r"\sqrt{", r"\sqrt[3]{", r"\begin{pmatrix}", r"\end{pmatrix}",
        r"\begin{array}{c}", r"\end{array}", r"\begin{alignat}{999999}", r"\end{alignat}",
        r"\left(", r"\right)", r"\rule{1em}{1000em}", r"\kern 99999", r"\hdashline",
        r"\mathrlap{", r"\widehat{", r"\text{---}", r"\def\a#1{\a{#1}}\a{x}", r"\color{red}{",
        r"\sum_{i=1}^{n}", r"\int_0^\infty", r"\mathbb{R}", r"\mathbf{v}", r"\verb|{{|",
        "{", "}", "^", "_", "&", r"\\", "x", "y", "0", "9", "-", "+", "=", "²", "α", "中",
    ];
    let t_tex = Instant::now();
    let mut tex_ok = 0usize;
    let mut tex_err = 0usize;
    let mut max_tex_ms = 0.0f64;
    for i in 0..tex_iters {
        let mut s = if i % 3 == 0 {
            valid_corpus[rng.usize(valid_corpus.len())].1.to_owned()
        } else {
            String::new()
        };
        let n_tok = 1 + rng.usize(40);
        for _ in 0..n_tok {
            let tok = tokens[rng.usize(tokens.len())];
            if s.len() + tok.len() <= 16_384 {
                s.push_str(tok);
            }
        }
        if i % 17 == 0 && s.len() < 4096 {
            let rep = 1 + rng.usize(16);
            s = s.repeat(rep);
        }
        let t0 = Instant::now();
        let res = math::eval_wire_for_proof(&s)
            .and_then(|w| math::decode_and_raster_for_proof(&w, 14.0, ink, 2.0));
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        if ms > max_tex_ms {
            max_tex_ms = ms;
        }
        match res {
            Some(_) => tex_ok += 1,
            None => tex_err += 1,
        }
    }
    println!(
        "  TEX FUZZ DONE: {tex_iters} iters in {:.2}s | ok={tex_ok} err/trapped={tex_err} | max_iter={max_tex_ms:.2} ms",
        t_tex.elapsed().as_secs_f64()
    );

    println!("\n=== Stage 4: End-to-end async Markdown + Find exclusion + 50-formula cache ===");
    let wakes = Arc::new(AtomicUsize::new(0));
    let w2 = wakes.clone();
    math::set_waker(Arc::new(move || {
        w2.fetch_add(1, Ordering::Relaxed);
    }));
    let mut doc = String::new();
    for i in 0..50 {
        doc.push_str(&format!("Formula {i}:\n\n```latex\nx_{{{i}}}^2 + y_{{{i}}}^2 = \\frac{{{i}}}{{n+1}}\n```\n\n"));
    }
    doc.push_str("Bad formula:\n\n```latex\n\\frac{1}{\\invalidmacro{\n```\n\n");

    let style = MarkdownStyle::neutral(true).text_width(680.0).math_scale(2.0);
    struct S { doc: String, style: MarkdownStyle }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().w(680.0).child(Markdown::new(s.doc.clone()).scroll_wide(true).render(&s.style))
    }
    let mut rt = Runtime::new(S { doc: doc.clone(), style }, view, (800.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // Wait up to 5 s for all 51 background jobs (50 valid + 1 invalid) to settle
    let start_wait = Instant::now();
    let target_gen = math::generation() + 51;
    while math::generation() < target_gen && start_wait.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    // Simulate a Rune host's `poll()` noticing `math::generation()` advanced and marking view dirty.
    rt.update(|_, _| {});
    rt.frame(1.0 / 60.0);
    let img_count = rt.elements().into_iter().filter(|e| e.role == "img").count();
    let find_rendered = find(&doc, "\\frac{0}{n+1}", Case::Insensitive);
    let find_fallback = find(&doc, "invalidmacro", Case::Insensitive);
    println!(
        "  ASYNC E2E: wakes={} gen={} img_els={img_count}/50 find_rendered_matches={} find_fallback_matches={}",
        wakes.load(Ordering::Relaxed),
        math::generation(),
        find_rendered.len(),
        find_fallback.len()
    );
    assert_eq!(img_count, 50, "all 50 valid formulas rendered asynchronously");
    assert_eq!(find_rendered.len(), 0, "rendered math skipped in Markdown::find (0 ghost matches)");
    assert_eq!(find_fallback.len(), 1, "failed fallback math still searchable in Markdown::find");
    math::rest();
    println!("ALL FUZZ & PROOF STAGES PASSED!");
}
