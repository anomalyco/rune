- **Web builds bundle smaller Inter faces.** With `embedded-fonts`, the wasm now carries copies of
  Inter Regular, Medium and SemiBold without TrueType hinting or glyph names, which the web's
  rasterizer never reads: 1.25 MB → 0.89 MB raw and 479 KB → 352 KB with Brotli. Outlines,
  advances, character maps, layout tables and names are unchanged, and a test checks every glyph.
  `scripts/web-fonts.sh` regenerates them; native builds keep the hinted originals.
