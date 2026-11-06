- **Markdown can be styled to match OpenCode's `markdown.css` exactly.** New `MarkdownStyle`
  builders: `heading_font(4, …)` styles h4 to h6 apart from h3, `quote(rule, width, pad, text)`
  sets a quote's rule width, padding and muted text, `link_hover` sets the hover colour,
  `code_block_text` sets the colour of uncoloured code, `shell_code` draws `bash`, `sh` and the
  other shell fences plain on their own fill, `list_markers_outside` ends numbers and bullets a
  space before the text as `list-style-position: outside` does, `list_nested_indent` sets
  nested lists' marker columns, `task_checkbox` sizes, places and colours task checkboxes, and
  `table_column_align(false)` ignores `:-:` and `--:` as `text-align: start` does. A clear
  `rule` colour draws `---` as spacing only. `Markdown::code_frame` wraps each code block's
  card, so an app can put a Copy button on it (`CodeFrame` gives its index, code and
  language). Spacing follows the CSS too: the block after a heading sits the heading's own
  bottom margin below it, and in a list item the first paragraph has no margins and a nested
  list sits 4 points above and below.
