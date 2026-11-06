import { useEffect, useRef, useState } from "react"
import { withBase } from "../base"

type Phase = "poster" | "live" | "still" | "unloaded"

const CAPTION: Record<Phase, string> = {
  poster: "Rune draws this panel in your browser with WebGPU.",
  live: "Drawn by Rune, live. Flip the switch, pick a spring, throw the puck.",
  still: "A still: this browser has no WebGPU, so Rune can't draw it here.",
  unloaded: "A still: the live version didn't load.",
}

type Gpu = { requestAdapter(): Promise<unknown> }

/** The hero's live panel: examples/web-showcase compiled to wasm, drawn by Rune on a canvas the page already has.
 *  Until it's ready an HTML copy of its first frame stands in, in the same box, so nothing shifts. The wasm loads only
 *  after the page has loaded and gone idle and someone has moved, touched or scrolled, when the panel is near the
 *  screen, and only where WebGPU exists. */
export function LivePanel() {
  const box = useRef<HTMLDivElement>(null)
  const [phase, setPhase] = useState<Phase>("poster")

  useEffect(() => {
    const el = box.current!
    const gpu = (navigator as Navigator & { gpu?: Gpu }).gpu
    let cancelled = false
    let io: IntersectionObserver | undefined
    let mo: MutationObserver | undefined

    const start = async () => {
      if (!gpu || !(await gpu.requestAdapter().catch(() => null))) return setPhase("still")
      if (cancelled) return
      // Rune fills its screen-reader mirror once it has laid out and drawn a frame: that's when the poster can go.
      const drawn = new Promise<void>(resolve => {
        mo = new MutationObserver(() => {
          if (el.querySelector("[data-rune-a11y] *")) resolve()
        })
        mo.observe(el, { childList: true, subtree: true })
      })
      try {
        const url = withBase("/live/app.js")
        const mod = (await import(/* @vite-ignore */ url)) as { default: (o: { module_or_path: string }) => Promise<unknown> }
        await mod.default({ module_or_path: withBase("/live/app_bg.wasm") })
      } catch {
        return setPhase("unloaded")
      }
      await Promise.race([drawn, new Promise(r => setTimeout(r, 3000))])
      mo?.disconnect()
      requestAnimationFrame(() => requestAnimationFrame(() => !cancelled && setPhase("live")))
    }

    const whenNear = () => {
      io = new IntersectionObserver(
        ([entry]) => {
          if (!entry!.isIntersecting) return
          io!.disconnect()
          void start()
        },
        { rootMargin: "200px" },
      )
      io.observe(el)
    }
    // 2 MB of wasm waits for a sign of a person (a move, a touch, a key, a scroll), so a page nobody touches never
    // pays for it.
    const SIGNS = ["pointermove", "pointerdown", "keydown", "wheel", "scroll"] as const
    const onSign = () => {
      SIGNS.forEach(type => removeEventListener(type, onSign, true))
      whenNear()
    }
    const idle = (fn: () => void) =>
      "requestIdleCallback" in window ? requestIdleCallback(fn, { timeout: 2000 }) : setTimeout(fn, 200)
    const afterLoad = () =>
      idle(() => !cancelled && SIGNS.forEach(type => addEventListener(type, onSign, { capture: true, passive: true })))
    if (document.readyState === "complete") afterLoad()
    else addEventListener("load", afterLoad, { once: true })

    return () => {
      cancelled = true
      removeEventListener("load", afterLoad)
      SIGNS.forEach(type => removeEventListener(type, onSign, true))
      io?.disconnect()
      mo?.disconnect()
    }
  }, [])

  return (
    <figure className="live" data-phase={phase}>
      <div className="live-stage" ref={box}>
        <div className="live-frame">
          <canvas id="rune-live" className="live-canvas" />
        </div>
        <Poster />
      </div>
      <figcaption className="live-caption" aria-live="polite">
        {CAPTION[phase]}
      </figcaption>
    </figure>
  )
}

/** The showcase's first frame in HTML and CSS, to the point: same card, same controls, same place. */
function Poster() {
  return (
    <div className="poster" aria-hidden="true">
      <div className="pc-card">
        <div className="pc-line">
          <span className="pc-label">Lights</span>
          <span className="pc-switch">
            <span className="pc-knob" />
          </span>
        </div>
        <div className="pc-line">
          <span className="pc-label">Spring</span>
          <span className="pc-picker">
            <span className="pc-thumb" />
            <span className="pc-seg">Snappy</span>
            <span className="pc-seg">Smooth</span>
            <span className="pc-seg is-on">Bouncy</span>
          </span>
        </div>
        <div className="pc-track">
          <span className="pc-rail" />
          {[0, 1, 2, 3].map(i => (
            <span key={i} className="pc-stop" style={{ left: `calc(12px + (100% - 28px) * ${i / 3})` }} />
          ))}
          <span className="pc-puck" />
        </div>
      </div>
    </div>
  )
}
