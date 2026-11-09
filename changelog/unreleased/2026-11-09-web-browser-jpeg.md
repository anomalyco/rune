- **Web builds hand JPEG to the browser.** On the web, `ImageCache::load` passes encoded bytes
  other than PNG, SVG and ICNS to the browser's own codecs (`createImageBitmap`, off the main
  thread, upright by EXIF orientation), as `ImageCache::url` already did, so WebP, AVIF and
  GIF's first frame load from bytes too. A web build no longer compiles in a JPEG decoder: about
  140 KB less wasm, 30 KB less with Brotli, in an app that loads images. PNG still decodes in
  Rune, which keeps its straight alpha exact. `web-hello?image-proof` and `scripts/web-smoke.mjs`
  check both paths.
