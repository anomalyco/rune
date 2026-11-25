- **rune-kit `Kit::metrics` sets component metrics fixed in code until now.** `Metrics` has
  push `Button` boxes per `ControlSize` (height, horizontal padding, radius, icon; `ButtonMetrics`
  in a `BySize` table), compact `Switch` tracks and knobs per size (`SwitchMetrics`), the boxed
  field's height, horizontal padding, radius and focus outline for `TextInput` and `TextArea`
  (`FieldMetrics`; `focus_offset` is measured outward from the edge as `El::outline` takes it,
  so `-focus_width` keeps the ring just inside and 0 or more draws a halo outside; text starts
  at least 6 points in whatever `padding_x` is), `TextArea`'s text style and
  padding (`TextAreaMetrics`), and a checked `Checkbox`'s fill, border and mark
  (`CheckboxColors`, which also hold under the pointer). Every entry is `None` by default and
  keeps today's look; `ButtonMetrics::kit`, `SwitchMetrics::NS_SWITCH`, `FieldMetrics::kit`,
  `TextAreaMetrics::kit` and `CheckboxColors::kit` give the built-in values to start from. A
  `Kit` written as a struct literal needs `metrics: Metrics::default()`.
