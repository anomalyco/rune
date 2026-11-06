#!/usr/bin/env bun
// `bun run build [--mode dev]`: typecheck, client build (Cloudflare Build Output), SSR build, then prerender the page
// to static HTML.
import { existsSync } from "node:fs"
import { join } from "node:path"
import { parseArgs } from "node:util"
import { $ } from "bun"

const { values } = parseArgs({ options: { mode: { type: "string", default: "production" } } })
const mode = values.mode

if (!existsSync(join(import.meta.dirname, "..", "public", "live", "app_bg.wasm")))
  console.warn("public/live/ isn't built, so the hero keeps its poster. See \"The live panel\" in README.md.")

await $`tsc --noEmit`
await $`vite build --mode ${mode}`
await $`vite build --mode ${mode} --ssr src/entry-server.tsx --outDir dist-ssr`
await $`bun scripts/prerender.ts --mode ${mode}`
