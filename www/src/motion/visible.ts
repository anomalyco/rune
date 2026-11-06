import { reducedMotion } from "../brand/motion"

export { animate, reducedMotion } from "../brand/motion"

/** Calls `onChange` when `el` comes on or goes off screen, or the tab is hidden or shown. Returns a cleanup. */
export function watchVisible(el: Element, onChange: (visible: boolean) => void, threshold = 0) {
  let onScreen = false
  const emit = () => onChange(onScreen && !document.hidden)
  const io = new IntersectionObserver(([entry]) => {
    onScreen = entry!.isIntersecting
    emit()
  }, { threshold })
  io.observe(el)
  document.addEventListener("visibilitychange", emit)
  return () => {
    io.disconnect()
    document.removeEventListener("visibilitychange", emit)
  }
}

export const now = () => performance.now() / 1000

export const isReduced = () => typeof window !== "undefined" && reducedMotion()

export const clamp01 = (x: number) => Math.min(1, Math.max(0, x))

/** Minimum-jerk travel: starts and ends at rest. */
export const smootherstep = (x: number) => {
  const t = clamp01(x)
  return t * t * t * (t * (t * 6 - 15) + 10)
}
