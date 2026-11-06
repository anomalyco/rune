- **A web page swaps its static export for the live mirror without stopping a flick.** The swap
  ran two frames after Rune's first frame and called `window.scrollTo` whenever an anchor sat
  even a fraction off where the exported CSS put it, which on a phone aborts momentum and jumps
  the page under the reader's thumb. It now waits while a finger is down or a scroll is still
  moving, and only corrects the scroll for an offset over two CSS pixels; a smaller difference
  never touches the page's scroll at all.
