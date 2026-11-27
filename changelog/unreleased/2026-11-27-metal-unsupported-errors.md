- **macOS's native Metal backend reports errors as wgpu does instead of panicking.** A command it
  doesn't implement (indirect draws, occlusion and pipeline-statistics queries, timestamps inside
  passes, executing render bundles, `clear_texture`, bind groups past 8) is skipped and reported
  as a validation error naming the call and `App::gpu_backend(GpuBackend::Wgpu)`, and a render
  pipeline with depth and stencil or more than four colour targets is made invalid (it draws
  nothing) with the same error. Errors go to the innermost `push_error_scope`, else to the
  handler `Device::on_uncaptured_error` set, which this backend used to ignore, else panic as
  wgpu's default handler does. Rune's windows and overlays install that handler, so on them these
  errors are logged once instead of ending the app. A call that must return an object the backend
  can't make (a pass with depth, a render bundle encoder, a mesh pipeline) still panics naming it.
