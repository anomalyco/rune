import { useEffect, useRef } from "react"
import { animate, isReduced, smootherstep, watchVisible } from "../motion/visible"

const STEPS: [string, string][] = [
  ["Input", "pointer, keys, text"],
  ["State", "your struct, changed"],
  ["View", "your view, called again"],
  ["Diff", "keyed, against the last"],
  ["Layout", "flex and grid, by Taffy"],
  ["Springs", "position and velocity"],
  ["Draw", "one instanced draw"],
]

const TRAVEL = 1.8 // seconds from the first step to the last
const RISE = 0.06
const TAU = 0.32 // cooling time constant: a lit step fades, it never cuts out

/** The frame's path as a row of steps (a column on a phone). When it comes into view a light runs along it once,
 *  warming each step as it passes; click to run it again. Under reduced motion the steps light together and fade. */
export function Pipeline() {
  const root = useRef<HTMLOListElement>(null)

  useEffect(() => {
    const ol = root.current!
    const nodes = [...ol.querySelectorAll<HTMLElement>(".pl-step")]
    const heats = nodes.map(n => n.querySelector<HTMLElement>(".pl-heat")!)
    const comet = ol.querySelector<HTMLElement>(".pl-comet")!
    const wire = ol.querySelector<HTMLElement>(".pl-wire")!
    let centers: [number, number][] = []
    let t = -1
    let played = false
    let visible = false

    const measure = () => {
      const o = ol.getBoundingClientRect()
      centers = nodes.map(n => {
        const r = n.getBoundingClientRect()
        return [r.left - o.left + r.width / 2, r.top - o.top + r.height / 2]
      })
      const [a, b] = [centers[0]!, centers.at(-1)!]
      const vertical = Math.abs(b[1] - a[1]) > Math.abs(b[0] - a[0])
      ol.dataset.axis = vertical ? "y" : "x"
      wire.style.cssText = vertical
        ? `left:${a[0]}px;top:${a[1]}px;width:1px;height:${b[1] - a[1]}px`
        : `left:${a[0]}px;top:${a[1]}px;width:${b[0] - a[0]}px;height:1px`
    }

    const at = (p: number): [number, number] => {
      const span = (centers.length - 1) * p
      const i = Math.min(centers.length - 2, Math.floor(span))
      const f = span - i
      const [a, b] = [centers[i]!, centers[i + 1]!]
      return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]
    }

    // When the head passes each step: invert the smootherstep by bisection.
    const passes = nodes.map((_, i) => {
      const target = i / (nodes.length - 1)
      let lo = 0, hi = 1
      for (let k = 0; k < 18; k++) {
        const m = (lo + hi) / 2
        if (smootherstep(m) < target) lo = m
        else hi = m
      }
      return lo * TRAVEL
    })

    const tick = (dt: number) => {
      if (!visible) {
        // Off screen or hidden: finish cooled, without drawing the rest.
        heats.forEach(h => (h.style.opacity = "0"))
        comet.style.opacity = "0"
        t = -1
        return false
      }
      t += dt
      const p = smootherstep(t / TRAVEL)
      const [x, y] = at(p)
      comet.style.transform = `translate3d(${x.toFixed(1)}px,${y.toFixed(1)}px,0)`
      // The head is bright while it travels and cools once it reaches the draw.
      const tail = t - TRAVEL
      comet.style.opacity = (tail < 0 ? 1 : Math.exp(-tail / TAU)).toFixed(3)
      let alive = tail < 0 || Number(comet.style.opacity) > 0.004
      nodes.forEach((_, i) => {
        const since = t - passes[i]!
        const last = i === nodes.length - 1
        let h = since < 0 ? 0 : since < RISE ? since / RISE : Math.exp(-(since - RISE) / (last ? TAU * 2.5 : TAU))
        if (since < 0 || h > 0.004) alive = true
        else h = 0
        heats[i]!.style.opacity = h.toFixed(3)
      })
      if (!alive) t = -1
      return alive
    }

    const fade = () => {
      let s = 0
      animate(dt => {
        s += dt
        const h = s < 0.75 ? smootherstep(s / 0.15) : 1 - smootherstep((s - 0.75) / 0.15)
        heats.forEach(el => (el.style.opacity = h.toFixed(3)))
        return s < 0.9
      })
    }

    const play = () => {
      if (isReduced()) return fade()
      measure()
      const idle = t < 0
      t = 0
      if (idle) animate(tick)
    }

    const ro = new ResizeObserver(measure)
    ro.observe(ol)
    const stopWatch = watchVisible(
      ol,
      v => {
        visible = v
        if (v && !played) {
          played = true
          play()
        }
      },
      0.4,
    )
    ol.addEventListener("click", play)
    return () => {
      ro.disconnect()
      stopWatch()
      ol.removeEventListener("click", play)
    }
  }, [])

  return (
    <figure className="pipeline">
      <ol className="pl" ref={root} aria-label="One frame, step by step">
        <span className="pl-wire" aria-hidden="true" />
        <span className="pl-comet" aria-hidden="true" />
        {STEPS.map(([name, note]) => (
          <li key={name} className="pl-step">
            <span className="pl-heat" aria-hidden="true" />
            <span className="pl-name">{name}</span>
            <span className="pl-note">{note}</span>
          </li>
        ))}
      </ol>
      <figcaption className="demo-caption">Click the diagram to run the frame again.</figcaption>
    </figure>
  )
}
