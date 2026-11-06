#!/usr/bin/env bun
// `bun run images`: cuts the page's images from Rune's own renders in demo/ into AVIF and WebP at the sizes the page
// shows them, into public/img/, and writes src/images.json with each image's size so every <img> is sized up front.
// Needs ImageMagick 7 with AVIF and WebP. The outputs are committed, so a build never needs it.
import { mkdirSync, rmSync, statSync, writeFileSync } from "node:fs"
import { join, resolve } from "node:path"
import { $ } from "bun"

// Gallery tiles are 16:9, so crop renders of other shapes to 16:9 here.
type Spec = { src: string; crop?: [number, number, number, number]; widths: number[] }

const repo = resolve(import.meta.dirname, "..", "..")
const www = resolve(import.meta.dirname, "..")
const out = join(www, "public", "img")

export const specs: Record<string, Spec> = {
  hello: { src: "site/public/shots/hello-on.png", widths: [320, 640] },
  "opencode-session": { src: "demo/opencode/session-top.png", widths: [720, 1120, 1680, 2240] },
  "opencode-home": { src: "demo/opencode/home.png", widths: [560, 1100] },
  "opencode-streaming": { src: "demo/opencode/streaming.png", widths: [560, 1100] },
  glow: { src: "demo/fx/glow.png", widths: [560, 1120] },
  "liquid-metal": { src: "demo/fx/liquid-metal.png", widths: [560, 1120] },
  "fluted-glass": { src: "demo/fx/fluted-glass.png", widths: [560, 1120] },
  "raised-light": { src: "demo/light/raised.png", crop: [300, 360, 1960, 1102], widths: [560, 1120] },
  electric: { src: "demo/fx/electric.png", widths: [560, 1120] },
  "rust-diff": { src: "demo/kit/diff-split-dark.png", crop: [0, 0, 1800, 1012], widths: [560, 1120] },
}

const manifest: Record<string, { width: number; height: number; widths: number[] }> = {}
rmSync(out, { recursive: true, force: true })
mkdirSync(out, { recursive: true })
let bytes = 0
for (const [name, spec] of Object.entries(specs)) {
  const src = join(repo, spec.src)
  const crop = spec.crop ? ["-crop", `${spec.crop[2]}x${spec.crop[3]}+${spec.crop[0]}+${spec.crop[1]}`, "+repage"] : []
  const [w0, h0] = spec.crop ? [spec.crop[2], spec.crop[3]] : (await $`magick identify -format "%w %h" ${src}`.text()).split(" ").map(Number)
  const widths = spec.widths.filter(w => w <= w0!)
  for (const w of widths) {
    for (const [ext, quality] of [["avif", 52], ["webp", 80]] as const) {
      const file = join(out, `${name}-${w}.${ext}`)
      await $`magick ${src} ${crop} -resize ${w}x -strip -quality ${quality} ${file}`.quiet()
      bytes += statSync(file).size
    }
  }
  manifest[name] = { width: w0!, height: h0!, widths }
}
writeFileSync(join(www, "src", "images.json"), `${JSON.stringify(manifest, null, 2)}\n`)
console.log(`${Object.keys(specs).length} images, ${(bytes / 1024).toFixed(0)} KB → public/img/`)
