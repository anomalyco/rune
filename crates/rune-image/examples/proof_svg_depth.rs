//! Proof program for anomalyco/rune#1139 (proof branch only, never merged): SVG nesting depth bound.
//! Runs `rune_image::svg_rgba` on a thread with the decoder's 8 MiB stack, as `rune-images` does.

use std::time::Instant;

fn nested(depth: usize, filler: &str) -> String {
    let mut s = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">"#);
    for _ in 0..depth {
        s.push_str("<g>");
        s.push_str(filler);
    }
    s.push_str(r#"<rect width="16" height="16" fill="red"/>"#);
    for _ in 0..depth {
        s.push_str("</g>");
    }
    s.push_str("</svg>");
    s
}

fn run(name: &str, svg: String, want_ok: bool) -> bool {
    let t = Instant::now();
    let r = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || rune_image::svg_rgba(svg.as_bytes(), 16).map(|(w, h, _)| (w, h)))
        .expect("spawn")
        .join()
        .expect("decoder thread did not overflow or panic");
    let ok = r.is_ok() == want_ok;
    println!("{} {name:32} {:>9.1} ms  {:?}", if ok { "PASS" } else { "FAIL" }, t.elapsed().as_secs_f64() * 1e3, r.map_err(|e| e.0));
    ok
}

fn main() {
    let cases = [
        ("depth 1", nested(1, ""), true),
        ("depth 400", nested(400, ""), true),
        ("depth 511", nested(511, ""), true),
        ("depth 600 (refused)", nested(600, ""), false),
        ("depth 5000 (refused)", nested(5_000, ""), false),
        ("depth 200000 (refused)", nested(200_000, ""), false),
        ("depth 400, comments with <g>", nested(400, "<!-- <g><g><g> -->"), true),
        ("depth 400, CDATA with <g>", nested(400, "<![CDATA[<g><g>]]>"), true),
        ("depth 600, self-closing filler", nested(600, "<g/>"), false),
    ];
    let failed = cases.into_iter().filter(|(n, s, ok)| !run(n, s.clone(), *ok)).count();
    println!("{}", if failed == 0 { "ALL PASS" } else { "SOME FAILED" });
    std::process::exit(i32::from(failed != 0));
}
