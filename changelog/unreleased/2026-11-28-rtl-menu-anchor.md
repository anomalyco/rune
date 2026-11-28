- **rune-kit menus open at their anchor in a right-to-left window.** `Menu::with_anchor_rect` and
  `Menu::at` take the window's own coordinates, but the overlay layer mirrored them under
  `El::dir(Rtl)`, so a menu opened from a button at the right of the window appeared at its left.
  A drawn pop-up menu (`Menu::from_popup_menu`) placed over an element now opens from the
  element's start: its left edge, or right to left its right edge.
