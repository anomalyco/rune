# Changelog

Rune has no releases yet; consumers pin `dev` revs. This lists changes that can move an app's output
without a code change (goldens, layout, input), newest first, with the rev that introduced each.

## dev

- **Colour emoji** (#7). Emoji fall back to the system's colour emoji font (Apple Color Emoji,
  Segoe UI Emoji, Noto Color Emoji) and draw as colour images from the image atlas, where they
  used to draw as blank gaps. Text without emoji renders as before.

- **Cached overlay reveals preserve new motion.** A prewarmed macOS overlay settles hidden
  changes while preparing its cached frame; updates arriving after that frame in the same
  turn as `show()` now animate instead of snapping, including a hidden poll that commits
  the target and requests show together. Already-stale frames and ordinary overlay shows
  still settle before reveal.
- **rune-kit menus open instantly** (Abattoir "Open menus instantly, fade only the close"). `Menu`,
  `context_menu`, submenus and `Select`'s list now appear fully on the next frame instead of fading
  and scaling in from 0.96, and close with a plain `.25 / 0` fade (no scale). Menu item highlights
  land the frame the pointer arrives and leave over 100 ms ease-out. No `--snap` goldens move:
  they capture frame 0, where overlays already mount without an entrance.
- **Wrapped centred and end-aligned text aligns each line** (#6). A wrapping `Align::Center`/`End`
  text now centres (or right-aligns) every line inside its wrap width, instead of moving the whole
  block as one left-aligned paragraph. `--snap` goldens with multi-line centred text move.
- **Single-line text fields** (#6). `El::single_line()` keeps a text input on one line: it scrolls
  sideways to keep the caret visible, clips to the field, and rests at the start when unfocused.
  rune-kit `TextInput` (except `multiline`) and `List` search opt in; core `text_input` is unchanged.
- **Scroll views clip their content** (`3333f37`). `El::scroll` now clips its children to its box with a
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
