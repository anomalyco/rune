# Changelog

Rune has no releases yet; consumers pin `dev` revs. This lists changes that can move an app's output
without a code change (goldens, layout, input), newest first, with the rev that introduced each.

## dev

- **Wrapped centred and end-aligned text aligns each line** (#8). A wrapping `Align::Center`/`End`
  text now centres (or right-aligns) every line inside its wrap width, instead of moving the whole
  block as one left-aligned paragraph. `--snap` goldens with multi-line centred text move.
- **Single-line text fields** (#8). `El::single_line()` keeps a text input on one line: it scrolls
  sideways to keep the caret visible, clips to the field, and rests at the start when unfocused.
  rune-kit `TextInput` (except `multiline`) and `List` search opt in; core `text_input` is unchanged.

- **Scroll views clip their content** (#7). `El::scroll` now clips its children to its box with a
  hard edge, so overflowing content no longer draws, or takes presses, outside it. An explicit
  `clip_soft`/`clip_fade` still sets the edge. No `--snap` goldens move.
- **Optical sizing on by default** (`2eae09d`, from `edc9892`). Text with an `opsz` axis now uses each
  run's point size, as AppKit does: system SF at 13 pt renders as SF Text, so labels are slightly
  wider. Layout is otherwise unchanged, but `--snap` goldens with SF text move (Scrawl: all 7, PAE
  ~0.85). Opt out per style with `.optical_sizing(OpticalSize::FontDefault)`. Fonts without `opsz`
  are unaffected.
- **Presses inside scrolled views land** (`b3c7bdb`, #5). A captured press is released against
  where the element was drawn (after scrolling), not its unscrolled layout rect, so `on_press` now
  fires for rows scrolled into view.
