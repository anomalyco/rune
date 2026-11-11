- **A closing shortcut host leaves a later host's handlers alone.** Closing used to clear every
  handler in the App, so a host that attached while the previous one was closing could
  register a chord successfully and then have it do nothing when pressed. Each registration now
  records the host routing handed it to, and a close clears the handlers of every host up to the
  latest one it ends.
