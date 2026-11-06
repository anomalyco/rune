#!/usr/bin/env bun
// `bun run images`: cuts the page's images from Rune's own renders in demo/ into AVIF and WebP at the sizes the page
// shows them, into public/img/, and writes src/images.json with each image's size so every <img> is sized up front.
// Needs ImageMagick 7 with AVIF and WebP. The outputs are committed, so a build never needs it.
import { mkdirSync, rmSync, statSync, writeFileSync } from "node:fs"
import { join, resolve } from "node:path"
import { $ } from "bun"

// Gallery tiles are 16:9, so crop renders of other shapes to 16:9 here. `smooth` is for soft gradients on black, which
// band in 8-bit: it encodes 10-bit AVIF at a higher quality.
type Spec = { src: string; crop?: [number, number, number, number]; widths: number[]; smooth?: boolean }

const repo = resolve(import.meta.dirname, "..", "..")
const www = resolve(import.meta.dirname, "..")
const out = join(www, "public", "img")

export const specs: Record<string, Spec> = {
  hello: { src: "site/public/shots/hello-on.png", widths: [320, 640] },
  "opencode-session": { src: "demo/opencode/session-top.png", widths: [720, 1120, 1680, 2240] },
  "opencode-home": { src: "demo/opencode/home.png", widths: [560, 1100] },
  "opencode-streaming": { src: "demo/opencode/streaming.png", widths: [560, 1100] },
  // `cargo run --release -p site-scenes` renders these into target/; they aren't committed.
  metal: { src: "target/site-scenes/metal.png", widths: [760, 1520], smooth: true },
  glass: { src: "target/site-scenes/glass.png", crop: [240, 135, 1440, 810], widths: [380, 760], smooth: true },
  light: { src: "target/site-scenes/light.png", crop: [240, 135, 1440, 810], widths: [380, 760], smooth: true },
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
    const formats = spec.smooth ? ([["avif", 70, 10], ["webp", 90, 8]] as const) : ([["avif", 52, 8], ["webp", 80, 8]] as const)
    for (const [ext, quality, depth] of formats) {
      const file = join(out, `${name}-${w}.${ext}`)
      await $`magick ${src} ${crop} -resize ${w}x -strip -depth ${depth} -quality ${quality} ${file}`.quiet()
      bytes += statSync(file).size
    }
  }
  manifest[name] = { width: w0!, height: h0!, widths }
}
writeFileSync(join(www, "src", "images.json"), `${JSON.stringify(manifest, null, 2)}\n`)
console.log(`${Object.keys(specs).length} images, ${(bytes / 1024).toFixed(0)} KB → public/img/`)
