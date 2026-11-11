- **A finished call no longer keeps a replaced Linux shortcut engine alive.** A call held on to
  its engine while it woke the app, so a new host could get `Conflict` for a chord the engine
  it replaced had registered, both from Rune and from the X server's grab. A call now lets go
  of the engine before that wake, so the old backend closes and its chords are released in
  that order. An engine with a call still running inside it keeps its grabs and its owner
  until that call ends.
