import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react"
import { useScrollSpy } from "./useScrollSpy"
import "./primitives.css"

export type NavItem = {
  href: string
  label: string
  /** Mono counter shown before the label, e.g. a rule number. */
  num?: string
  /** Element id on this page that marks the item current while it is in view. */
  spy?: string
  /** The page this item opens is the one being shown. */
  current?: boolean
  children?: NavItem[]
  /** A quiet label above a run of children, e.g. a guide's section. */
  heading?: boolean
}

export type NavGroup = { label: string; items: NavItem[] }

function spyIds(items: NavItem[]): string[] {
  return items.flatMap(item => [...(item.spy ? [item.spy] : []), ...spyIds(item.children ?? [])])
}

/** Top-level items follow the one active section; children (a guide's rules) light up for everything in view. */
function NavLinks({ items, active, inView, depth }: { items: NavItem[]; active: string | null; inView: string[]; depth: number }) {
  const here = depth === 0 ? active : (inView[0] ?? null)
  return (
    <ul className="nav-list" data-depth={depth}>
      {items.map(item =>
        item.heading ? (
          <li key={item.href} className="nav-heading">
            <a href={item.href}>{item.label}</a>
          </li>
        ) : (
          <li key={item.href}>
            <a
              href={item.href}
              className="nav-link"
              data-parent={(item.children && item.children.length > 0) || undefined}
              data-active={(item.spy ? (depth === 0 ? item.spy === active : inView.includes(item.spy)) : item.current) || undefined}
              aria-current={item.current && !item.children ? "page" : item.spy && item.spy === here ? "location" : undefined}
            >
              {item.num && <span className="nav-num">{item.num}</span>}
              <span className="nav-label">{item.label}</span>
            </a>
            {item.children && item.children.length > 0 && (
              <NavLinks items={item.children} active={active} inView={inView} depth={depth + 1} />
            )}
          </li>
        ),
      )}
    </ul>
  )
}

/** One edge of the rail mark on a critically damped spring: position, velocity, and where it's headed. */
type Edge = { x: number; v: number; target: number }
type RailMotion = { top: Edge; bottom: Edge; x: number; shown: boolean; frame: number; last: number }
const still = (): Edge => ({ x: 0, v: 0, target: 0 })
/** .35 s, bounce 0. */
const RAIL_OMEGA = (2 * Math.PI) / 0.35
const RAIL_INSET = 2
const grid = (v: number) => Math.round(v * 64) / 64

/** Advance an edge by dt seconds with the exact critically damped solution, so retargets keep their velocity.
 *  Returns whether it's still moving. */
function step(s: Edge, dt: number): boolean {
  const d = s.x - s.target
  const c = s.v + RAIL_OMEGA * d
  const e = Math.exp(-RAIL_OMEGA * dt)
  s.x = s.target + (d + c * dt) * e
  s.v = (s.v - RAIL_OMEGA * c * dt) * e
  if (Math.abs(s.x - s.target) < 0.05 && Math.abs(s.v) < 1) {
    s.x = s.target
    s.v = 0
    return false
  }
  return true
}

const noSubscribe = () => () => {}
/** "⌘" on Apple platforms, "Ctrl " elsewhere; "⌘" on the server and during hydration. */
function useModKey(): string {
  return useSyncExternalStore(
    noSubscribe,
    () => (/Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl "),
    () => "⌘",
  )
}

/** The persistent sidebar: brand, then groups of links. A current top-level item sits on a pill; a current child
 *  (a rule in view, a problem being read) lights its stretch of the guide line, and that mark travels as you scroll.
 *  The list fades at whichever edge has links scrolled under it. Under 900 px it folds into a top bar. */
function Sidebar({
  brand,
  groups,
  revealBrandAfter,
  onSearch,
}: {
  brand: ReactNode
  groups: NavGroup[]
  revealBrandAfter?: string
  onSearch?: () => void
}) {
  const mod = useModKey()
  const ids = groups.flatMap(g => spyIds(g.items))
  const { active, inView } = useScrollSpy(ids)
  const [open, setOpen] = useState(false)
  const [brandShown, setBrandShown] = useState(!revealBrandAfter)

  useEffect(() => {
    const hero = revealBrandAfter && document.getElementById(revealBrandAfter)
    if (!hero) return setBrandShown(true)
    const observer = new IntersectionObserver(([entry]) => setBrandShown(!entry!.isIntersecting))
    observer.observe(hero)
    return () => observer.disconnect()
  }, [revealBrandAfter])
  const scroller = useRef<HTMLDivElement>(null)
  const pill = useRef<HTMLSpanElement>(null)
  const rail = useRef<HTMLSpanElement>(null)

  const fade = () => {
    const el = scroller.current
    if (!el) return
    const below = el.scrollHeight - el.clientHeight - el.scrollTop
    el.style.setProperty("--fade-top", String(Math.min(1, el.scrollTop / 15)))
    el.style.setProperty("--fade-bottom", String(Math.min(1, Math.max(0, below) / 15)))
  }

  const railMotion = useRef<RailMotion>({ top: still(), bottom: still(), x: 0, shown: false, frame: 0, last: 0 })
  useEffect(() => () => cancelAnimationFrame(railMotion.current.frame), [])

  useLayoutEffect(() => {
    const el = scroller.current
    const pillEl = pill.current
    const railEl = rail.current
    if (!el || !pillEl || !railEl) return
    const m = railMotion.current

    const paint = () => {
      const top = grid(m.top.x)
      railEl.style.transform = `translate(${m.x}px, ${top}px)`
      railEl.style.height = `${Math.max(0, grid(m.bottom.x) - top)}px`
    }
    const tick = (now: number) => {
      const dt = m.last ? Math.min((now - m.last) / 1000, 1 / 20) : 1 / 60
      m.last = now
      const moving = [m.top, m.bottom].map(s => step(s, dt)).some(Boolean)
      paint()
      m.frame = moving ? requestAnimationFrame(tick) : 0
    }
    const showRail = (x: number, top: number, bottom: number) => {
      m.x = x
      m.top.target = top
      m.bottom.target = bottom
      // First paint and reduced motion place the mark directly; otherwise the spring carries it, velocity and all.
      if (!m.shown || matchMedia("(prefers-reduced-motion: reduce)").matches) {
        cancelAnimationFrame(m.frame)
        m.frame = 0
        m.top = { x: top, v: 0, target: top }
        m.bottom = { x: bottom, v: 0, target: bottom }
        paint()
      } else if (!m.frame) {
        m.last = 0
        m.frame = requestAnimationFrame(tick)
      }
      m.shown = true
      // Fade back in after hiding, but not on the page's first paint.
      if (!railEl.dataset.placed) {
        railEl.style.transition = "none"
        railEl.style.opacity = "1"
        void railEl.offsetHeight
        railEl.style.transition = ""
        railEl.dataset.placed = ""
      }
      railEl.style.opacity = "1"
    }
    const hideRail = () => {
      m.shown = false
      railEl.style.opacity = "0"
    }
    const showPill = (link: HTMLElement, top: number) => {
      const first = !pillEl.dataset.placed
      if (first) pillEl.style.transition = "none"
      pillEl.style.transform = `translateY(${top}px)`
      pillEl.style.height = `${link.offsetHeight}px`
      if (first) {
        void pillEl.offsetHeight
        pillEl.style.transition = ""
        pillEl.dataset.placed = ""
      }
      pillEl.style.opacity = "1"
    }
    const hidePill = () => {
      pillEl.style.opacity = "0"
      delete pillEl.dataset.placed
    }

    const place = () => {
      const box = el.getBoundingClientRect()
      const offset = el.scrollTop - box.top
      const leaves = [...el.querySelectorAll<HTMLElement>(".nav-link[data-active]:not([data-parent])")]
      // The deepest current leaves win: the rules in view, not the guide that holds them.
      const nested = leaves.filter(link => link.closest(".nav-list")?.getAttribute("data-depth") !== "0")
      let span: [number, number] | null = null
      if (nested.length > 0) {
        hidePill()
        const list = nested[0]!.closest<HTMLElement>(".nav-list")!
        const top = nested[0]!.getBoundingClientRect().top + offset
        const bottom = nested.at(-1)!.getBoundingClientRect().bottom + offset
        // A 2 px bar centred on the 1 px guide line, running alongside the labels' text.
        showRail(list.getBoundingClientRect().left - box.left - 0.5, top + RAIL_INSET, bottom - RAIL_INSET)
        span = [top, bottom]
      } else if (leaves.length > 0) {
        hideRail()
        const link = leaves.at(-1)!
        const top = link.getBoundingClientRect().top + offset
        showPill(link, top)
        span = [top, top + link.offsetHeight]
      } else {
        hideRail()
        hidePill()
      }
      // Keep the current items inside the list's viewport, without animating the list; the first wins if they don't fit.
      if (span) {
        const [top, bottom] = span
        if (top - el.scrollTop < 24 || bottom - top > el.clientHeight - 48) el.scrollTop = top - 24
        else if (bottom - el.scrollTop > el.clientHeight - 24) el.scrollTop = bottom - el.clientHeight + 24
      }
      fade()
    }
    place()
    const observer = new ResizeObserver(place)
    observer.observe(el)
    return () => observer.disconnect()
  }, [active, inView])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false)
    document.addEventListener("keydown", onKey)
    return () => document.removeEventListener("keydown", onKey)
  }, [open])

  return (
    <aside className="sidebar" data-open={open || undefined}>
      <div className="sidebar-scrim" aria-hidden="true" onClick={() => setOpen(false)} />
      <div className="sidebar-brand" data-brand-hidden={!brandShown || undefined}>
        <div className="sidebar-lockup" inert={!brandShown}>
          {brand}
        </div>
        <div className="sidebar-actions">
          {onSearch && (
            <button
              type="button"
              className="sidebar-search"
              aria-label="Search"
              aria-keyshortcuts="Meta+K Control+K /"
              title={`Search (${mod}K)`}
              onClick={() => {
                setOpen(false)
                onSearch()
              }}
            >
              <svg className="sidebar-search-icon" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
                <circle cx="7" cy="7" r="4.75" fill="none" stroke="currentColor" strokeWidth="1.5" />
                <path d="M10.5 10.5 14 14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
              </svg>
              <span className="sidebar-search-label">Search</span>
            </button>
          )}
          <button
            type="button"
            className="sidebar-toggle"
            aria-expanded={open}
            aria-controls="sidebar-nav"
            onClick={() => setOpen(o => !o)}
          >
            {open ? "Close" : "Menu"}
          </button>
        </div>
      </div>
      <div
        ref={scroller}
        id="sidebar-nav"
        className="sidebar-scroll"
        onScroll={fade}
        onClick={e => (e.target as HTMLElement).closest("a") && setOpen(false)}
      >
        <span ref={pill} className="nav-pill" aria-hidden="true" />
        <span ref={rail} className="nav-rail" aria-hidden="true" />
        {groups.map(group => (
          <nav key={group.label} className="nav-group" aria-label={group.label}>
            <p className="nav-group-label">{group.label}</p>
            <NavLinks items={group.items} active={active} inView={inView} depth={0} />
          </nav>
        ))}
      </div>
    </aside>
  )
}

/** Page frame: sidebar on the left, a wide main column on the right, footer at the end of main. */
export function Frame({
  brand,
  groups,
  revealBrandAfter,
  onSearch,
  footer,
  children,
}: {
  brand: ReactNode
  groups: NavGroup[]
  /** Shows a search button in the sidebar's top row. */
  onSearch?: () => void
  /** Id of a hero that already shows the brand: the sidebar's lockup fades in once it scrolls away. */
  revealBrandAfter?: string
  footer?: ReactNode
  children: ReactNode
}) {
  return (
    <div className="frame">
      <Sidebar brand={brand} groups={groups} revealBrandAfter={revealBrandAfter} onSearch={onSearch} />
      <div className="frame-main">
        <main className="frame-content">{children}</main>
        {footer}
      </div>
    </div>
  )
}

/** Eyebrow, title and lede at the top of a page. */
export function PageHead({ eyebrow, title, children }: { eyebrow?: ReactNode; title: ReactNode; children?: ReactNode }) {
  return (
    <header className="page-head">
      {eyebrow && <p className="eyebrow">{eyebrow}</p>}
      <h1 className="page-title">{title}</h1>
      {children && <div className="page-lede">{children}</div>}
    </header>
  )
}

/** A section's heading on a hairline, with an optional count or note on the right. */
export function SectionHead({ id, title, aside, children }: { id?: string; title: ReactNode; aside?: ReactNode; children?: ReactNode }) {
  return (
    <header className="section-head">
      <div className="section-head-row">
        <h2 id={id}>{title}</h2>
        {aside && <span className="section-aside">{aside}</span>}
      </div>
      {children && <div className="section-intro">{children}</div>}
    </header>
  )
}
