# Run a Rune app in the browser

Rune's experimental web target renders one canvas through WebGPU. It uses the same Rust
views, layout, glyph atlas and shaders as native. GPU initialization is asynchronous; the
runtime stays on the browser's main thread and requests frames only while work is pending.

## Build the example

Install the wasm target, Python 3.11 or newer, and the wasm-bindgen CLI version in Cargo.lock:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
scripts/web-build.sh web-hello --out target/web
```

Serve the entire output folder over HTTPS, including `snippets/`. A local HTTP server on
`localhost` also works for development. The host must serve `.wasm` as `application/wasm`.
No application server, SharedArrayBuffer, COOP or COEP headers are required.
`--opt` additionally runs an installed `wasm-opt -Oz`; the default build does not need it.
The script prints the raw and gzip wasm sizes. Configure gzip or Brotli on the static host
to get the corresponding transfer size.

The example is a scrollable message list with a composer, Send and Copy last. Its title
opens an external link. Refreshing resets the in-memory messages.

## Give your app a wasm entrypoint

Add a `cdylib` target and a wasm-only wasm-bindgen dependency:

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen = "0.2"
```

Export a start function that calls the usual Rune entrypoint:

```rust,ignore
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    rune::run(AppSpec::new("My app", (390.0, 760.0), State::default, view));
}
```

From your app's workspace, run `path/to/rune/scripts/web-build.sh your-package`. The script
selects one cdylib, or one binary if there is no cdylib. `--example NAME` selects a Cargo
example. Disable Rune's `dev`, `hot` and `native-file-dialog` features on wasm. Move native
filesystem, process and thread dependencies behind target conditions in your app too.
Use `rune::time::Instant` for deadlines passed to Rune; it is `std::time::Instant` on native.

## Mount in a page

By default Rune appends its own canvas, covers the viewport and stops the body scrolling.
`App::canvas_id("id")` mounts onto a `<canvas id="id">` the page already has instead:

```rust,ignore
// Inline in an article, as tall as its content; the page keeps scrolling.
App::new(spec).canvas_id("chart").fit_content_height().try_run()?;
// Full screen inside a single-page app's route, which owns the canvas.
App::new(spec).canvas_id("route-canvas").full_viewport().try_run()?;
```

Inline, the canvas is as wide as its parent and as tall as `AppSpec::size` (or its content with
`fit_content_height`, when vertical wheel and touch pans scroll the page); safe-area insets
are zero and the title doesn't touch `document.title`. With `full_viewport` the canvas is fixed to
the visual viewport like Rune's own, so an ancestor with a `transform` or `filter` would
become its containing block; mount it outside one.

`AppHandle::quit` unmounts the app when its route goes away: the loop stops, the GPU device is
destroyed, Rune's listeners come off the page, and the body style and title it changed go back.
Rune removes a canvas it made; a passed-in canvas stays, with its own style and attributes
back, for the page to remove. One app runs per page at a time. An app started while another is
quitting mounts once that one has gone, so a route can swap apps in the same tick:

```rust,ignore
let handle = app.handle();           // keep it with the route
App::new(spec).canvas_id("route-canvas").full_viewport().try_run()?; // after handle.quit()
```

## Images

`ImageCache::url` fetches an image and lets the browser decode it with its own codecs
(`createImageBitmap`, off the main thread), so WebP, AVIF and anything else the browser reads
work, turned upright by EXIF orientation. The pixels land through the same mip path as
`ImageCache::rgba` and the cache's waker draws the frame. `ImageCache::file` does the same on
web with its path as a URL relative to the page, so one call loads a bundled asset in both
builds:

```rust,ignore
let logo = cache.file("assets/logo.webp", 48.0, env.scale); // disk on native, fetched on web
let avatar = cache.url(&user.avatar_url, 32.0, env.scale);  // size and scale matter for SVG only
```

SVG (by `Content-Type` or a `.svg`/`.svgz` path) rasterizes with Rune's renderer at the size
asked for. Images over 8192 pixels a side are refused before their pixels are read, and a
failed request or a cross-origin image without CORS headers resolves to `Image::error`.

## Fonts and browser input

Web includes Inter Regular and IBM Plex Mono Regular. There is no system-font discovery.
Set `AppSpec::fonts` to static font bytes and `AppSpec::families` to your sans/mono family
names for branding, additional weights, emoji and scripts outside the default fonts.
For example, extend a static slice of `rune_theme_opencode::FONTS` with
`include_bytes!("assets/NotoColorEmoji.ttf")` and the CJK fonts your app needs, then assign
the combined slice to `spec.fonts`. Keep the theme's sans/mono family names in `spec.families`.
Missing emoji and CJK glyphs display fallback boxes; they do not trigger font downloads.

Rust supplies the hidden textarea with up to 512 UTF-16 units on each side of the selection,
expanding for the selected range, and skips unchanged mirrors. Browser input events supply replacements; Rune validates the mirrored
context and converts window-relative UTF-16 positions to UTF-8 boundaries before editing.
Composition holds the mirror stable until commit and replaces the composing word's range.
Mobile `beforeinput` line breaks use Rune's Enter rules: submit, or Shift/Alt+Enter newline
in a multiline field. A completed touch tap focuses the editor synchronously;
a pan scrolls instead, with momentum and a damped edge return.

Secure fields use an `<input type="password">` with autocomplete, autocorrect and spellcheck
disabled. It receives only new keystrokes and is emptied after every input; the model's
secret is never mirrored into the DOM. Leaving a field returns focus to the canvas so
shortcuts and Tab keep working. Continuous pointer movement and wheel events wait for rAF.

`Tx::copy` and `Tx::request_paste` use the browser clipboard. Call them from a user input
handler: permission and transient-activation rules still apply. Native Paste events work
without an asynchronous clipboard read. Secret-copy requests report failure because the
browser cannot promise native clipboard-history exclusion or conditional timed clearing.

`rune::web::open_url` accepts HTTP(S) and mail links and opens a tab with `noopener,noreferrer`.
Call it from a user input handler. `Tx::open_url_same_tab` (or `rune::web::open_url_same_tab`)
replaces the page instead, and takes links relative to the page (`/notes/42`). A single-page
app routes those itself with `App::same_tab_router`: it gets the absolute URL of each link on
the page's origin and returns true when it handled it; otherwise, and for other origins, the
browser loads the link with `location.assign`.

```rust,ignore
App::new(spec).same_tab_router(|href| router.push(href)) // true: routed with pushState
tx.open_url_same_tab("/notes/42")?;                       // from a handler
```
 `Cx::safe_area()` returns top/right/bottom/left insets in
points (zero on native). Apply them inside a full-canvas background. The canvas follows
`visualViewport` when a software keyboard reduces the visible height. Keep the generated
page's `viewport-fit=cover` meta tag and the textarea's 16 px font size.

## Scope and verification

This target needs WebGPU. It has no WebGL fallback, DOM accessibility mirror, overlays,
extra windows, native menus, drag-out, native audio/video, networking layer or hot patching.
Native configuration types remain source-compatible, but their desktop host paths are not
used on wasm. Unsupported window requests warn in the console; Show/Open are ignored.
The document title follows `AppSpec::title`, and hidden tabs report `is_visible() == false`.

The functional proof uses headless Chromium and Playwright:

```sh
node scripts/web-smoke.mjs target/web target/web-proof
```

Install Playwright with its Chromium browser first. `PLAYWRIGHT_MODULE` and `CHROMIUM_PATH`
can select an existing installation. `WEB_ENGINE=webkit` tries an installed Playwright WebKit
and reports whether it has a WebGPU adapter. The proof checks GPU errors, idle rAF requests
and submissions, touch
focus and momentum, Unicode edits, a long sliding context, predictive replacement,
composition, selection, paste, copied model text, link isolation, theme and viewport resize.
The `?input-proof` fixture adds password input, Tab/Escape focus, mobile Enter, recomposition,
same-field focus stability, continuous-input coalescing and visibility regression checks.
Its mobile DPR check exercises the Safari-style CSS-size fallback because Chromium device
emulation does not scale `devicePixelContentBoxSize`. This is not a real iPhone keyboard,
dictation, autocorrect or Safari WebGPU test; those still need device verification.

The shell follows PLAN's winit-first approach, retaining winit 0.30's rAF and ResizeObserver
implementation. Unlike research/11's proposed custom shell, it adds a document-aware
textarea beside winit rather than replacing winit. This also defers PLAN's proposed switch
to an owned shell when text input lands. WebGL fallback, EditContext and a DOM
accessibility mirror from that research remain later work.
