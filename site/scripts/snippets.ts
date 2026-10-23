/** Code examples on the site, cut from Rune's own source at build time and highlighted there.
 *
 *  Each snippet names files in this repo and where to cut. A piece starts at the first line beginning with `from`
 *  (attributes above it come along) and ends at `to`, just before `until`, after `lines` lines, or at the closing
 *  brace at the starting line's indent. A missing file or marker fails the build, so an example can't drift away from the code.
 *  The Vite plugin serves the result as `virtual:snippets`; the client gets HTML strings, not the highlighter. */
import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import type { Plugin } from "vite"
import { highlight, type CodeLang } from "./highlight"

type Piece = { file: string; from?: string; to?: string; until?: string; lines?: number }
type Spec = { lang: CodeLang; label: string; pieces: Piece[] }

export type Snippet = { html: string; label: string; file: string; start: number; end: number }

const SETTINGS = "examples/settings/src/main.rs"
const HELLO = "examples/hello/src/main.rs"
const TODO = "examples/todo/src/main.rs"

const specs: Record<string, Spec> = {
  hello: {
    lang: "rust",
    label: "Rust",
    pieces: [{ file: HELLO, from: "use rune::", until: "#[cfg(test)]" }],
  },
  "hello-test": {
    lang: "rust",
    label: "Rust",
    pieces: [{ file: HELLO, from: "    fn a_tap_flips_the_switch" }],
  },
  "todo-list": {
    lang: "rust",
    label: "Rust",
    pieces: [{ file: TODO, from: "fn view(" }],
  },
  "todo-spring": {
    lang: "rust",
    label: "Rust",
    pieces: [{ file: TODO, from: "fn progress(" }],
  },
  "overlay-windows": {
    lang: "rust",
    label: "Rust",
    pieces: [{ file: "examples/overlay/src/main.rs", from: "    rune::run_windows(", to: "    });" }],
  },
  "app-cargo": {
    lang: "toml",
    label: "TOML",
    pieces: [{ file: "examples/settings/Cargo.toml" }],
  },
  "app-main": {
    lang: "rust",
    label: "Rust",
    pieces: [
      { file: SETTINGS, from: "fn view(", lines: 23 },
      { file: SETTINGS, from: "fn main() {" },
    ],
  },
  toggle: {
    lang: "rust",
    label: "Rust",
    pieces: [
      { file: SETTINGS, from: '                        toggle_row(p, "login"', lines: 3 },
      { file: SETTINGS, from: "fn toggle_row(" },
      { file: SETTINGS, from: "fn toggle(" },
    ],
  },
  "toggle-only": {
    lang: "rust",
    label: "Rust",
    pieces: [{ file: SETTINGS, from: "fn toggle(" }],
  },
  "motion-tokens": {
    lang: "rust",
    label: "Rust",
    pieces: [
      { file: "crates/rune-motion/src/tokens.rs", from: "/// Small controls", lines: 3 },
      { file: "crates/rune-motion/src/tokens.rs", from: "pub const SNAPPY", lines: 1 },
    ],
  },
  retarget: {
    lang: "rust",
    label: "Rust",
    pieces: [
      {
        file: "crates/rune-motion/tests/spring.rs",
        from: "fn animated_value_retargets_keeping_velocity",
        to: "    assert!((a.sample(5.0) + 50.0)",
      },
    ],
  },
  live: {
    lang: "rust",
    label: "Rust",
    pieces: [
      { file: "crates/rune/tests/live.rs", from: "fn view(" },
      { file: "crates/rune/tests/live.rs", from: "fn properties_change_without_view", to: "    assert_eq!(rt.stats.shaped, 0);" },
    ],
  },
  "test-segmented": {
    lang: "rust",
    label: "Rust",
    pieces: [
      { file: "crates/rune-kit/tests/segmented.rs", from: "fn fitted(" },
      { file: "crates/rune-kit/tests/segmented.rs", from: "fn fitted_segments_share" },
    ],
  },
  "snap-commands": {
    lang: "shell",
    label: "Shell",
    pieces: [{ file: "examples/live-values/README.md", from: "target/release/live-values --snap", lines: 2 }],
  },
  "tap-script": {
    lang: "ts",
    label: "JSON",
    pieces: [{ file: "examples/settings/scripts/toggle-retarget.json" }],
  },
  "golden-hashes": {
    lang: "rust",
    label: "Rust",
    pieces: [{ file: "crates/rune-ref/tests/reference.rs", from: "fn canonical_hashes" }],
  },
  "rune-deps": {
    lang: "toml",
    label: "TOML",
    pieces: [{ file: "crates/rune/Cargo.toml", from: "rune-motion = ", to: "parley = " }],
  },
}

const indentOf = (line: string) => line.match(/^\s*/)![0]

function cut(root: string, id: string, piece: Piece): { text: string; start: number; end: number } {
  const where = `snippet "${id}": ${piece.file}`
  let source: string
  try {
    source = readFileSync(resolve(root, piece.file), "utf8")
  } catch {
    throw new Error(`${where} not found`)
  }
  const lines = source.replace(/\n$/, "").split("\n")
  if (!piece.from) return { text: lines.join("\n"), start: 1, end: lines.length }

  const first = lines.findIndex(l => l.startsWith(piece.from!))
  if (first < 0) throw new Error(`${where}: no line starts with ${JSON.stringify(piece.from)}`)
  let start = first
  while (start > 0 && lines[start - 1]!.trim().startsWith("#[")) start--

  let end: number
  if (piece.lines) {
    end = first + piece.lines - 1
  } else if (piece.until) {
    end = lines.findIndex((l, i) => i > first && l.startsWith(piece.until!)) - 1
    if (end < 0) throw new Error(`${where}: no line starts with ${JSON.stringify(piece.until)} after ${JSON.stringify(piece.from)}`)
    while (end > first && !lines[end]!.trim()) end--
  } else if (piece.to) {
    end = lines.findIndex((l, i) => i >= first && l.startsWith(piece.to!))
    if (end < 0) throw new Error(`${where}: no line starts with ${JSON.stringify(piece.to)} after ${JSON.stringify(piece.from)}`)
  } else {
    const close = `${indentOf(lines[first]!)}}`
    end = lines.findIndex((l, i) => i > first && l.trimEnd() === close)
    if (end < 0) throw new Error(`${where}: no closing brace for ${JSON.stringify(piece.from)}`)
  }

  const taken = lines.slice(start, end + 1)
  const pad = Math.min(...taken.filter(l => l.trim()).map(l => indentOf(l).length))
  const out = taken.map(l => l.slice(pad))
  // A block cut short keeps its shape: an elision inside, then its closing brace.
  const open = (out.join("\n").match(/\{/g)?.length ?? 0) - (out.join("\n").match(/\}/g)?.length ?? 0)
  if (open > 0) {
    const indent = indentOf(lines[first]!).slice(pad)
    out.push(`${indent}    // …`, `${indent}}`)
  }
  return { text: out.join("\n"), start: start + 1, end: end + 1 }
}

export function buildSnippets(root: string): Record<string, Snippet> {
  const out: Record<string, Snippet> = {}
  for (const [id, spec] of Object.entries(specs)) {
    const parts = spec.pieces.map(p => cut(root, id, p))
    const gap = spec.lang === "rust" || spec.lang === "ts" ? "\n\n// …\n\n" : "\n\n# …\n\n"
    const code = parts.map(p => p.text).join(gap)
    // One cut links to its lines; several cuts or a whole file link to the file.
    const one = spec.pieces.length === 1 && spec.pieces[0]!.from ? parts[0]! : undefined
    out[id] = {
      html: highlight(code, spec.lang),
      label: spec.label,
      file: spec.pieces[0]!.file,
      start: one?.start ?? 0,
      end: one?.end ?? 0,
    }
  }
  return out
}

const ID = "virtual:snippets"

/** Serves the snippets as a module and reloads the page when a cited file changes in dev. */
export function snippets(root: string): Plugin {
  const files = new Set(Object.values(specs).flatMap(s => s.pieces.map(p => resolve(root, p.file))))
  return {
    name: "rune-snippets",
    resolveId: id => (id === ID ? `\0${ID}` : undefined),
    load(id) {
      if (id !== `\0${ID}`) return
      for (const f of files) this.addWatchFile(f)
      return `export default ${JSON.stringify(buildSnippets(root))}`
    },
    handleHotUpdate({ file, server }) {
      if (!files.has(file)) return
      const mod = server.moduleGraph.getModuleById(`\0${ID}`)
      if (mod) server.moduleGraph.invalidateModule(mod)
      server.ws.send({ type: "full-reload" })
    },
  }
}
