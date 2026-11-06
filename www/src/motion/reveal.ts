// Scroll-in entrances: anything marked `data-reveal` that starts below the fold fades in once as it enters. What's
// on screen at load is already there, so the first paint never animates.

export function attachReveals(root: ParentNode = document) {
  const els = [...root.querySelectorAll<HTMLElement>("[data-reveal]")]
  const fold = innerHeight
  const io = new IntersectionObserver(
    entries => {
      for (const e of entries) {
        if (!e.isIntersecting) continue
        const el = e.target as HTMLElement
        el.dataset.reveal = "in"
        io.unobserve(el)
      }
    },
    { rootMargin: "0px 0px -12% 0px" },
  )
  for (const el of els) {
    if (el.getBoundingClientRect().top < fold) continue
    el.dataset.reveal = "out"
    io.observe(el)
  }
  return () => io.disconnect()
}
