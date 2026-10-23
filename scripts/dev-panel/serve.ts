#!/usr/bin/env bun
// Socket/page glue only. Every control and the page itself belong to @kitlangton/knobs.
import { resolve } from "node:path"

const port = Number(process.env.PORT ?? 4794)
const upstream = process.env.RUNE_DEV_WS ?? "ws://127.0.0.1:4793"
const bundle = await Bun.build({
  entrypoints: [resolve(import.meta.dir, "index.ts")],
  target: "browser",
  conditions: ["development"],
  external: ["@pierre/diffs"],
})
if (!bundle.success) throw new AggregateError(bundle.logs, "Could not bundle the knobs remote page")
const js = await bundle.outputs[0]!.text()
const server = Bun.serve<{ upstream?: WebSocket }>({
  hostname: "127.0.0.1", port,
  fetch(request, server) {
    const url = new URL(request.url)
    if (url.pathname === "/ws") {
      // Proxy only this configured Rune runtime; clients never select an upstream address.
      if (server.upgrade(request, { data: {} })) return
      return new Response("Upgrade required", { status: 426 })
    }
    if (url.pathname === "/panel.js") return new Response(js, { headers: { "Content-Type": "text/javascript" } })
    if (url.pathname !== "/") return new Response("Not found", { status: 404 })
    return new Response(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Rune</title></head><body><script type="module" src="/panel.js"></script></body></html>`, { headers: { "Content-Type": "text/html" } })
  },
  websocket: {
    maxPayloadLength: 64 * 1024,
    open(client) {
      const peer = new WebSocket(upstream)
      client.data.upstream = peer
      peer.addEventListener("message", event => client.send(String(event.data)))
      peer.addEventListener("close", () => client.close())
      peer.addEventListener("error", () => client.close())
    },
    message(client, message) {
      const peer = client.data.upstream
      if (peer?.readyState === WebSocket.OPEN) peer.send(message)
    },
    close(client) { client.data.upstream?.close() },
  },
})
console.log(`Rune knobs page: ${server.url} → ${upstream}`)
