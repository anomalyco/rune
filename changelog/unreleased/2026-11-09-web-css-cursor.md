- **Web image cursors are CSS.** On the web, `Cursor::Image` sets the canvas's CSS cursor to the
  PNG at 2x (`image-set(url(blob:…) 2x)`), so the browser decodes it and draws it at its size in
  points. It was twice too big on a 2x display, and an app that loads no images no longer
  compiles in a PNG decoder: 163 KB less wasm, 60 KB less with Brotli, in an app that only
  draws text. `web-hello?image-proof` and `scripts/web-smoke.mjs` check the cursor.
