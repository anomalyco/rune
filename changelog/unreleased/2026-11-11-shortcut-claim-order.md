- **A shortcut's claim outlives its native grab only until the backend is gone.** Unregistering
  the last chord disclaimed it before the backend closed, so another App in the process could
  take the chord while the old backend still held the desktop grab, and a third could skip the
  process guard entirely. The backend now retires first, then the claim goes.
