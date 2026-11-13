- **A closed window is no longer woken by its global shortcuts.** A call that had left the
  engine, or posted its work, could still wake its host after that host closed, and so could an
  activation passed along to it. A press woke whichever window connected last, even one that had
  closed. A host's wake now ends as the host closes, a press wakes the host whose engine it came
  from, and a press for a host that has gone queues nothing. A close never waits for a wake, so
  a wake callback that calls back into shortcuts can't hold up a window's teardown.
