// The site lives under /rune on anoma.ly (production) and dev.anoma.ly (dev).
// Files keep root-relative paths internally ("/og.png"); these helpers add and remove the prefix at the edges.
export const BASE_PATH = "/rune"

// The build mode (`--mode dev`) picks the environment; every other mode builds production.
export function hostForMode(mode: string | undefined): string {
  return mode === "dev" ? "dev.anoma.ly" : "anoma.ly"
}

export function withBase(path: string): string {
  return `${BASE_PATH}${path}`
}

// Returns the site path for a request path, or undefined when it is outside the base.
export function stripBase(pathname: string): string | undefined {
  if (pathname === BASE_PATH) return "/"
  if (!pathname.startsWith(`${BASE_PATH}/`)) return undefined
  return pathname.slice(BASE_PATH.length)
}
