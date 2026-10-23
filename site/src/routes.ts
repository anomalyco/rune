export type Route = { path: string; label: string; title: string; description: string }

export const REPO = "https://github.com/anomalyco/rune"

export const routes: Route[] = [
  {
    path: "/",
    label: "Overview",
    title: "Rune",
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
