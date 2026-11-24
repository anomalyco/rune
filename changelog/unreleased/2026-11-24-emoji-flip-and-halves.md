- **Colour emoji that drew blank or two cells wide on macOS draw as Core Text draws them.** The
  108 people facing right (🚶‍➡️, 🏃🏽‍➡️, 🧑‍🦯‍➡️ and their skin tones) drew nothing: Apple Color Emoji
  stores each as a `flip` of the left-facing picture, which swash doesn't decode, so Rune reads the
  record and mirrors that picture (`raster/sbix.rs`). And sequences drawn as two half pictures (a
  couple with a heart, a kiss, a handshake in two skin tones) took two advances and drew off centre:
  harfrust planned their runs without zeroing the right half's advance, as HarfBuzz does. Rune now
  patches harfrust (`vendor/harfrust`, through `[patch.crates-io]`), so an app that builds Rune
  adds `harfrust` to its own `[patch.crates-io]` beside `parley_engine`.
