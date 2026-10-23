import { useEffect, useState, type JSX, type MouseEvent } from "react"
import { layers, milestones, principles, risks } from "./content"
import { ArchitectureView, HomeView, HowItWorksView, NotFoundView, PrinciplesView, RoadmapView, num } from "./pages"
import { stripBase, withBase } from "./base"
import { REPO, normalizePath } from "./routes"
import signature from "./footer-signature.svg"
import { Frame, type NavGroup, type NavItem } from "./system/Frame"

function Wordmark() {
  return (
    <a className="wordmark" href={withBase("/")} aria-label="Rune, overview">
      <span className="wordmark-glyph" aria-hidden="true" />
      Rune
    </a>
  )
}

function navGroups(pathname: string): NavGroup[] {
  const page = (path: string, label: string, children: NavItem[]): NavItem => ({
    href: withBase(path),
    label,
    current: pathname === path,
    children: pathname === path ? children : undefined,
  })
  const spy = (id: string, label: string, n?: string): NavItem => ({ href: `#${id}`, label, spy: id, num: n })
  const heading = (id: string, label: string): NavItem => ({ href: `#${id}`, label, heading: true })

  return [
    {
      label: "Rune",
      items: [
        page("/", "Overview", [spy("status", "Status"), spy("next", "Next")]),
        page("/how-it-works", "How it works", [
          spy("frame", "A frame"),
          spy("app", "A window and a view"),
          spy("state", "State changes in handlers"),
          spy("springs", "Springs keep their velocity"),
          spy("live", "Values that skip the view"),
          spy("threads", "Threads"),
          spy("testing", "Tests without a window"),
          spy("crates", "Where the code lives"),
        ]),
        page("/architecture", "Architecture", [
          spy("overview", "Overview"),
          spy("channel", "The command channel"),
          heading("layers", "Layers"),
          ...layers.map((l, i) => spy(l.slug, l.title, num(i))),
          heading("choices", "Choices"),
          spy("decisions", "Decisions"),
          spy("gpui", "Why not fork GPUI"),
        ]),
        page(
          "/principles",
          "Principles",
          principles.map((p, i) => spy(p.slug, p.title, num(i))),
        ),
        page("/roadmap", "Roadmap", [
          heading("milestones", "Milestones"),
          ...milestones.map(m => spy(m.id, m.title)),
          heading("risks", "Risks"),
          ...risks.map(r => spy(r.slug, r.title)),
        ]),
      ],
    },
    {
      label: "Source",
      items: [
        { href: REPO, label: "GitHub" },
        { href: `${REPO}/blob/dev/PLAN.md`, label: "Plan" },
        { href: `${REPO}/tree/dev/research`, label: "Research notes" },
      ],
    },
  ]
}

function Footer() {
  return (
    <footer className="site-footer">
      <div className="copyright">
        <img src={signature} alt="" aria-hidden="true" width={163} height={64} />
        <p>
          ©2026 <a href="https://anoma.ly">Anomaly</a>. MIT licence
        </p>
      </div>
    </footer>
  )
}

// The site path for a browser path under the base; paths outside it fall through to the not-found view.
function sitePath(pathname: string): string {
  return normalizePath(stripBase(pathname) ?? pathname)
}

export function App({ pathname: initial }: { pathname?: string }) {
  const [pathname, setPathname] = useState(() =>
    sitePath(initial ?? (typeof window !== "undefined" ? window.location.pathname : "/")),
  )

  useEffect(() => {
    const onPopState = () => setPathname(sitePath(window.location.pathname))
    window.addEventListener("popstate", onPopState)
    return () => window.removeEventListener("popstate", onPopState)
  }, [])

  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return
    const href = (e.target as HTMLElement | null)?.closest("a")?.getAttribute("href")
    if (!href || !href.startsWith("/") || /\.\w+$/.test(href)) return
    const url = new URL(href, window.location.origin)
    const next = sitePath(url.pathname)
    if (next === pathname) return
    e.preventDefault()
    window.history.pushState(null, "", `${url.pathname}${url.hash}`)
    setPathname(next)
    if (url.hash) requestAnimationFrame(() => document.getElementById(url.hash.slice(1))?.scrollIntoView())
    else window.scrollTo(0, 0)
  }

  const views: Record<string, () => JSX.Element> = {
    "/": HomeView,
    "/how-it-works": HowItWorksView,
    "/principles": PrinciplesView,
    "/architecture": ArchitectureView,
    "/roadmap": RoadmapView,
  }
  const View = views[pathname] ?? NotFoundView

  return (
    <div className="site" onClick={onClick}>
      <Frame brand={<Wordmark />} groups={navGroups(pathname)} footer={<Footer />}>
        <View />
      </Frame>
    </div>
  )
}
