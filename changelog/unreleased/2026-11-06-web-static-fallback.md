- **A web page keeps its static HTML when WebGPU fails.** In document mode a GPU error appended
  an opaque full-window `<pre>` over `[data-rune-static]`, wiping out a page that reads and
  scrolls perfectly without Rune. That happens whenever `navigator.gpu` exists but
  `requestAdapter()` finds no adapter: a simulator, Lockdown Mode, or a GPU process that failed
  to start. The error is logged to the console and the app ends without an overlay, leaving the
  static page exactly as the browser found it.
