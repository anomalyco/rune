- **Markdown container nesting follows Blink's 511-element parser bound with iterative rendering and
  bounded streaming tails.** `Markdown` now nests up to `511` open container elements (`255` nested
  lists, `1` quote + `255` nested lists, or `510` non-empty block quotes, matching Chromium's
  `DOMParser` `kMaximumHTMLParserDOMTreeDepth` = `512` under `<html><body>`), keeps bullets and task
  checkboxes when a list opens inside a deep quote, evaluates `Renderer::blocks_in`, `bidi::Dirs`,
  `collapsed`, and `quote_frame` `plain_text` iteratively in linear time so `510` nested quotes open
  within a `2 MiB` WebAssembly stack across Chromium, Firefox, and WebKit, and caps
  pathological flattened tails past the bound at `64` lines so streaming stays fast per token.
