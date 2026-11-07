- **Text, border, shadow and gradient colours can be `Live` values.** `El::live_text_color`,
  `live_border`, `live_shadow` (the first shadow's colour) and `live_gradient(top, bottom)` bind a
  `Live<Color>` the way `live_fill` does, smoothed or not, so a colour that tracks data changes
  without rerunning the view, committing or reshaping text. A rebuild keeps what the binding last
  set rather than snapping back to the authored colour.
