// The wordmark's motion: one shared frame loop that only runs while something moves, the carve glint, the reduced
// motion fade, and the hover and idle triggers. Nothing plays on first paint.

type Tick = (dt: number) => boolean | void

const ticks = new Set<Tick>()
let raf = 0
let last = 0

/** Run `tick` every frame until it returns false. */
export function animate(tick: Tick) {
  ticks.add(tick)
  if (!raf) {
    last = performance.now()
    raf = requestAnimationFrame(frame)
  }
  return () => ticks.delete(tick)
}

function frame(now: number) {
  const dt = Math.min(.05, (now - last) / 1000)
  last = now
  for (const tick of [...ticks]) if (tick(dt) === false) ticks.delete(tick)
  raf = ticks.size ? requestAnimationFrame(frame) : 0
}

export const reducedMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches

const easeInOut = (p: number) => (p < .5 ? 2 * p * p : 1 - (-2 * p + 2) ** 2 / 2)

function invert(y: number, ease: (p: number) => number) {
  let lo = 0, hi = 1
  for (let i = 0; i < 20; i++) {
    const m = (lo + hi) / 2
    if (ease(m) < y) lo = m
    else hi = m
  }
  return (lo + hi) / 2
}

/** A light runs along `cells` in carve order over `travel` s; each cell warms in 40 ms and cools with a 180 ms time
 *  constant, so nothing cuts out. A replay mid-run restarts the head. */
export function glint(cells: { el: SVGElement; order: number }[], { travel = .7, rise = .04, tau = .18 } = {}) {
  const at = cells.map(c => invert(c.order, easeInOut) * travel)
  let t = -1
  const tick = (dt: number) => {
    t += dt
    let alive = false
    cells.forEach((c, i) => {
      const since = t - at[i]!
      let h = since < 0 ? 0 : since < rise ? since / rise : Math.exp(-(since - rise) / tau)
      if (since < 0 || h > .004) alive = true
      else h = 0
      c.el.setAttribute("fill-opacity", h.toFixed(3))
    })
    if (!alive) t = -1
    return alive
  }
  return () => {
    const idle = t < 0
    t = 0
    if (idle) animate(tick)
  }
}

/** Reduced motion: no travel. Fade `el` out, show the lit pose, fade in, hold, and back; 90 ms fades. */
export function fadeSwap(el: SVGElement | HTMLElement, pose: (lit: boolean) => void, hold = .5) {
  const FADE = .09
  const plan: [number, number, number, (() => void)?][] = [
    [FADE, 1, 0, () => pose(true)],
    [FADE, 0, 1],
    [hold, 1, 1],
    [FADE, 1, 0, () => pose(false)],
    [FADE, 0, 1],
  ]
  let i = 0, t = 0
  animate(dt => {
    t += dt
    while (i < plan.length && t >= plan[i]![0]) {
      t -= plan[i]![0]
      plan[i]![3]?.()
      i++
    }
    if (i >= plan.length) {
      el.style.opacity = ""
      return false
    }
    const [d, a, b] = plan[i]!, p = t / d
    el.style.opacity = String(a + (b - a) * (b > a ? 1 - (1 - p) ** 2 : p * p))
  })
}

let lastScroll = -Infinity
if (typeof window !== "undefined") addEventListener("scroll", () => { lastScroll = performance.now() }, { passive: true, capture: true })

/** Play on hover; `periodic` adds an idle play every 5–9 s while `host` is on screen, the tab is visible, the page
 *  isn't scrolling and motion isn't reduced. The first one fires 5–9 s after load. Returns a cleanup. */
export function attachTriggers(host: HTMLElement, play: (reduced: boolean) => void, { periodic = false } = {}) {
  const enter = () => play(reducedMotion())
  host.addEventListener("pointerenter", enter)
  if (!periodic) return () => host.removeEventListener("pointerenter", enter)

  let visible = false, timer = 0
  const schedule = () => {
    clearTimeout(timer)
    if (!visible || document.hidden) return
    timer = window.setTimeout(() => {
      if (!reducedMotion() && performance.now() - lastScroll > 500) play(false)
      schedule()
    }, (5 + Math.random() * 4) * 1000)
  }
  const observer = new IntersectionObserver(([entry]) => {
    visible = entry!.isIntersecting
    schedule()
  })
  observer.observe(host)
  document.addEventListener("visibilitychange", schedule)
  return () => {
    host.removeEventListener("pointerenter", enter)
    observer.disconnect()
    document.removeEventListener("visibilitychange", schedule)
    clearTimeout(timer)
  }
}
