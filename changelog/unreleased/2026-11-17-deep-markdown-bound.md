- **Markdown container nesting follows Blink's 511-element parser bound with iterative rendering and
  lossless deep-tail grouping.** `Markdown` nests up to `511` open container elements (`255` nested
  lists, `1` quote + `255` nested lists, or `510` non-empty block quotes on Windows, macOS, and
  Linux; `770` flex containers / `385` pure block quotes on `wasm32` and other targets outside
  `rune-taffy`'s segment gate), keeps bullets and task checkboxes when a list opens inside a deep
  quote, evaluates `Renderer`, `bidi::Dirs`, `collapsed`, and `quote_frame` `plain_text` iteratively
  in linear time, and groups items past `64` flattened tail items into one selectable, searchable,
  copyable plain block with a visible [`rune::Label::MoreNestedLines`] marker so streaming stays
  fast per token without ever dropping text.
