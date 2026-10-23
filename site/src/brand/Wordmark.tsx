import { useEffect, useRef, type CSSProperties } from "react"
import { attachTriggers, fadeSwap, glint } from "./motion"
import { WORDMARK, carveOrder, runs } from "./pixels"

const { glyphs, width, height } = WORDMARK
const rune = glyphs[0]!
const carve = carveOrder(rune.rows)

/** ᚱUNE in pixel letters. Hover runs a light down the ᚱ's strokes (a fade under reduced motion); `periodic`, the home
 *  masthead, also plays it every 5–9 s while it's on screen. */
export function Wordmark({ periodic = false }: { periodic?: boolean }) {
  const ref = useRef<HTMLSpanElement>(null)

  useEffect(() => {
    const box = ref.current
    if (!box) return
    const print = box.querySelector<SVGGElement>(".wordmark-rune")!
    const light = glint([...box.querySelectorAll<SVGRectElement>(".wordmark-glint rect")].map(el => ({ el, order: Number(el.dataset.order) })))
    return attachTriggers(box, reduced => (reduced ? fadeSwap(print, lit => print.classList.toggle("is-lit", lit)) : light()), { periodic })
  }, [periodic])

  return (
    <span ref={ref} className="wordmark" style={{ "--cells": width, aspectRatio: `${width} / ${height}` } as CSSProperties}>
      <svg viewBox={`0 0 ${width} ${height}`} shapeRendering="crispEdges" aria-hidden="true" focusable="false">
        <g className="wordmark-rune">
          {runs(rune.rows, rune.x).map(([x, y, w, h], i) => <rect key={i} x={x} y={y} width={w} height={h} />)}
          <g className="wordmark-glint">
            {carve.map(([x, y, order]) => (
              <rect key={`${x},${y}`} x={rune.x + x} y={y} width={1} height={1} fillOpacity={0} data-order={order.toFixed(4)} />
            ))}
          </g>
        </g>
        {glyphs.slice(1).map(g => (
          <g key={g.char}>
            {runs(g.rows, g.x).map(([x, y, w, h], i) => <rect key={i} x={x} y={y} width={w} height={h} />)}
          </g>
        ))}
      </svg>
    </span>
  )
}
