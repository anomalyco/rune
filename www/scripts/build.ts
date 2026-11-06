#!/usr/bin/env bun
// `bun run build [--mode dev]`: the page is the Rune app in app/. This builds it to wasm (src/wasm/), bundles
// src/main.ts and the Worker with Vite into Build Output, then has the app write its own static HTML (`--html`) into
// index.html and 404.html, and adds robots.txt, sitemap.xml and llms.txt.
// Needs Rust with the wasm32-unknown-unknown target, the wasm-bindgen CLI at the version in Cargo.lock and binaryen's
// wasm-opt (`--no-opt` skips it).
import { readFileSync, writeFileSync } from "node:fs"
import { join, relative, resolve } from "node:path"
import { parseArgs } from "node:util"
import { getWorkerAssetsDir } from "@cloudflare/build-output-utils"
import { $ } from "bun"
import { BASE_PATH, hostForMode } from "../src/base"
import { DESCRIPTION, REPO, SHARE_TITLE, TITLE } from "../src/meta"

const { values } = parseArgs({
  options: { mode: { type: "string", default: "production" }, "no-opt": { type: "boolean", default: false } },
})
const production = values.mode !== "dev"
const origin = `https://${hostForMode(values.mode)}${BASE_PATH}`

const www = resolve(import.meta.dirname, "..")
const repo = resolve(www, "..")
// The commit the page tells you to pin: the one it was built from (or $RUNE_REV, outside a checkout).
const rev = process.env.RUNE_REV ?? (await $`git rev-parse --short=12 HEAD`.cwd(repo).text()).trim()
const env = { ...process.env, RUNE_REV: rev }
const cargo = process.env.CARGO ?? "cargo"
// The widths the static page is taken at: its breakpoints and the narrowest phone.
const widths = "360,400,560,760,860,1000"

// Smallest wasm: optimized for size with whole-program LTO, then wasm-opt. `--no-opt` builds quicker and larger.
const size = values["no-opt"] ? {} : { CARGO_PROFILE_RELEASE_OPT_LEVEL: "z", CARGO_PROFILE_RELEASE_LTO: "fat" }
const opt = values["no-opt"] ? [] : ["--opt"]
await $`scripts/web-build.sh rune-www --out www/src/wasm --no-html ${opt}`.cwd(repo).env({ ...env, ...size })
await $`tsc --noEmit`.cwd(www)
await $`vite build --mode ${values.mode}`.cwd(www)

const dir = getWorkerAssetsDir(www)
const built = readFileSync(join(dir, "index.html"), "utf8")
const escape = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;")

const jsonld = {
  "@context": "https://schema.org",
  "@graph": [
    { "@type": "WebSite", name: "Rune", url: `${origin}/` },
    {
      "@type": "SoftwareSourceCode",
      name: "Rune",
      description: DESCRIPTION,
      url: `${origin}/`,
      codeRepository: REPO,
      programmingLanguage: "Rust",
      runtimePlatform: ["macOS", "Windows", "Linux", "Web (WebGPU)", "iOS"],
      author: { "@type": "Organization", name: "Anomaly", url: "https://anoma.ly" },
    },
  ],
}

const head = (path: string, title: string, description: string, html: string) =>
  html
    .replace("<!--robots-->", production ? "" : `<meta name="robots" content="noindex, nofollow" />`)
    .replace("<!--jsonld-->", path === "/" ? `<script type="application/ld+json">${JSON.stringify(jsonld)}</script>` : "")
    .replaceAll("__URL__", `${origin}${path}`)
    .replaceAll("__SHARE_TITLE__", escape(path === "/" ? SHARE_TITLE : title))
    .replaceAll("__TITLE__", escape(title))
    .replaceAll("__DESCRIPTION__", escape(description))
    .replaceAll("__ORIGIN__", origin)

// The app lays itself out at each width and writes the page into the template; run from public/ so it finds the fonts.
const page = async (file: string, template: string, extra: Record<string, string> = {}) => {
  const path = join(dir, file)
  writeFileSync(path, template)
  await $`${cargo} run -q -p rune-www -- --html ${path} --html-widths ${widths} --html-template ${path}`
    .cwd(join(www, "public"))
    .env({ ...env, ...extra })
}

await page("index.html", head("/", TITLE, DESCRIPTION, built))
// The Worker serves this for any missing path under the base, with a 404. It has no wasm and no canonical URL.
const notFound = head("/404", "Not found · Rune", "There's no page here.", built)
  .replace(/\s*<script type="module"[^>]*><\/script>/g, "")
  .replace(/\s*<link rel="modulepreload"[^>]*>/g, "")
  .replace(/\s*<link rel="canonical"[^>]*>/, "")
await page("404.html", notFound, { RUNE_WWW_PAGE: "404" })

// Dev is for checking before release: keep it out of search.
writeFileSync(
  join(dir, "robots.txt"),
  production ? `User-agent: *\nAllow: /\n\nSitemap: ${origin}/sitemap.xml\n` : "User-agent: *\nDisallow: /\n",
)
if (production) {
  writeFileSync(
    join(dir, "sitemap.xml"),
    `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n  <url><loc>${origin}/</loc></url>\n</urlset>\n`,
  )
}

// llms.txt (llmstxt.org): what Rune is and where to read more.
writeFileSync(
  join(dir, "llms.txt"),
  [
    "# Rune",
    "",
    `> ${DESCRIPTION}`,
    "",
    "Rune is a Rust UI framework. Views are plain Rust functions of state; Rune diffs them, lays them out with Taffy,",
    "animates layout and properties with springs, and draws every frame with one instanced GPU draw. It runs on macOS",
    "(its own Metal backend), Windows, Linux, iOS (embedded in UIKit or SwiftUI) and the web (WebGPU, experimental).",
    "",
    "This page is itself a Rune app: the HTML is what the app writes with `--html`, and the canvas is the same app.",
    "",
    "## Source",
    "",
    `- [anomalyco/rune](${REPO}): the code`,
    `- [crates/rune/README.md](${REPO}/blob/dev/crates/rune/README.md): the framework guide`,
    `- [WEB.md](${REPO}/blob/dev/WEB.md): building a Rune app for the browser`,
    `- [CHANGELOG.md](${REPO}/blob/dev/CHANGELOG.md): what changed, newest first`,
    `- [www/app](${REPO}/tree/dev/www/app): this page's source`,
    "",
    "## Use it",
    "",
    "Add Rune to Cargo.toml pinned to a commit:",
    "",
    "```toml",
    `rune = { git = "${REPO}", rev = "${rev}" }`,
    "```",
    "",
  ].join("\n"),
)

console.log(`Built ${production ? "production" : "dev"} into ${relative(www, dir)}/`)
