// The page is already here as static HTML; this hands it to Rune, which draws the same page on the GPU and hides
// the static copy once its first frame is up. Without WebGPU, or if the wasm fails, the static page stays.
// Rune's glue loads with the wasm, after the page has painted, not with the page.
const start = () => void import("./wasm/app.js").then(rune => rune.default()).catch(error => console.error(error))

// Once the static page has painted its largest text, so Rune's download never competes with it.
function afterFirstPaint(then: () => void) {
  const type = PerformanceObserver.supportedEntryTypes?.includes("largest-contentful-paint") ? "largest-contentful-paint" : "paint"
  let done = false
  const go = () => {
    if (done) return
    done = true
    observer.disconnect()
    setTimeout(then, 0)
  }
  const observer = new PerformanceObserver(go)
  observer.observe({ type, buffered: true })
  // A background tab paints nothing until it's shown.
  setTimeout(go, 3000)
}

if ("gpu" in navigator) afterFirstPaint(start)
