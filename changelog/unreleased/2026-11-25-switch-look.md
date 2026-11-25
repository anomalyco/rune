- **rune-kit `Kit::metrics.switch_look` turns a compact or web `Switch`'s sheen and knob rim
  off and gives its track a border.** `SwitchLook` holds `sheen` (the track's white gradient),
  `knob_depth` (the knob's `depth_top`/`depth_bottom` gradient and 0.5 point rim), and
  `track_border` with its off and on colours. `None` keeps today's look (`SwitchLook::kit`:
  sheen and knob depth, no border); `SwitchLook::flat` paints plain fills. Shadows stay in
  `Kit::elevation`. A `Metrics` written as a struct literal needs `switch_look: None`.
