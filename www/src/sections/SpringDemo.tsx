import { useEffect, useRef, useState } from "react"
import { Animated, spring, type Spring } from "../motion/spring"
import { animate, clamp01, isReduced, now, watchVisible } from "../motion/visible"

const SPRINGS: [string, Spring][] = [
  ["Snappy", spring(0.25)],
  ["Smooth", spring(0.4)],
  ["Bouncy", spring(0.5, 0.3)],
]
const STOPS = 5
const WINDOW = 2.4 // seconds of history in the trace
const snap = (x: number) => Math.round(clamp01(x) * (STOPS - 1)) / (STOPS - 1)

type Sample = { t: number; x: number; target: number; kick: boolean }

/** A puck on a track, driven by Rune's spring. Click to send it somewhere, click again before it lands, or drag and
 *  throw it. The trace underneath plots its position: with velocity kept, every retarget bends the curve smoothly. */
export function SpringDemo() {
  const [which, setWhich] = useState(2)
  const [keep, setKeep] = useState(true)
  const stage = useRef<HTMLDivElement>(null)
  const puck = useRef<HTMLDivElement>(null)
  const trace = useRef<HTMLCanvasElement>(null)
  const api = useRef<{ setSpring(s: Spring): void; setKeep(k: boolean): void; step(dir: number): void } | null>(null)

  useEffect(() => {
    const box = stage.current!, knob = puck.current!, canvas = trace.current!
    const anim = new Animated(0, SPRINGS[2]![1])
    let keepVelocity = true
    let samples: Sample[] = []
    let kicked = false
    let lastMotion = -Infinity
    let visible = false
    let running = false
    let width = box.clientWidth
    let drag: { offset: number; pts: [number, number][]; live: boolean } | null = null

    // The puck's centre runs between 32 px in from each end (16 of padding, 16 of half puck).
    const travel = () => Math.max(1, width - 64)
    const toFrac = (clientX: number) => clamp01((clientX - box.getBoundingClientRect().left - 32) / travel())

    const draw = (t: number) => {
      const [x] = anim.sample(t)
      knob.style.transform = `translate3d(${(x * travel()).toFixed(2)}px,0,0)`
      knob.setAttribute("aria-valuenow", String(Math.round(anim.target * 100)))
      samples.push({ t, x, target: anim.target, kick: kicked })
      kicked = false
      while (samples.length > 2 && samples[1]!.t < t - WINDOW) samples.shift()
      plot(t)
    }

    const plot = (t: number) => {
      const dpr = Math.min(2, devicePixelRatio || 1)
      const w = canvas.clientWidth, h = canvas.clientHeight
      if (canvas.width !== Math.round(w * dpr)) {
        canvas.width = Math.round(w * dpr)
        canvas.height = Math.round(h * dpr)
      }
      const g = canvas.getContext("2d")!
      g.setTransform(dpr, 0, 0, dpr, 0, 0)
      g.clearRect(0, 0, w, h)
      const pad = 8
      const X = (s: number) => w - ((t - s) / WINDOW) * w
      // Bouncy springs overshoot: leave room above and below the track's ends.
      const Y = (v: number) => h - pad - ((v + 0.15) / 1.3) * (h - 2 * pad)
      g.lineWidth = 1
      g.strokeStyle = "rgba(255,255,255,.12)"
      g.setLineDash([2, 4])
      g.beginPath()
      samples.forEach((s, i) => (i ? g.lineTo(X(s.t), Y(s.target)) : g.moveTo(X(s.t), Y(s.target))))
      g.stroke()
      g.setLineDash([])
      g.lineWidth = 1.5
      g.strokeStyle = "oklch(0.74 0.14 245)"
      g.beginPath()
      samples.forEach((s, i) => (i ? g.lineTo(X(s.t), Y(s.x)) : g.moveTo(X(s.t), Y(s.x))))
      g.stroke()
      g.fillStyle = "oklch(0.97 0.035 245)"
      for (const s of samples) {
        if (!s.kick) continue
        g.beginPath()
        g.arc(X(s.t), Y(s.x), 2.5, 0, Math.PI * 2)
        g.fill()
      }
    }

    // One frame loop, only while something moves or the trace is still scrolling, and only on screen.
    const run = () => {
      if (running || !visible) return
      running = true
      animate(() => {
        const t = now()
        if (!visible) return (running = false)
        if (drag || !anim.settled(t, 0.0005)) lastMotion = t
        draw(t)
        if (t - lastMotion > WINDOW) return (running = false)
      })
    }

    const retarget = (target: number) => {
      const t = now()
      if (isReduced()) anim.set(target, t)
      else anim.to(target, t, keepVelocity)
      kicked = true
      lastMotion = t
      run()
    }

    const down = (e: PointerEvent) => {
      if (e.button !== 0) return
      const t = now()
      const x = anim.sample(t)[0]
      const at = toFrac(e.clientX)
      const onPuck = Math.abs((at - x) * travel()) < 22
      box.setPointerCapture(e.pointerId)
      drag = { offset: at - x, pts: [[t, at]], live: onPuck }
      if (onPuck) {
        // Caught: the finger holds it where it is, with no animation while it's held.
        anim.set(x, t)
        kicked = true
        run()
      } else retarget(at)
    }
    const move = (e: PointerEvent) => {
      if (!drag) return
      const t = now()
      const at = toFrac(e.clientX)
      drag.pts.push([t, at])
      if (drag.pts.length > 6) drag.pts.shift()
      if (!drag.live) {
        // A press on the track sent the puck there; moving 4 px takes it over from wherever it is now.
        if (Math.abs(at - drag.pts[0]![1]) * travel() < 4) return
        drag.live = true
        drag.offset = at - anim.sample(t)[0]
      }
      anim.set(clamp01(at - drag.offset), t)
    }
    const up = (e: PointerEvent) => {
      if (!drag) return
      const t = now()
      const { live, pts: all } = drag
      drag = null
      box.releasePointerCapture?.(e.pointerId)
      if (!live) return
      const pts = all.filter(([pt]) => t - pt < 0.1)
      const v = pts.length > 1 ? (pts.at(-1)![1] - pts[0]![1]) / Math.max(0.008, pts.at(-1)![0] - pts[0]![0]) : 0
      const x = anim.sample(t)[0]
      const target = snap(x + v * 0.12)
      if (isReduced()) anim.set(target, t)
      else anim.release(target, keepVelocity ? v : 0, t)
      kicked = true
      lastMotion = t
      run()
    }

    box.addEventListener("pointerdown", down)
    box.addEventListener("pointermove", move)
    box.addEventListener("pointerup", up)
    box.addEventListener("pointercancel", up)
    const ro = new ResizeObserver(() => {
      width = box.clientWidth
      draw(now())
    })
    ro.observe(box)
    const stopWatch = watchVisible(box, v => {
      visible = v
      if (v && now() - lastMotion < WINDOW) run()
    })
    api.current = {
      setSpring: s => (anim.spring = s),
      setKeep: k => (keepVelocity = k),
      step: dir => retarget(snap(anim.target + dir / (STOPS - 1))),
    }
    draw(now())
    return () => {
      box.removeEventListener("pointerdown", down)
      box.removeEventListener("pointermove", move)
      box.removeEventListener("pointerup", up)
      box.removeEventListener("pointercancel", up)
      ro.disconnect()
      stopWatch()
      visible = false
      samples = []
    }
  }, [])

  useEffect(() => {
    api.current?.setSpring(SPRINGS[which]![1])
  }, [which])
  useEffect(() => {
    api.current?.setKeep(keep)
  }, [keep])

  return (
    <figure className="demo spring-demo">
      <div className="sd-stage" ref={stage}>
        <div className="sd-rail" aria-hidden="true">
          {Array.from({ length: STOPS }, (_, i) => (
            <span key={i} className="sd-stop" style={{ left: `${(i / (STOPS - 1)) * 100}%` }} />
          ))}
        </div>
        <div
          ref={puck}
          className="sd-puck"
          role="slider"
          tabIndex={0}
          aria-label="Puck position"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={0}
          onKeyDown={e => {
            const dir = e.key === "ArrowRight" || e.key === "ArrowUp" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowDown" ? -1 : 0
            if (!dir) return
            e.preventDefault()
            api.current?.step(dir)
          }}
        />
      </div>
      <canvas className="sd-trace" ref={trace} aria-hidden="true" />
      <div className="demo-controls">
        <div className="segmented" role="radiogroup" aria-label="Spring">
          {SPRINGS.map(([name, s], i) => (
            <button key={name} type="button" role="radio" aria-checked={i === which} onClick={() => setWhich(i)}>
              {name}
              <span className="seg-note">
                {s.duration}s{s.bounce ? ` · ${s.bounce}` : ""}
              </span>
            </button>
          ))}
        </div>
        <label className="check">
          <input type="checkbox" checked={keep} onChange={e => setKeep(e.target.checked)} />
          <span>Keep velocity</span>
        </label>
      </div>
      <figcaption className="demo-caption">
        Click the track, then click somewhere else before the puck lands. Or grab it and throw it. Turn off “Keep
        velocity” to see a spring that starts from rest every time.
      </figcaption>
    </figure>
  )
}
