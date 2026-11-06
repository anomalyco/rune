- **A web page in document mode follows the browser past the top of the page.** The band a
  document's canvas draws was clamped at `scrollY = 0`, so pulling down at the top froze the
  page on the canvas while Safari's own scroll indicator and the mirrored DOM kept bouncing:
  flicking back up hit an abrupt wall. The band takes the negative offset Safari reports while
  it rubber-bands, and the mirror keeps its document position exactly as before.
