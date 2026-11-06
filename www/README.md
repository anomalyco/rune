# Rune www

Rune's public page: what Rune is, shown with Rune itself. Vite and React, prerendered to static HTML and hydrated.

- `bun run dev`: dev server.
- `bun run build`: typecheck, client and SSR builds, then prerender `index.html`, `404.html`, `robots.txt`,
  `sitemap.xml` and `llms.txt` into `.cloudflare/output/v0/workers/default/assets/` (Build Output for the `cf` CLI).
  `--mode dev` builds for dev.anoma.ly: `noindex` on every page and a `Disallow: /` robots.txt.
- `bun run serve`: serves the build through `worker.ts` with brotli, close to what Cloudflare does, at
  `http://127.0.0.1:4253/rune/`. It caches compressed files, so restart it after a build.
- `bun run check`: typecheck only.

Copy and sections live in `src/App.tsx`; the hero's live panel, the spring demo and the frame diagram are in
`src/sections/`. Headings state the point of their section, not a topic.

## Hosting and deploys

The page is served at https://anoma.ly/rune and https://dev.anoma.ly/rune.

- A push to `dev` that touches `www/` deploys https://dev.anoma.ly/rune (Worker `rune-www-dev`).
- A push to `production` that touches `www/` deploys https://anoma.ly/rune (Worker `rune-www-production`). To
  release, fast-forward it to `dev`: `git push origin dev:production`.

`.github/workflows/www.yml` runs `bun run deploy:<production|dev>` with the `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` secrets, the same ones the docs site uses.

`cloudflare.config.ts` defines both environments, selected by the build mode. `worker.ts` redirects other hosts to the
canonical one, strips the base path `/rune` (from `src/base.ts`), serves the static files and sets cache headers:
hashed assets for a year; images, fonts and the live panel's files for a day, then revalidated. `workers.dev` and preview URLs are off.

## Images

`bun run images` cuts the page's images from Rune's own headless renders in `demo/` and the docs site's shots into
AVIF and WebP at the widths the page shows, into `public/img/`, and writes `src/images.json` with each one's size so
every image has its box before it loads. Add an image by adding it to the list in `scripts/images.ts`.

`bun run icons` draws the favicons, app icons and `og.png` from the wordmark's pixels in `src/brand/pixels.ts`.

Code on the page is cut from Rune's source at build time (`scripts/snippets.ts`, the same way as the docs site), so a
renamed function fails the build instead of going stale.

## The live panel

The hero panel is `examples/web-showcase` compiled to wasm, in `public/live/`. Until it draws, an HTML copy of its
first frame (`Poster` in `src/sections/LivePanel.tsx`) holds the same box, so nothing shifts and the headline stays
the largest paint. The wasm loads only after the page has loaded and gone idle, once someone moves, touches, scrolls
or types, when the panel is near the screen, and only where WebGPU has an adapter. Without WebGPU the copy stays and
the caption says why.

`public/live/` isn't committed: Rune's history is public and keeps every file forever, and each build is a 6 MB
wasm. CI builds it before every deploy (`.github/workflows/www.yml`), which is why changes to Rune, the example or
`scripts/web-build.sh` deploy the page too. Without it, `bun run build` warns and the page works with the poster in
place of the live panel.

To build it locally, from the repo root, on a machine with Rust, the `wasm32-unknown-unknown` target, the
`wasm-bindgen` CLI at the version in `Cargo.lock` and binaryen's `wasm-opt`:

```sh
scripts/web-build.sh web-showcase --out www/public/live --no-html --opt
rm www/public/live/*.d.ts
```

Unset `RUSTC_WRAPPER` if you use a compiler cache, so it can't undo the path stripping `web-build.sh` does.

The result is about 2.5 MB gzipped. The showcase's card is 376 px wide, or the panel less 16 px down to 280; if you
change it, change the poster's numbers in `src/site.css` to match.

## Motion

Nothing moves on first paint. Sections fade up once as they arrive (`src/motion/reveal.ts`). The spring demo runs
Rune's spring (`src/motion/spring.ts`, a port of `rune-motion`'s closed-form solver), so a retarget keeps the current
velocity, and a held puck follows the finger without animation. Every loop shares one `requestAnimationFrame`
(`src/brand/motion.ts`), runs only while something moves and on screen, and touches only transforms and opacity.
Under reduced motion, travel becomes a short fade.

## Design system

`src/system/tokens.css` and the Geist fonts in `public/fonts/` are copied from the docs site's design system.
`src/site.css` holds everything else.
