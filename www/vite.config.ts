import { cloudflare } from "@cloudflare/vite-plugin"
import react from "@vitejs/plugin-react"
import { execSync } from "node:child_process"
import { resolve } from "node:path"
import { defineConfig } from "vite"
import { snippets } from "./scripts/snippets"
import { BASE_PATH } from "./src/base"

// The commit the page tells you to pin: the one it was built from, which is on dev.
const rev = (() => {
  try {
    return execSync("git rev-parse --short=12 HEAD", { encoding: "utf8" }).trim()
  } catch {
    return "<commit>"
  }
})()

// The Cloudflare plugin only shapes the client build into Build Output for `cf deploy`.
// Dev stays plain Vite, and the SSR build only feeds the prerender.
export default defineConfig(({ command, isSsrBuild }) => ({
  plugins: [react(), snippets(resolve(import.meta.dirname, "..")), ...(command === "build" && !isSsrBuild ? [cloudflare()] : [])],
  base: `${BASE_PATH}/`,
  define: { __RUNE_REV__: JSON.stringify(rev) },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsInlineLimit: 0,
  },
  server: {
    host: "127.0.0.1",
    port: 4253,
    strictPort: true,
    allowedHosts: [".ts.net"],
  },
  preview: {
    host: "127.0.0.1",
    port: 4253,
    strictPort: true,
    allowedHosts: [".ts.net"],
  },
}))
