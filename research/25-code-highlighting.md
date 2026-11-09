# Syntax highlighting in Markdown code fences

How rune-kit colours fenced code, and why it uses hand-written scanners instead of a grammar
engine.

## What it has to do

- Colour the languages chat answers use most: TypeScript/TSX/JavaScript, JSON, Rust, Swift,
  CSS, HTML, shell, SQL and YAML. Anything else stays plain.
- Treat the input as untrusted: model output can be huge or adversarial, so time must be linear
  in the source with no backtracking, and work per block must be capped.
- Stream without layout shift: a fence grows a line at a time, and colouring must not move a
  glyph.
- Build everywhere Rune builds, including wasm and iOS, without adding much to binaries.

## Options

**syntect** (Sublime Text grammars). The most accurate of the three for many languages, but
it ships a serialized grammar set (hundreds of KB to a few MB, depending on the languages kept),
loads it lazily at a noticeable startup cost, and its grammars rely on regexes with lookaround,
run by `onig` (C) or `fancy-regex`. Both backtrack, so a crafted line can take super-linear
time. Highlighting is per line with carried state, which fits streaming, but the cost and the
backtracking risk don't fit untrusted chat text.

**tree-sitter**. Real incremental parsing and the best structure, but each language is a
generated C parser (the TypeScript and TSX grammars alone are large), the runtime is C, and
highlighting needs a query file per language. Every target, wasm and iOS included, then needs a
C toolchain, and the size cost is several hundred KB per language. It is the right tool for an
editor, more than a chat transcript needs.

**Hand-written scanners** (chosen). One small byte scanner per language family (C-like, shell,
markup, CSS, SQL, YAML), in the same spirit as the native mermaid renderer. Each only moves
forward; lookaheads are bounded (a regex literal at most 512 bytes) or skip bytes the scanner
would otherwise visit; embedded code (template `${}`, JSX `{}`, `<script>`, `$()`) nests at most
three deep. Pure Rust, no dependencies, no data files, wasm-friendly. The cost is semantic
precision: it recognises tokens, not grammar, so an unusual construct can colour slightly
wrong. For reading an answer that is an acceptable trade, and the scanners err toward leaving
text plain.

## Decisions

- **Colour only.** Spans set a brush and nothing else: no weight, italic or size, so shaping
  sees the same runs and glyph positions are identical with and without colour. A unit test
  lays out the same code plain, coloured and part-coloured and compares every glyph quad.
- **Streaming.** An unclosed fence colours its complete lines only; the line still arriving
  draws plain and takes its colour when its newline arrives.
- **Caps.** The first 32 KiB of a block (cut at a line end) and at most 8192 coloured runs;
  the rest draws plain. Results are cached per (language, source) for the last 32 blocks on
  the thread, the same bound as the parse cache.
- **Colours.** Eight token kinds from `MarkdownStyle`. Defaults use fixed familiar hues (violet
  keywords, green strings, orange numbers, teal types, blue functions, rose properties) at one
  lightness and chroma derived from the theme's text colour, and the theme's muted and faint
  text for punctuation and comments. Theme status colours were tried as the hue source, but
  they vary too much between themes (one theme's "info" is magenta) to keep kinds apart.
- **No new feature flag.** The scanners are about a thousand lines of plain Rust with no
  dependencies, so they live under `markdown`.
- **`Block` keeps its shape.** Each code block's language travels beside the parsed blocks
  rather than in a new `Block` variant, so apps that match on `Block::Code` keep working.

## 2026-11: Desktop's own lexers, translated

OpenCode Desktop doesn't highlight with a TextMate engine. Its diffs, file views and Markdown use
Pierre's Highlights (`@pierre/highlights`, Apache-2.0): 80 hand-written lexers, written in
WebAssembly Text and shipped as one 183 KB module, whose tokens are Zed capture names that
Desktop's theme maps to its `syntax-*` colours. The scanners above covered 20 of those
languages and coloured 82.9% of their samples' bytes as Desktop does.

Options, measured on a Windows x64 desktop shared with other builds, with one sample per language, repeated to 1,000+ lines,
optimised builds; sizes are the growth of a minimal binary:

| Option | Languages | Native size | Web (wasm32, gzip) | ms per 1,000 lines | Same colours as Desktop |
|---|---:|---:|---:|---:|---:|
| Hand-written scanners (before) | 20 | +81 KB | +25 KB | 0.1-0.4 | 82.9% |
| Highlights, translated to Rust (chosen) | 80 | +429 KB | +175 KB | 0.16-0.37 quiet, 0.56-0.70 loaded (V8 runs the module at 0.24 and 0.58-0.72) | 100% (tokens identical) |
| Highlights in an interpreter (wasmi) | 80 | about +2.9 MB | the browser's own WebAssembly | 8.4 | 100% |
| syntect, 75 Sublime grammars | 75 | +2.46 MB | - | 147 (warm) | TextMate scopes; see Shiki |
| Shiki's grammars (TextMate, JS or Oniguruma) | 80 | 3.9 MB of grammar JSON (487 KB gzip) + engine | - | 41-49 | 81.0% |
| tree-sitter, 8 grammars (parse only) | 8 | +16.2 MB | needs a C toolchain | 2.4-15.4 | different grammar and queries |

The choice: `crates/rune-highlights` translates `highlights.wasm` to Rust in `build.rs`
(`codegen/translate.rs`, about 550 lines over `wasmparser`), so the same lexers run natively on
every target with no runtime and no compiling on users' machines. Its tokens are identical to
the module's in V8 on all 80 samples and 3,360 fuzzed sources (mutations, deep nesting, random
text); 200,000-deep nesting runs on a 1 MiB main-thread stack. A block streaming in uses
Highlights' stream mode: 0.044 ms per new line at 5,000 lines, against 1.5 ms to lex the whole
block again. A trap (a bug in the lexers) draws the code plain instead of panicking.
