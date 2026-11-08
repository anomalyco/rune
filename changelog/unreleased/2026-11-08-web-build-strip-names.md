- **Web release builds drop the wasm's function-name section.** `scripts/web-build.sh` kept the
  `name` section in release builds, over 40% of an unoptimized wasm. A release build now passes
  `--remove-name-section` to wasm-bindgen: on a mid-sized app the wasm went from 10.9 MB to 6.2 MB
  raw and from 1.83 MB to 1.66 MB with Brotli, and the first frame came about 70 ms sooner on a
  20 Mbps link. `--dev` builds keep the names so panics stay readable, and the new `--names` flag
  keeps them in a release build for profiling (`--opt` then passes `-g` to wasm-opt).
