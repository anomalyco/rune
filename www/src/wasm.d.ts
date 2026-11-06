// src/wasm/ is built by scripts/build.ts and never committed. This types its entry point until it exists.
declare module "*/wasm/app.js" {
  const init: () => Promise<unknown>
  export default init
}
