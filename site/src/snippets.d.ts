declare module "virtual:snippets" {
  import type { Snippet } from "../scripts/snippets"
  const snippets: Record<string, Snippet>
  export default snippets
}
