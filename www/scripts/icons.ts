#!/usr/bin/env bun
// `bun scripts/icons.ts`: draws the brand into public/: favicon.svg and favicon.ico (16 and 32) from the 16-cell ᚱ on
// transparent; apple-touch-icon.png (180), icon-192.png and icon-512.png from the wordmark's ᚱ on black; and og.png
// (1200 × 630), the whole wordmark on black. Every cell is a whole number of pixels, so the PNGs are written directly,
// with no browser.
import { writeFileSync } from "node:fs"
import { join, resolve } from "node:path"
import { deflateSync } from "node:zlib"
import { ICON16, INK, WORDMARK, oklch, runs, type Grid } from "../src/brand/pixels"

const out = resolve(import.meta.dirname, "..", "public")
const ink = oklch(...INK).map(c => Math.round(c * 255)) as [number, number, number]
const hex = `#${ink.map(c => c.toString(16).padStart(2, "0")).join("")}`

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c >>> 0
})
const crc = (bytes: Uint8Array) => {
  let c = 0xffffffff
  for (const b of bytes) c = crcTable[(c ^ b) & 0xff]! ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}
function chunk(type: string, data: Uint8Array) {
  const body = new Uint8Array(4 + data.length)
  body.set(new TextEncoder().encode(type))
  body.set(data, 4)
  const out = new Uint8Array(12 + data.length)
  const dv = new DataView(out.buffer)
  dv.setUint32(0, data.length)
  out.set(body, 4)
  dv.setUint32(8 + data.length, crc(body))
  return out
}

/** `rows` at `px` pixels a cell, centred in a `width` × `height` image on `background` (null for transparent), as an
 *  RGBA PNG. */
function png(rows: Grid, px: number, background: [number, number, number] | null, width: number, height = width) {
  const w = rows[0]!.length * px, h = rows.length * px
  const ox = Math.floor((width - w) / 2), oy = Math.floor((height - h) / 2)
  const raw = new Uint8Array(height * (width * 4 + 1))
  for (let y = 0; y < height; y++) {
    const row = y * (width * 4 + 1)
    for (let x = 0; x < width; x++) {
      const cx = Math.floor((x - ox) / px), cy = Math.floor((y - oy) / px)
      const on = x >= ox && y >= oy && rows[cy]?.[cx] === "#"
      const rgb = on ? ink : background
      if (rgb) raw.set([...rgb, 255], row + 1 + x * 4)
    }
  }
  const header = new Uint8Array(13)
  const dv = new DataView(header.buffer)
  dv.setUint32(0, width)
  dv.setUint32(4, height)
  header.set([8, 6, 0, 0, 0], 8)
  const parts = [new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", new Uint8Array())]
  return Buffer.concat(parts)
}

/** An ICO holding PNGs, one entry per size. */
function ico(images: { png: Uint8Array; size: number }[]) {
  const header = 6 + 16 * images.length
  const out = new Uint8Array(header + images.reduce((n, i) => n + i.png.length, 0))
  const dv = new DataView(out.buffer)
  dv.setUint16(2, 1, true)
  dv.setUint16(4, images.length, true)
  let offset = header
  images.forEach(({ png, size }, k) => {
    const e = 6 + 16 * k
    out[e] = size
    out[e + 1] = size
    dv.setUint16(e + 4, 1, true)
    dv.setUint16(e + 6, 32, true)
    dv.setUint32(e + 8, png.length, true)
    dv.setUint32(e + 12, offset, true)
    out.set(png, offset)
    offset += png.length
  })
  return out
}

const rune = WORDMARK.glyphs[0]!.rows
const black: [number, number, number] = [0, 0, 0]
for (const [name, size, px] of [["icon-512.png", 512, 32], ["icon-192.png", 192, 12], ["apple-touch-icon.png", 180, 12]] as const) {
  writeFileSync(join(out, name), png(rune, px, black, size))
}
writeFileSync(join(out, "favicon.ico"), ico([32, 16].map(size => ({ size, png: png(ICON16, size / 16, null, size) }))))
const word = Array.from({ length: WORDMARK.height }, (_, y) =>
  Array.from({ length: WORDMARK.width }, (_, x) => (WORDMARK.glyphs.some(g => g.rows[y]?.[x - g.x] === "#") ? "#" : ".")).join(""))
writeFileSync(join(out, "og.png"), png(word, 24, black, 1200, 630))
const rects = runs(ICON16).map(([x, y, w, h]) => `<rect x="${x}" y="${y}" width="${w}" height="${h}"/>`).join("")
writeFileSync(join(out, "favicon.svg"), `<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" shape-rendering="crispEdges" fill="${hex}">${rects}</svg>\n`)
console.log(`Icons in ${ink.join(",")} (${hex}) → ${out}`)
