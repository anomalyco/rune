- **An invisible effect layer no longer keeps the frame loop awake.** `FxLayer::is_animating` is
  false for a layer that draws nothing whatever the time: zero opacity, or a zero intensity,
  heat, amount or sparkle count (and a motion blur with no source). Such layers already drew
  nothing but kept an idle window ticking at the display rate. A `PulseStyle::Ping` now reports
  animating until 2.2× its duration, when its head finishes cooling, instead of stopping at 2×
  and freezing the bloom mid-fade.
