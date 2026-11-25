- **`Tx::request_poll()` asks for a poll on the next frame.** `poll` runs at the start of a
  frame, before that frame's handlers, so work a handler left in state for `poll` waited for
  the next input or wake. A handler that queues work now calls `tx.request_poll()` (the same as
  `wake_after(Duration::ZERO)`), and `AppSpec::poll`, `Runtime::poll` and the README's app
  hooks say that `poll` precedes the frame's handlers.
