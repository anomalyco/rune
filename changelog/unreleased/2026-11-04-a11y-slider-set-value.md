- **Assistive technology can set a slider's value.** A slider with `El::on_step`, `range_value` and
  `range_step` now takes UI Automation's RangeValue SetValue, AT-SPI's Value and VoiceOver's value
  changes: Rune calls `on_step` with the number of steps between its value and the one asked
  for. README, "Accessibility".
