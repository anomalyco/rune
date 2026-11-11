import { bindings, defineConfig, triggers } from "cf/config"
import { BASE_PATH, hostForMode } from "./src/base.ts"

// `--mode dev` builds and deploys the dev Worker on dev.anoma.ly; any other mode is production on anoma.ly.
// The account comes from CLOUDFLARE_ACCOUNT_ID.
export default defineConfig(({ mode }) => {
  const host = hostForMode(mode)
  return {
    worker: {
      name: mode === "dev" ? "rune-www-dev" : "rune-www-production",
      compatibilityDate: "2026-10-13",
      entrypoint: "./worker.ts",
      assets: { runWorkerFirst: true },
      env: {
        ASSETS: bindings.assets(),
        CANONICAL_HOST: bindings.text(host),
      },
      triggers: [
        triggers.fetch({ pattern: `${host}${BASE_PATH}`, zone: host }),
        triggers.fetch({ pattern: `${host}${BASE_PATH}/*`, zone: host }),
      ],
      workersDev: false,
      previewUrls: false,
    },
  }
})
