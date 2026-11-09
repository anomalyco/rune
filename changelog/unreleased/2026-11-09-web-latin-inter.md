- **Web apps embed Inter cut to Latin, Greek and Cyrillic text and fetch the full faces after the first frame.**
  The cut keeps every glyph, kerning pair and layout feature for those scripts, so their text
  draws pixel for pixel as before, and web-hello's wasm is 153 KB smaller with Brotli (510 KB
  raw). The full faces (`Inter-*.ttf`) load from beside `app.js` once the first frame is up,
  and every text system swaps them in and lays text out again. `scripts/web-build.sh` copies
  them into its output; an app built another way must serve them there too, or other text
  (Vietnamese, polytonic Greek, Cyrillic beyond Russian, Ukrainian, Belarusian, Serbian,
  Macedonian and Bulgarian) keeps drawing in fallback fonts. Apps that turn off
  `embedded-fonts` are unchanged.
