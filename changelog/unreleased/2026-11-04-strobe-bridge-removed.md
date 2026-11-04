- **The strobe bridge is gone; accessibility replaces it.** The `strobe` feature, `--strobe`
  and `STROBE_BRIDGE` are removed: tools drive a Rune app through the OS accessibility tree and
  posted window input, and read frame timing from the OS. An app that forwarded the feature
  (`strobe = ["rune/strobe"]`) drops it.
