import snippets from "virtual:snippets"
import { REPO } from "./routes"

/** A code example cut from Rune's source at build time (scripts/snippets.ts), with a link to the lines it came from. */
export function Code({ id }: { id: string }) {
  const s = snippets[id]
  if (!s) throw new Error(`Unknown snippet "${id}"; add it to scripts/snippets.ts`)
  const href = `${REPO}/blob/dev/${s.file}${s.start ? `#L${s.start}-L${s.end}` : ""}`
  return (
    <figure className="code-block">
      <figcaption className="code-head">
        <a href={href}>
          {s.file}
          {s.start > 0 && <span className="code-lines">:{s.start}–{s.end}</span>}
        </a>
        <span className="code-lang">{s.label}</span>
      </figcaption>
      <pre className="code">
        <code dangerouslySetInnerHTML={{ __html: s.html }} />
      </pre>
    </figure>
  )
}
