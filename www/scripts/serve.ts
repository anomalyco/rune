#!/usr/bin/env bun
// `bun run serve [--port 4253]`: serves the built site through worker.ts, with a stand-in for the ASSETS binding that
// reads the build output, so the base path, 404 page and headers are the Worker's own. Run `bun run build` first.
import { existsSync, statSync } from "node:fs"
import { extname, join, resolve } from "node:path"
import { parseArgs } from "node:util"
import { brotliCompressSync, constants } from "node:zlib"
import { getWorkerAssetsDir } from "@cloudflare/build-output-utils"
import worker from "../worker"

const { values } = parseArgs({ options: { port: { type: "string", default: "4253" }, host: { type: "string", default: "127.0.0.1" } } })
const dir = getWorkerAssetsDir(resolve(import.meta.dirname, ".."))
if (!existsSync(join(dir, "index.html"))) throw new Error(`${dir}/index.html not found; run \`bun run build\` first`)

const file = (p: string) => existsSync(p) && statSync(p).isFile() ? p : undefined

// Static assets as Cloudflare serves them with html_handling "auto-trailing-slash": /x finds x.html or x/index.html.
const ASSETS = {
  async fetch(request: Request) {
    const path = decodeURIComponent(new URL(request.url).pathname)
    const base = join(dir, path)
    const hit = path.endsWith("/") ? file(join(base, "index.html")) : file(base) ?? file(`${base}.html`)
    if (!hit) return new Response("Not found", { status: 404 })
    const html = extname(hit) === ".html"
    return new Response(Bun.file(hit), { headers: html ? { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-cache" } : {} })
  },
}

// Cloudflare compresses text and wasm on the way out; do the same so timings here are close to the real thing.
const squeezable = /^(text\/|application\/(javascript|json|wasm|manifest\+json)|image\/(svg|x-icon)|font\/(ttf|otf))/
const squeezed = new Map<string, Buffer>()

const server = Bun.serve({
  hostname: values.host,
  port: Number(values.port),
  async fetch(request) {
    const response = await worker.fetch(request, { ASSETS, CANONICAL_HOST: "localhost", ANY_HOST: "1" })
    const type = response.headers.get("Content-Type") ?? ""
    if (!response.ok || !squeezable.test(type) || !request.headers.get("Accept-Encoding")?.includes("br")) return response
    const key = new URL(request.url).pathname
    let body = squeezed.get(key)
    if (!body) {
      body = brotliCompressSync(new Uint8Array(await response.arrayBuffer()), { params: { [constants.BROTLI_PARAM_QUALITY]: 5 } })
      squeezed.set(key, body)
    }
    const headers = new Headers(response.headers)
    headers.set("Content-Encoding", "br")
    headers.delete("Content-Length")
    return new Response(new Uint8Array(body), { status: response.status, headers })
  },
})
console.log(`rune-www on ${server.url}rune/`)
