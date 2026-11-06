declare module "virtual:snippets" {
  import type { Snippet } from "../scripts/snippets"
  const snippets: Record<string, Snippet>
  export default snippets
}

declare const __RUNE_REV__: string
