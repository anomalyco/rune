- **A web page's horizontal scrollers belong to the browser, and tapping a page no longer drops
  its selection.** Text in `[data-rune-a11y]` had no `touch-action` of its own while the canvas
  beneath it did, so a phone's touch arbitration on most of the page's surface was the browser's
  default; and a `scroll_x` code block was covered by its mirrored `<p>`, so a sideways flick
  never reached the canvas and the block couldn't be scrolled by touch at all. In document mode a
  horizontal viewport now mirrors as its own `overflow-x` container (`pan-x pan-y pinch-zoom`,
  hidden scrollbar) that the browser scrolls natively, at the compositor's rate with momentum and
  axis locking; its `scroll` events set Rune's offset, and Rune's own wheel scrolling moves the
  container back. Taps on the page also stopped calling `focus()` on the canvas (`tabindex="-1"`),
  which used to clear a text selection or a link's focus, though a tap that leaves an input
  focused still opens the keyboard.
