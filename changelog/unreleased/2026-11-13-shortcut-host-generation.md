- **A replaced shortcut host no longer acts on the host that replaced it.** Each host now
  carries a generation, taken as it installs its engine. A host closing while a newer one
  attached used to take and release the newer host's engine and clear its handlers, so the new
  host's chords went dead and its later calls failed; it now releases only its own engine, and
  clears no handlers once a newer host exists. An activation a replaced engine queued no longer
  runs its handler in the new host's state, and on macOS and Windows a replaced host no longer
  runs calls that wait for the newest host on its own engine. A close from any host still ends
  routing, as before.
