- **A document-mode page's canvas scrolls with the page instead of being pinned to the screen.**
  A `position: fixed` canvas was redrawn from asynchronous `window.scroll` events, a frame at a
  time, while iOS's own compositor panned the mirrored DOM at the display's rate: every pan ran a
  frame or two behind, so the page visibly stepped during a flick and selection highlights sheared
  off the glyphs. The canvas is now a band of the document itself — part of the page's flow, as
  tall as the window plus 256 points of overscan at each end, in a clipper as tall as the page and
  moved with `translate3d(0, top)` — so the browser scrolls pixels Rune already drew at its own
  rate, and a frame is drawn only when the window comes within 64 points of the band's edge (and
  never in the middle of an animation). A 4,000-point scroll through static sections now draws
  about a dozen frames rather than one per scroll event, native rubber-banding works at both ends,
  and the mirror, `El::on_visible` and pointer hovering all map through the band so what is on
  screen is what they mean. This applies to `App::web_document` pages only: a full-window canvas
  and an embedded `fit_content_height` canvas behave as before.
