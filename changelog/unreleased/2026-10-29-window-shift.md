- **A virtual list's window shift no longer re-runs the view.** While a list scrolls, the
  commits that only mount and unmount rows reuse the last view build and build only the
  `virtual_rows` rows new to the window, as long as no handler, poll or update ran, nothing
  was invalidated and every `Cx` read the view made still holds. A view whose output depends on
  something outside its state and its `Cx` reads (a global, a clock) and that relied on those
  commits to refresh now waits for a real change; `Runtime::invalidate` forces one.
