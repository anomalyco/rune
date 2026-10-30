- **Debug builds warn about two slow patterns, once per site, on stderr.** A `virtualize` list
  handed more than 200 rows and four times what it mounts names its path and `.virtualize` call
  site and points to `El::virtual_rows`, which builds only the mounted rows. A handler, `poll`,
  `update` closure, `init` or `on_ready` that spends over 8 ms on the main thread and over four
  times its CPU time warns as "`poll` blocked 263 ms (CPU 2 ms) on the main thread: move IO off
  it with `rune::Latest` or a thread". `RUNE_PERF_WARN=0` silences both; release builds contain
  neither. On macOS `libc` is now a regular dependency for the thread CPU clock.
