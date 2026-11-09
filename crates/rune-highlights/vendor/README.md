# Vendored Highlights

- `highlights.wasm`: `dist/highlights.wasm` from the npm package `@pierre/highlights` 0.2.0, unchanged. SHA-256 `cf5fc4d83e7c1ddab19d163de096f787f87efe61504909aa469434f61ef51e2e`.
- `LICENSE.md`: that package's license, Apache-2.0.

`build.rs` translates the module to Rust; `src/langs.rs` copies the package's `languages.js` (names and aliases) and `token-types.js` (Zed capture names). To update, replace both with a new release's, check `Lang` and `Token` against the new lists, and build: an instruction the translator doesn't know fails the build and names it.
