import { cloudflare } from "@cloudflare/vite-plugin"
import { defineConfig } from "vite"
import { BASE_PATH } from "./src/base"

// Vite only bundles src/main.ts with the wasm Rune builds into src/wasm/, and the Cloudflare plugin shapes the result
// into Build Output for `cf deploy`. scripts/build.ts then writes the page into index.html.
export default defineConfig({
  plugins: [cloudflare()],
  base: `${BASE_PATH}/`,
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsInlineLimit: 0,
    sourcemap: true,
  },
  preview: {
    host: "127.0.0.1",
    port: 4253,
    strictPort: true,
    allowedHosts: [".ts.net"],
  },
})
