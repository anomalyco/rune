#!/usr/bin/env bun
// `bun run build [--mode dev]`: typecheck, client build (Cloudflare Build Output), SSR build, then prerender every
// route to static HTML.
import { parseArgs } from "node:util"
import { $ } from "bun"

const { values } = parseArgs({ options: { mode: { type: "string", default: "production" } } })
const mode = values.mode

await $`tsc --noEmit`
await $`vite build --mode ${mode}`
await $`vite build --mode ${mode} --ssr src/entry-server.tsx --outDir dist-ssr`
await $`bun scripts/prerender.ts --mode ${mode}`
