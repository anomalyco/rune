- **Web apps embed Inter cut to Latin text and fetch the full faces after the first frame.**
  The cut keeps every Latin glyph, kerning pair and layout feature, so Latin text draws pixel
  for pixel as before, and the wasm is 172 KB smaller with Brotli (582 KB raw): web-hello's
  first frame came 46 ms sooner at 20 Mbit/s. The full faces (`Inter-*.ttf`) load from beside
  `app.js` once the first frame is up, and every text system swaps them in and lays text out
  again. `scripts/web-build.sh` copies them into its output; an app built another way must
  serve them there too, or text outside Latin keeps drawing in fallback fonts. Apps that turn
  off `embedded-fonts` are unchanged.
