- **The strobe performance tool can drive and measure Rune apps.** With the new `strobe`
  feature and `STROBE_BRIDGE` set, an app connects to strobe once its windows exist and serves
  queries, waits, clicks, typing, chords, scrolling, window actions, snapshots and frame
  recording, with input entering through the same path an OS event takes. `--strobe` serves the
  same protocol headless on a virtual clock. Off, nothing is compiled; on and unset, nothing
  runs. The frame profile's Windows CPU column now counts thread cycles instead of 15.6 ms
  `GetThreadTimes` ticks. README, "strobe bridge".
