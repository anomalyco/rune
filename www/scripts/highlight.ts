/** A small build-time highlighter: one regex pass per language, spans with `tk-*` classes. Copied from
 *  @anomalyco/site's content/highlight.ts until this site moves onto the shared package. Only scripts/snippets.ts
 *  imports it, at build time; the client gets finished HTML. */

export type CodeLang = "rust" | "toml" | "ts" | "css" | "shell" | "plain"
export const CODE_LANGS: readonly CodeLang[] = ["rust", "toml", "ts", "css", "shell", "plain"]

type Role = "k" | "s" | "n" | "t" | "f" | "a" | "c" | "p"
type Rule = [RegExp, Role | null]

const escape = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
const words = (list: string) => new RegExp(`\\b(?:${list.split(" ").join("|")})\\b`, "y")

const STRING: Rule = [/"(?:\\.|[^"\\\n])*"?/y, "s"]
const NUMBER: Rule = [/\b\d[\d_]*(?:\.\d+)?(?:[a-z%]+\d*)?\b|(?<![\w.])\.\d+\b/y, "n"]
const WORD: Rule = [/[A-Za-z_][\w]*/y, null]

const RULES: Record<Exclude<CodeLang, "plain">, Rule[]> = {
  rust: [
    [/\/\/[^\n]*/y, "c"],
    STRING,
    [/#!?\[[^\]\n]*\]/y, "a"],
    [/'[a-z_]\w*\b(?!')/y, "a"],
    [/[a-z_]\w*!/y, "f"],
    NUMBER,
    [words("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while"), "k"],
    [/[A-Z]\w*/y, "t"],
    [/[a-z_]\w*(?=\s*(?:::<[^>]*>)?\()/y, "f"],
    WORD,
    [/::|->|=>|[{}()[\];,<>&|]/y, "p"],
  ],
  toml: [
    [/#[^\n]*/y, "c"],
    STRING,
    [/(?<=^|\n)\s*\[\[?[^\]\n]+\]\]?/y, "a"],
    [words("true false"), "k"],
    NUMBER,
    [/[A-Za-z_][\w.:-]*(?=\s*=)/y, null],
    WORD,
    [/[{}[\],=]/y, "p"],
  ],
  ts: [
    [/\/\/[^\n]*|\/\*[\s\S]*?\*\//y, "c"],
    STRING,
    [/'(?:\\.|[^'\\\n])*'?|`(?:\\.|[^`\\])*`?/y, "s"],
    NUMBER,
    [words("as async await break case catch class const continue default else export extends false for from function if import in interface let new null of return satisfies switch this throw true try type typeof undefined var void while"), "k"],
    [/[A-Z]\w*/y, "t"],
    [/[a-z_$][\w$]*(?=\s*\()/y, "f"],
    [/[\w$]+/y, null],
    [/=>|[{}()[\];,<>=:|&?]/y, "p"],
  ],
  css: [
    [/\/\*[\s\S]*?\*\//y, "c"],
    STRING,
    [/@[\w-]+/y, "k"],
    [/--[\w-]+/y, "a"],
    [/[a-z-]+(?=\s*:(?!:))/y, null],
    [/#[0-9a-fA-F]{3,8}\b/y, "n"],
    NUMBER,
    [/[a-z-]+(?=\()/y, "f"],
    [/::?[a-z-]+/y, "a"],
    [/[\w-]+/y, null],
    [/[{}()[\];,:/]/y, "p"],
  ],
  shell: [
    [/#[^\n]*/y, "c"],
    STRING,
    [/'[^'\n]*'?/y, "s"],
    [/(?<=^|[|;&]\s*)[\w./+-]+/y, "f"],
    [/(?<=\s)--?[\w-]+/y, "a"],
    [/<[\w-]+>/y, "c"],
    [/[\w./:@+-]+/y, null],
    [/[|;&=<>]/y, "p"],
  ],
}

export function highlight(code: string, lang: CodeLang): string {
  if (lang === "plain") return escape(code)
  const rules = RULES[lang]
  const runs: { role: Role | null; text: string }[] = []
  const push = (role: Role | null, text: string) => {
    const last = runs.at(-1)
    // Whitespace joins the run before it, so `) -> (` stays one span.
    if (last && (last.role === role || (last.role && /^\s+$/.test(text)))) last.text += text
    else runs.push({ role, text })
  }
  let i = 0
  outer: while (i < code.length) {
    for (const [re, role] of rules) {
      re.lastIndex = i
      const m = re.exec(code)
      if (m && m[0].length > 0) {
        push(role, m[0])
        i += m[0].length
        continue outer
      }
    }
    push(null, code[i]!)
    i++
  }
  return runs
    .map(({ role, text }) => {
      if (!role) return escape(text)
      const body = text.trimEnd()
      return `<span class="tk-${role}">${escape(body)}</span>${escape(text.slice(body.length))}`
    })
    .join("")
}
