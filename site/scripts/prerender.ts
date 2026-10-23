import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join, relative, resolve } from "node:path"
import { parseArgs } from "node:util"
import { getWorkerAssetsDir } from "@cloudflare/build-output-utils"
import { BASE_PATH, hostForMode } from "../src/base"
import { REPO, type Route } from "../src/routes"

const { values } = parseArgs({ options: { mode: { type: "string", default: "production" } } })
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

const ssr = (await import(join(ssrDir, "entry-server.js"))) as {
  render: (pathname: string) => string
  routes: Route[]
}

const page = (route: Route) =>
  template
    .replace(/<title>[\s\S]*?<\/title>/, `<title>${escape(route.title)}</title>`)
    .replace(/<meta name="description" content="[^"]*" \/>/, `<meta name="description" content="${escape(route.description)}" />`)
    .replaceAll("__URL__", `${origin}${route.path}`)
    .replaceAll("__TITLE__", escape(route.title))
    .replaceAll("__DESCRIPTION__", escape(route.description))
    .replaceAll("__ORIGIN__", origin)
    .replace("<!--app-->", ssr.render(route.path))

// The Worker serves this for any missing path under the base, with a 404.
const notFound = page({ path: "/404", label: "Not found", title: "Not found · Rune", description: "There's no page here." })
writeFileSync(join(distDir, "404.html"), notFound.replace(/\s*<link rel="canonical"[^>]*>/, ""))

// Every link into the repo names a file or folder that exists, so code references can't drift.
const repoRoot = resolve(root, "..")
const missing = new Set<string>()
const checkRepoLinks = (html: string) => {
  for (const [, path] of html.matchAll(new RegExp(`${REPO}/(?:blob|tree)/dev/([^"#?]+)`, "g"))) {
    if (!existsSync(join(repoRoot, decodeURIComponent(path!)))) missing.add(path!)
  }
}

for (const route of ssr.routes) {
  const html = page(route)
  checkRepoLinks(html)
  if (route.path === "/") {
    writeFileSync(templatePath, html)
  } else {
    // Both files, so /principles serves directly instead of redirecting to /principles/.
    const dir = join(distDir, route.path.slice(1))
    mkdirSync(dir, { recursive: true })
    writeFileSync(join(dir, "index.html"), html)
    writeFileSync(`${dir}.html`, html)
  }
}

if (missing.size) throw new Error(`Links to files that aren't in the repo:\n  ${[...missing].join("\n  ")}`)

// llms.txt (llmstxt.org): what the site is and where each page lives, from the same routes.
const home = ssr.routes.find(r => r.path === "/")!
writeFileSync(
  join(distDir, "llms.txt"),
  [
    "# Rune",
    "",
    `> ${home.description}`,
    "",
    "## Pages",
    "",
    ...ssr.routes.map(r => `- [${r.label}](${origin}${r.path}): ${r.description}`),
    "",
    "## Source",
    "",
    `- [anomalyco/rune](${REPO}): the code`,
    `- [PLAN.md](${REPO}/blob/dev/PLAN.md): the full plan, the source of truth for this site`,
    "",
  ].join("\n"),
)

rmSync(ssrDir, { recursive: true, force: true })
console.log(`Prerendered ${ssr.routes.length} routes into ${distRel}/`)
