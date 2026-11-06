// ᚱUNE on Anomaly's pixel grid: letters 11 pixels tall, 2-pixel stems, notched corners, fitted one clear pixel apart.
// The R is the rune ᚱ (raidō), so the name is spelled with a real rune and still reads as RUNE. Plain data and string
// builders, so the site and the icon script draw the same pixels.

export type Grid = readonly string[]
const grid = (s: string): Grid => s.trim().split("\n").map(row => row.trim())

const GLYPHS: Record<string, Grid> = {
  // The stave, a pointed bowl and a straight leg, all on 45° steps two pixels thick.
  ᚱ: grid(`
###....
####...
##.##..
##..##.
##..##.
##.##..
####...
##.##..
##..##.
##...##
##...##`),
  U: grid(`
##...##
##...##
##...##
##...##
##...##
##...##
##...##
##...##
##...##
#######
.#####.`),
  N: grid(`
###...##
####..##
####..##
####..##
##.##.##
##.##.##
##.##.##
##..####
##..####
##..####
##...###`),
  E: grid(`
######
######
##....
##....
#####.
#####.
##....
##....
##....
######
######`),
}

export const CAP = 11

/** The ᚱ redrawn for 16 and 32 px favicons: 3-pixel stems, one or two device pixels per cell. */
export const ICON16: Grid = grid(`
................
...####.........
...#####........
...######.......
...###.###......
...###..###.....
...###..###.....
...###.###......
...######.......
...#####........
...######.......
...###.###......
...###..###.....
...###...###....
...###....###...
................`)

/** A pixel rectangle: x, y, width, height. */
export type Rect = readonly [number, number, number, number]
export interface Glyph { char: string; x: number; rows: Grid }

const ink = (rows: Grid) => rows.flatMap((row, y) => [...row].flatMap((c, x) => (c === "#" ? [[x, y] as const] : [])))

/** Where `next` starts after `prev` (placed at `at`): as far left as keeps one clear pixel between their ink in every
 *  direction, diagonals included. */
function fit(prev: Grid, at: number, next: Grid) {
  const a = ink(prev).map(([x, y]) => [x + at, y] as const), b = ink(next)
  let x = at + 1
  while (a.some(([ax, ay]) => b.some(([bx, by]) => Math.abs(bx + x - ax) <= 1 && Math.abs(by - ay) <= 1))) x++
  return x
}

export const WORDMARK = (() => {
  const glyphs: Glyph[] = []
  for (const char of "ᚱUNE") {
    const rows = GLYPHS[char]!
    const last = glyphs[glyphs.length - 1]
    glyphs.push({ char, x: last ? fit(last.rows, last.x, rows) : 0, rows })
  }
  const last = glyphs[glyphs.length - 1]!
  return { glyphs, width: last.x + last.rows[0]!.length, height: CAP }
})()

/** Rows to rectangles, merging each row's runs so no seams show between pixels. */
export function runs(rows: Grid, dx = 0, dy = 0): Rect[] {
  const out: Rect[] = []
  rows.forEach((row, y) => {
    for (let x = 0; x < row.length; x++) {
      if (row[x] !== "#") continue
      let end = x
      while (row[end] === "#") end++
      out.push([dx + x, dy + y, end - x, 1])
      x = end
    }
  })
  return out
}

/** ᚱ's carve order: the stave top to bottom, then the bowl out and back, then the leg. Each cell gets the arc length
 *  (0–1) of its nearest point on that path. */
export function carveOrder(rows: Grid): [number, number, number][] {
  const path = [[[1, .5], [1, 10.5]], [[1.5, .5], [5, 3.5]], [[5, 3.5], [1.5, 6.5]], [[2.5, 6.5], [6, 10.5]]] as const
  const lens = path.map(([a, b]) => Math.hypot(b[0] - a[0], b[1] - a[1]))
  const total = lens.reduce((s, l) => s + l, 0)
  return ink(rows).map(([x, y]) => {
    const c = [x + .5, y + .5] as const
    let best = Infinity, arc = 0, acc = 0
    path.forEach(([a, b], i) => {
      const dx = b[0] - a[0], dy = b[1] - a[1]
      const p = Math.max(0, Math.min(1, ((c[0] - a[0]) * dx + (c[1] - a[1]) * dy) / (dx * dx + dy * dy)))
      const d = Math.hypot(a[0] + dx * p - c[0], a[1] + dy * p - c[1])
      if (d < best - 1e-6) { best = d; arc = acc + p * lens[i]! }
      acc += lens[i]!
    })
    return [x, y, arc / total]
  })
}

/** OKLCH to gamma-encoded sRGB, 0–1 per channel. */
export function oklch(l: number, c: number, hDeg: number): [number, number, number] {
  const h = (hDeg * Math.PI) / 180, a = c * Math.cos(h), b = c * Math.sin(h)
  const l_ = (l + .3963377774 * a + .2158037573 * b) ** 3
  const m_ = (l - .1055613458 * a - .0638541728 * b) ** 3
  const s_ = (l - .0894841775 * a - 1.291485548 * b) ** 3
  const lin = [
    4.0767416621 * l_ - 3.3077115913 * m_ + .2309699292 * s_,
    -1.2684380046 * l_ + 2.6097574011 * m_ - .3413193965 * s_,
    -.0041960863 * l_ - .7034186147 * m_ + 1.707614701 * s_,
  ]
  const gamma = (x: number) => { x = Math.min(1, Math.max(0, x)); return x <= .0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - .055 }
  return [gamma(lin[0]!), gamma(lin[1]!), gamma(lin[2]!)]
}

/** Rune's ink, `--ink` in site.css, and the light the carve runs. */
export const INK: [number, number, number] = [.74, .14, 245]
export const HOT: [number, number, number] = [.97, .035, 245]
