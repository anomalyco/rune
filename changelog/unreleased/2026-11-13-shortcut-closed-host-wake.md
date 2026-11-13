- **A closed window is no longer woken by a shortcut call that finishes after it.** A call that
  had left the engine, or posted its work, could still wake its host after that host closed, and
  so could an activation passed along to it. A host's wake now ends as the host closes, and the
  close waits for a wake already running, so nothing wakes a window once its shortcuts have shut
  down.
