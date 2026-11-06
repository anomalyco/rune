# Rune www

Rune's public page, written in Rune. The whole page is one Rune app, `app/` (package `rune-www`), in web document
mode: the browser scrolls a semantic mirror of it (headings, paragraphs, real links, landmarks), and the canvas draws
the part in view. The same app writes its own static HTML with `--html`, which is the page's first paint and what
crawlers, readers without WebGPU and readers without JavaScript get.

- `bun run build`: builds `app/` to wasm (`src/wasm/`, size-optimized with LTO and `wasm-opt`), typechecks, bundles
  `src/main.ts` and the Worker with Vite, then runs the native app with `--html` to write `index.html` and `404.html`,
  and adds `robots.txt`, `sitemap.xml` and `llms.txt`, into `.cloudflare/output/v0/workers/default/assets/` (Build
  Output for the `cf` CLI). `--mode dev` builds for dev.anoma.ly: `noindex` on every page and a `Disallow: /`
  robots.txt. `--no-opt` skips the size passes for a quicker, larger build.
- `bun run serve`: serves the build through `worker.ts` with brotli, close to what Cloudflare does, at
  `http://127.0.0.1:4253/rune/`. It caches compressed files, so restart it after a build.
- `bun run check`: typecheck only. `cargo test -p rune-www` tests the app.

The build needs Rust with the `wasm32-unknown-unknown` target, the `wasm-bindgen` CLI at the version in `Cargo.lock`
and binaryen's `wasm-opt`. Unset `RUSTC_WRAPPER` if you use a compiler cache, so it can't undo the path stripping
`scripts/web-build.sh` does.

Copy and sections live in `app/src/`: the hero and its live card in `hero.rs`, the rest in `sections.rs`, the spring
demo in `springs.rs`, the frame diagram in `pipeline.rs`, the wordmark in `wordmark.rs` and colours, type and spacing in
`tokens.rs`. Headings state the point of their section, not a topic. The tree is the same at every width and only
styles change at the breakpoints, so the static page switches them with `@media` rules.

## Loading

`index.html` is the template (title, description, Open Graph and Twitter cards, canonical URL); `scripts/build.ts`
fills it in, adds JSON-LD, and the app writes the page's body into it, laid out at each breakpoint. That page paints
on its own with no script.

`src/main.ts` then loads Rune, only where `navigator.gpu` exists, and only after the static page has painted its
largest text, so the wasm never competes with it. Rune's first frame matches the static page, then hides the static
copy (it stays in the DOM, inert, so nothing shifts). Without WebGPU, or if the wasm fails, the static page stays.

The wasm isn't committed: Rune's history is public and keeps every file forever. CI builds it before every deploy,
which is why changes to Rune deploy the page too.

## Hosting and deploys

The page is served at https://anoma.ly/rune and https://dev.anoma.ly/rune.

- A push to `dev` that touches `www/` or Rune deploys https://dev.anoma.ly/rune (Worker `rune-www-dev`).
- A push to `production` that touches `www/` or Rune deploys https://anoma.ly/rune (Worker `rune-www-production`). To
  release, fast-forward it to `dev`: `git push origin dev:production`.
- A pull request runs the tests and the dev build and deploys nothing.

`.github/workflows/www.yml` runs `bun run build` and `cf deploy --prebuilt` with the `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` secrets, the same ones the docs site uses.

`cloudflare.config.ts` defines both environments, selected by the build mode. `worker.ts` redirects other hosts to the
canonical one, strips the base path `/rune` (from `src/base.ts`), serves the static files and sets cache headers:
hashed assets (the wasm and its glue) for a year; images and fonts for a day, then revalidated. `workers.dev` and
preview URLs are off.

## Images and fonts

`bun run images` cuts the page's images from Rune's own headless renders in `demo/` and the docs site's shots into
AVIF at the widths the page shows, into `public/img/`. The widths and aspect ratios match the `Picture`s in
`app/src/sections.rs`, which hand the browser a `srcset` so it picks one size; every image has its box before it loads.

The metal, glass and light stills are drawn by `examples/site-scenes`, which runs rune-fx and rune-light headless at
2x over a frame Rune draws. Their PNGs aren't committed, only the cut images, so render them before `bun run images`:

```sh
cargo run --release -p site-scenes            # all three, into target/site-scenes/
cargo run --release -p site-scenes -- --only glass --t 2.5
```

`--t` is the effect clock in seconds; the liquid metal's flow and the light's sway depend on it.

`bun run icons` draws the favicons, app icons and `og.png` from the wordmark's pixels in `src/brand/pixels.ts`.

The Geist fonts in `public/fonts/` are TTF, because Rune loads fonts by URL and doesn't read WOFF2; the static page
uses the same files.

## Motion

Nothing moves on first paint, and an idle page draws no frames. The wordmark's ᚱ catches a light on hover, and now and
then in the masthead while it's on screen. The hero card's switch and puck, and the spring demo, run `rune-motion`'s
springs, so a retarget keeps the current velocity and a held puck follows the finger without animation. The frame
diagram runs once when it comes into view and again on a click. Under reduced motion, travel becomes a short fade and
the idle light stays off.
