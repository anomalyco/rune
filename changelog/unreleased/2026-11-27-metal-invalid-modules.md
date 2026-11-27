- **On macOS's native Metal backend, a pipeline made from a shader module that failed is an
  invalid pipeline, as in wgpu, instead of a panic.** A module whose WGSL doesn't parse or
  validate (a struct field named `target`, which WGSL reserves, as wgpu also refuses) reports its
  error; a compute or render pipeline made from it then reports its own and dispatches or draws
  nothing. Before, the module's error went to an error scope and the pipeline panicked re-reading
  the module. A compute pipeline Metal refuses to build is invalid the same way.
