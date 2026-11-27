- **Compute renderers that size their work on the GPU run on macOS's native Metal backend.**
  `ComputePass::dispatch_workgroups_indirect` dispatches with the three workgroup counts an earlier
  dispatch wrote, as on wgpu, where it used to panic. vello draws the same bytes on it as on wgpu's
  Metal backend (area, MSAA8 and MSAA16), so an app that embeds it through `El::effect` and
  `Renderer::draw_with` no longer needs `App::gpu_backend(GpuBackend::Wgpu)`.
