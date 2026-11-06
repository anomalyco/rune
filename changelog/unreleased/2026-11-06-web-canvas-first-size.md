- **A web canvas's first frame is sized from both sides of its box.** `Wake::Ready` took the
  canvas's box for its width and kept the browser's reported height, so a browser that reports
  the box in CSS pixels (Chromium's emulated device scale, a phone at 2×) laid out and drew at
  half the height its style then stretched over the screen: about one load in six showed the
  page stretched 2× vertically until the next `ResizeObserver` event. The height now comes from
  the box the same way the width does, except in a window that fits its content, which keeps the
  height it measured.
