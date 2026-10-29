- **An overlay whose compositor animates over a still view no longer steps or rebuilds the view
  each frame.** When the last frame asked for no other and nothing has changed since (no input,
  state change, timer, live value, poll or playground), a frame only advances `Runtime::clock`
  and `Runtime::t`, then draws the last build again, so a compositor reading the clock still
  animates. The macOS overlay frame also waits on its present thread only once a frame is due,
  reads the panel's occlusion once per loop turn, and doesn't sample the pointer for a
  click-through overlay that follows nothing.
