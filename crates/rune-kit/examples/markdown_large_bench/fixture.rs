//! Shared deterministic Markdown fixtures for large-document memory and frame benchmarks.

/// Generates a ~950 KB Markdown document (~10,500 lines, 488 table rows, 190 fenced code blocks
/// across 6 languages, headings, lists, links, inline code, no images).
pub fn generate_large_guide() -> String {
    let mut out = String::with_capacity(1_000_000);
    out.push_str("# Developer Architecture Guide\n\n");
    out.push_str("Comprehensive reference covering layout, rendering, storage, networking, and diagnostics.\n\n");
    let langs = ["rust", "typescript", "json", "python", "bash", "sql"];
    let mut table_rows = 0usize;
    let mut code_blocks = 0usize;
    let mut sec = 0usize;
    while out.len() < 948_000 || code_blocks < 190 || table_rows < 488 {
        sec += 1;
        out.push_str(&format!("## Section {sec}: Subsystem Design and Invariants\n\n"));
        for p in 0..3 {
            out.push_str(&format!(
                "Paragraph {sec}.{p} describes the **key invariants** for `subsystem_{sec}` including *incremental layout*, \
                 [reference links](https://example.com/docs/section-{sec}/part-{p}), `inline_code_token_{p}`, and bounded caches \
                 that drop unused allocations once a view settles at rest after navigation.\n\n"
            ));
        }
        out.push_str(&format!(
            "- Item {sec}.a: validate `FrameBudget` and `CacheLimit` on every state transition.\n\
             - Item {sec}.b: ensure virtualized rows and non-virtualized documents both release stale memory.\n\
             - Item {sec}.c: track `layout_pass`, `paint_pass`, and `text_shape` counts per commit.\n\n"
        ));
        if code_blocks < 190 {
            let lang = langs[code_blocks % langs.len()];
            code_blocks += 1;
            out.push_str(&format!("```{lang}\n"));
            for k in 0..14 {
                match lang {
                    "rust" => out.push_str(&format!(
                        "pub fn handle_event_{sec}_{k}(state: &mut State, delta: f32) -> bool {{ state.count += {k}; delta > 0.0 }}\n"
                    )),
                    "typescript" => out.push_str(&format!(
                        "export function processItem{sec}_{k}(input: readonly string[]): number {{ return input.length + {k}; }}\n"
                    )),
                    "json" => out.push_str(&format!(
                        "  \"field_{sec}_{k}\": {{ \"enabled\": true, \"limit\": {}, \"name\": \"entry-{k}\" }}{}\n",
                        k * 128,
                        if k < 13 { "," } else { "" }
                    )),
                    "python" => out.push_str(&format!(
                        "def compute_metric_{sec}_{k}(values: list[float]) -> float: return sum(values) / max(len(values), {k} + 1)\n"
                    )),
                    "bash" => out.push_str(&format!(
                        "echo \"step {sec}.{k}: checking service_status_{k} with threshold=$(( {k} * 64 ))\"\n"
                    )),
                    _ => out.push_str(&format!(
                        "SELECT id, name, metric_{k} FROM telemetry_events WHERE section_id = {sec} AND value > {k} ORDER BY id LIMIT 50;\n"
                    )),
                }
            }
            out.push_str("```\n\n");
        }
        if table_rows < 488 {
            out.push_str("| Key | Component | Budget | Notes |\n| --- | --- | --- | --- |\n");
            for r in 0..12 {
                if table_rows >= 488 {
                    break;
                }
                table_rows += 1;
                out.push_str(&format!(
                    "| `key_{sec}_{r}` | Component {sec}.{r} | {} KiB | Verified invariant for row {table_rows} in section {sec} |\n",
                    (r + 1) * 64
                ));
            }
            out.push('\n');
        }
    }
    out
}

/// An ordinary 10-block chat reply (heading, paragraphs with inline code/links, bullet list,
/// code fence, small table, blockquote, numbered list, closing summary) to guard the common case.
#[allow(dead_code)]
pub fn generate_chat_message() -> String {
    [
        "## Build summary",
        "",
        "The **incremental pass** finished cleanly with `0` warnings across `rune-text` and `rune`. See [the layout notes](https://example.com/layout) for background.",
        "",
        "- Dropped throwaway table measurement layouts from the shared text cache.",
        "- Moved cold inherited paint fields behind indexed side scratch handles.",
        "- Verified bit-identical output on all existing Markdown and gallery tests.",
        "",
        "```rust",
        "pub fn content_widths(text: &str, spans: &[Span], style: TextStyle) -> (f32, f32) {",
        "    with_text(|t| t.content_widths(text, spans, style))",
        "}",
        "```",
        "",
        "| Metric | Before | After |",
        "| --- | --- | --- |",
        "| Cached layouts | 8,422 | 5,458 |",
        "| Inh size | 440 B | 84 B |",
        "",
        "> Headless measurements exclude GPU upload and swap-chain presentation.",
        "",
        "1. Run paired alternating `--profile perf` rounds.",
        "2. Compare medians and keep outliers in the report.",
        "",
        "Let me know if you want to measure a longer thread next.",
        "",
        "### Follow-up",
        "",
        "Part 2 adds opt-in block virtualization for long documents while keeping short messages eager.",
        "",
    ]
    .join("\n")
}
