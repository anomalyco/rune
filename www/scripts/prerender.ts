import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join, relative, resolve } from "node:path"
import { parseArgs } from "node:util"
import { getWorkerAssetsDir } from "@cloudflare/build-output-utils"
import { BASE_PATH, hostForMode } from "../src/base"
import { DESCRIPTION, REPO, SHARE_TITLE, TITLE } from "../src/meta"

const { values } = parseArgs({ options: { mode: { type: "string", default: "production" } } })
const production = values.mode !== "dev"
const origin = `https://${hostForMode(values.mode)}${BASE_PATH}`

const root = resolve(import.meta.dirname, "..")
const distDir = getWorkerAssetsDir(root)
const distRel = relative(root, distDir)
const ssrDir = join(root, "dist-ssr")
const templatePath = join(distDir, "index.html")

if (!existsSync(templatePath)) throw new Error(`${distRel}/index.html not found; run \`vite build\` first`)

let template = readFileSync(templatePath, "utf8")
if (!template.includes("<!--app-->")) throw new Error(`${distRel}/index.html is missing the <!--app--> marker`)

// Inline the one stylesheet so the first paint needs no extra request.
template = template.replace(new RegExp(`<link rel="stylesheet"[^>]*href="${BASE_PATH}/(assets/[^"]+\\.css)"[^>]*>`, "g"), (full, href: string) => {
  const cssPath = join(distDir, href)
  if (!existsSync(cssPath)) return full
  return `<style>${readFileSync(cssPath, "utf8").replace(/<\/style/gi, "<\\/style")}</style>`
})

const escape = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;")

const ssr = (await import(join(ssrDir, "entry-server.js"))) as { render: (pathname: string) => string }

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

const page = (path: string, title: string, description: string, body: string) =>
  template
    .replace(/<title>[\s\S]*?<\/title>/, `<title>${escape(title)}</title>`)
    .replace(/<meta name="description" content="[^"]*" \/>/, `<meta name="description" content="${escape(description)}" />`)
    .replace("<!--robots-->", production ? "" : `<meta name="robots" content="noindex, nofollow" />`)
    .replace("<!--jsonld-->", path === "/" ? `<script type="application/ld+json">${JSON.stringify(jsonld)}</script>` : "")
    .replaceAll("__URL__", `${origin}${path}`)
    .replaceAll("__SHARE_TITLE__", escape(path === "/" ? SHARE_TITLE : title))
    .replaceAll("__TITLE__", escape(title))
    .replaceAll("__DESCRIPTION__", escape(description))
    .replaceAll("__ORIGIN__", origin)
    .replace("<!--app-->", body)

writeFileSync(templatePath, page("/", TITLE, DESCRIPTION, ssr.render("/")))

// The Worker serves this for any missing path under the base, with a 404.
const notFound = page("/404", "Not found · Rune", "There's no page here.", ssr.render("/404"))
writeFileSync(join(distDir, "404.html"), notFound.replace(/\s*<link rel="canonical"[^>]*>/, ""))

// Dev is for checking before release: keep it out of search.
writeFileSync(
  join(distDir, "robots.txt"),
  production ? `User-agent: *\nAllow: /\n\nSitemap: ${origin}/sitemap.xml\n` : "User-agent: *\nDisallow: /\n",
)
if (production) {
  writeFileSync(
    join(distDir, "sitemap.xml"),
    `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n  <url><loc>${origin}/</loc></url>\n</urlset>\n`,
  )
}

// llms.txt (llmstxt.org): what Rune is and where to read more.
writeFileSync(
  join(distDir, "llms.txt"),
  [
    "# Rune",
    "",
    `> ${DESCRIPTION}`,
    "",
    "Rune is a Rust UI framework. Views are plain Rust functions of state; Rune diffs them, lays them out with Taffy,",
    "animates layout and properties with springs, and draws every frame with one instanced GPU draw. It runs on macOS",
    "(its own Metal backend), Windows, Linux, iOS (embedded in UIKit or SwiftUI) and the web (WebGPU, experimental).",
    "",
    "## Source",
    "",
    `- [anomalyco/rune](${REPO}): the code`,
    `- [crates/rune/README.md](${REPO}/blob/dev/crates/rune/README.md): the framework guide`,
    `- [WEB.md](${REPO}/blob/dev/WEB.md): building a Rune app for the browser`,
    `- [CHANGELOG.md](${REPO}/blob/dev/CHANGELOG.md): what changed, newest first`,
    "",
    "## Use it",
    "",
    "Add Rune to Cargo.toml pinned to a commit:",
    "",
    "```toml",
    `rune = { git = "${REPO}", rev = "<commit>" }`,
    "```",
    "",
  ].join("\n"),
)

rmSync(ssrDir, { recursive: true, force: true })
console.log(`Prerendered ${production ? "production" : "dev"} into ${distRel}/`)
