- **A shortcut call wakes the host whose engine ran it.** A newer host installs its engine
  before it takes over routing, so a call already through routing could run on the new engine
  and then wake the host it replaced, which no longer has an event loop waiting. The wake now
  travels with the engine in its slot, so the host that ran the call is the host that wakes,
  and the replaced host's wake is released before the call runs rather than held through the
  new host's callback.
