- **Pipeline caches on macOS's native Metal backend.** Its devices offer
  `Features::PIPELINE_CACHE` (Rune's own device turns it on): a `wgpu::PipelineCache` keeps the GPU
  code of the pipelines made with it in an `MTLBinaryArchive`, `get_data` saves it, and a cache made
  from those bytes on a later launch makes the same pipelines without compiling them for the GPU.
  vello's compute pipelines on an M5 Max: 7 ms from a saved cache against 0.3 to 0.4 s cold on
  vello's thread pool (16 to 45 ms against 2.4 to 10 s on one thread). A cache's data names its GPU and macOS build; data
  for another, damaged data or another format starts an empty cache and compiles as before, never
  an error. Metal still compiles shaders' source to AIR, which macOS's own shader cache usually
  answers. wgpu's Metal backend has no pipeline caches, so check the feature.
