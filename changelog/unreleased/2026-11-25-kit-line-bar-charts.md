- **rune-kit draws line and bar charts from app data.** `LineChart` draws one or more series
  over shared x labels, with value ticks, a legend and gaps at non-finite values; `BarChart`
  compares categories, upright over a value axis or `.horizontal()` as rows of name, bar and
  value. Both take their colours from kit tokens, so they follow light and dark themes.
  `.hovered(i)` with `.on_hover(|s, i| ..)` draws a guide and a readout at the hovered index,
  and assistive technology reads each x band or category as a list item with its values
  (`Tue: CPU 30%, Memory 42%`). A gap reads as `–` and never reaches the app's `.format`.
