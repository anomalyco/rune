- **An opt-in native Metal backend on macOS.** Build with the `metal-backend` feature and launch
  with `RUNE_GPU_BACKEND=metal`, and Rune draws through its own Metal backend, plugged in under
  wgpu's API through wgpu's `custom` backend. wgpu stays the default everywhere: without the
  variable, or without the feature, nothing changes. It covers what Rune itself draws, in windows,
  overlays and headless renders, with the same pixels as wgpu: render pipelines from WGSL, buffers,
  the common 8-, 10-, 16- and 32-bit colour formats, samplers, copies and presentation. It doesn't
  cover yet: `NativeSurface` video (it needs wgpu's backend and says so once on stderr), compute,
  query sets and timestamps, render bundles, indirect and mesh draws, multisampling, depth and
  stencil, more than four colour targets, ray tracing, shaders other than WGSL, and iOS. A
  compositor that needs those stays on wgpu; reaching one on Metal panics with its name rather than
  drawing wrong.
