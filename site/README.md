# Rune site

The docs site for Rune: what it is, its principles, its architecture and its roadmap. Vite and React, prerendered to
static HTML.

- `bun run dev`: dev server at `http://127.0.0.1:4241/team/rune/`.
- `bun run build`: typecheck, client and SSR builds, then prerender every route into
  `.cloudflare/output/v0/workers/default/assets/` (Build Output for the `cf` CLI). `--mode dev` builds for
  dev.anoma.ly.

Content lives in `src/content.tsx` (principles, layers, decisions, milestones, risks) and `src/pages.tsx`. The
source of truth for all of it is `PLAN.md` at the repo root; update the plan first, then the site.

Code examples are cut from Rune's source at build time. `scripts/snippets.ts` lists each one by file and marker,
highlights it with `scripts/highlight.ts` (copied from Abattoir) and serves the HTML as `virtual:snippets`; pages
render them with `<Code id="…" />`. A moved or renamed function fails the build with the snippet's name. The
how-it-works diagrams are plain HTML in `src/Diagram.tsx`.

## Hosting and deploys

The site is served at https://anoma.ly/team/rune and https://dev.anoma.ly/team/rune, the same way as Abattoir.

- A push to `dev` that touches `site/` deploys https://dev.anoma.ly/team/rune (Worker `rune-site-dev`).
- A push to `production` that touches `site/` deploys https://anoma.ly/team/rune (Worker `rune-site`). To release,
  fast-forward it to `dev`: `git push origin dev:production`. Never commit to `production` directly.

Nothing else deploys. `.github/workflows/site.yml` runs `bun run deploy:<production|dev>` with the
`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` secrets. The token needs Workers Scripts Write on the account and
Workers Routes Write on `anoma.ly`.

`cloudflare.config.ts` defines both environments, selected by the build mode. `worker.ts` redirects other hosts to the
canonical one, strips the base path `/team/rune` (from `src/base.ts`) and serves the static files. Vite's `base`, the
links (`withBase`) and the client router (`stripBase`) use the same constant.

`/team` on both hosts sits behind a Cloudflare Access app, so only Anomaly accounts get in. `workers.dev` and preview
URLs are off, so nothing bypasses Access.

## Design system

`src/system/` (`tokens.css`, `primitives.css`, `Frame.tsx`, `useScrollSpy.ts`) and the Geist fonts in
`public/fonts/` are copied unchanged, apart from the font URLs, from `packages/site` in Abattoir (Anomaly's docs
system, direction "Workbench"). `src/site.css` sets Rune's accent by overriding `--ink` and adds only what the system
lacks: the wordmark, the home page, the diagrams, the decisions table and the roadmap rows. Once `@anomalyco/site` is
published, import it instead of these copies; until then, copy changes across by hand rather than editing them here.

The screenshots in `public/shots/` are Rune's own headless renders (`--snap`) of the examples, some resized.

## Brand

The logo is ᚱUNE on Abattoir's pixel grid, the R swapped for the rune ᚱ. `src/brand/pixels.ts` holds the letters;
`Wordmark.tsx` draws them in the sidebar (2 px a cell) and as the home masthead (12 px), and runs a light down the ᚱ's
strokes on hover, and every 5–9 s on the home page while the masthead is on screen (`motion.ts`). `bun run icons`
redraws the favicons, app icons and `og.png` in `public/` from the same pixels.

The prerender also writes `404.html` (served by the Worker for missing paths) and `llms.txt`.
