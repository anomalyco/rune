- **Markdown container nesting follows Blink's 511-element parser bound with iterative rendering and
  bounded streaming tails.** `Markdown` now nests up to `511` open container elements (`255` nested
  lists, `1` quote + `255` nested lists, or `510` non-empty block quotes on native and `385` nested
  block quotes = `770` flex containers on `wasm32` so V8's native WebAssembly call stack stays
  within bounds alongside a `2 MiB` shadow stack across Chromium, Firefox, and WebKit), keeps
  bullets and task checkboxes when a list opens inside a deep quote, evaluates `Renderer`,
  `bidi::Dirs`, `collapsed`, and `quote_frame` `plain_text` iteratively in linear time, and caps
  pathological flattened tails past the bound at `64` lines so streaming stays fast per token.
