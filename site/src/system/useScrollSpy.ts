import { useEffect, useMemo, useState } from "react"

/** How much of an element has to be on screen to count as in view: enough to read its heading, or all of it. */
const IN_VIEW_PX = 48

/** Scroll position against a list of element ids, measured once per frame. `active` is the last element whose top has
 *  passed a line a third of the way down the viewport. `inView` is every element showing at least a heading's worth
 *  on screen, in order, falling back to `active` when none is. Both are empty on the server and during hydration, so
 *  the first client render matches the prerendered HTML. */
export function useScrollSpy(ids: string[]): { active: string | null; inView: string[] } {
  const [active, setActive] = useState<string | null>(null)
  const [visible, setVisible] = useState("")
  const key = ids.join(" ")

  useEffect(() => {
    if (ids.length === 0) return
    let frame = 0
    const measure = () => {
      frame = 0
      const height = window.innerHeight
      const line = Math.min(height * 0.33, 280)
      const atEnd = height + window.scrollY >= document.documentElement.scrollHeight - 2
      let current: string | null = null
      let lastOnScreen: string | null = null
      const shown: string[] = []
      for (const id of ids) {
        const el = document.getElementById(id)
        if (!el) continue
        const rect = el.getBoundingClientRect()
        if (rect.top <= line) current = id
        if (rect.top < height) lastOnScreen = id
        const overlap = Math.min(rect.bottom, height) - Math.max(rect.top, 0)
        if (overlap >= Math.min(IN_VIEW_PX, rect.height)) shown.push(id)
      }
      // At the very end of the page the last item wins, even if it never reaches the line.
      if (atEnd && lastOnScreen) current = lastOnScreen
      setActive(current)
      setVisible(shown.join(" "))
    }
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(measure)
    }
    measure()
    window.addEventListener("scroll", schedule, { passive: true })
    window.addEventListener("resize", schedule)
    return () => {
      cancelAnimationFrame(frame)
      window.removeEventListener("scroll", schedule)
      window.removeEventListener("resize", schedule)
    }
  }, [key])

  const inView = useMemo(() => (visible ? visible.split(" ") : active ? [active] : []), [visible, active])
  return { active, inView }
}
