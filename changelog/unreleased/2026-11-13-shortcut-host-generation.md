- **A shortcut host no longer acts on another host's work.** Each host now carries a
  generation. The newest host is the last to install its engine on Linux, and the last to take
  routing on macOS and Windows. A host closing while a newer one attached used to take and
  release the newer host's engine and clear its handlers, so the new host's chords went dead and
  its later calls failed; it now releases only its own engine, and clears no handlers once a
  newer host exists. An activation runs only on its own host's state: another host delivering
  the App's updates leaves it for that host and wakes it, and an activation from a replaced or
  closed host runs nothing, and a host's activations run in the order they were pressed however
  hosts pass them along. On macOS and Windows only the host routing names takes the calls
  waiting for it, so a host that installs first but routes last still runs them, and a host
  replaced mid-drain leaves them to its successor, which is woken for them. A close from any host
  still ends routing, as before.
