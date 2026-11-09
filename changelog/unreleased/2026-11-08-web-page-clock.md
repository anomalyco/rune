- **Web frames follow the page's clock (t1480).** On the web, `rune::time::Instant` now reads the
  page's current `performance.now()` on every call, instead of the `performance` object it found
  first. A test harness that installs a fake clock after the app has started (Playwright's
  `clock.install`) now drives Rune's animations too, so a paused clock holds a frame and
  `clock.runFor` steps it; before, motion ran on in real time. A clock that jumps back carries on
  from its last reading, so instants never go backwards. `scripts/web-smoke.mjs` checks it.
