export type Route = { path: string; label: string; title: string; description: string }

export const REPO = "https://github.com/anomalyco/rune"

export const routes: Route[] = [
  {
    path: "/",
    label: "Overview",
    title: "Rune · Rust UI framework",
    description:
      "Rune is a Rust UI framework with its own GPU renderer, text, layout, spring animation and thin per-platform shells, written and checked by agents.",
  },
  {
    path: "/how-it-works",
    label: "How it works",
    title: "How it works · Rune",
    description:
      "How a Rune frame runs, from input and state through Taffy layout and springs to wgpu, with real code, the thread model, headless tests and the crate map.",
  },
  {
    path: "/launch-fast",
    label: "Launch fast",
    title: "Launch fast · Rune",
    description:
      "How to get a Rune app's first window on screen in about 300 ms: prewarm_gpu, precompiled Metal shaders, a first frame at creation, deferred setup, a still first second, an idle window that draws nothing, a reused hidden window, and how to measure it.",
  },
  {
    path: "/fast-apps",
    label: "Fast apps",
    title: "Fast apps · Rune",
    description:
      "How to keep a Rune app fast: a free main thread, long lists that build only what is on screen, memo keyed small, an idle window that draws nothing, cheap poll, bounded caches, and a script that audits a repo.",
  },
  {
    path: "/principles",
    label: "Principles",
    title: "Principles · Rune",
    description: "What Rune optimises for and the rules it holds itself to.",
  },
  {
    path: "/architecture",
    label: "Architecture",
    title: "Architecture · Rune",
    description: "How Rune is built: the app side, the command channel, the render thread, the renderer and the platform shells.",
  },
  {
    path: "/roadmap",
    label: "Roadmap",
    title: "Roadmap · Rune",
    description: "Rune's milestones, from the first spikes to OpenCode Desktop, the web and iOS.",
  },
]

export function normalizePath(path: string): string {
  const trimmed = path.replace(/\/index\.html$/, "").replace(/\/+$/, "")
  return trimmed === "" ? "/" : trimmed
}
