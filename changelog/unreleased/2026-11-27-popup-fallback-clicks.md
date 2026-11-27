- **Clicking an item in an in-window pop-up menu picks it** (`App::on_popup_menu`, Linux's
  fallback, such as rune-kit's `Menu::from_popup_menu`). Every frame while the menu was open
  released the pointer as for a native menu, which cancelled the item's press before its release,
  so the click did nothing and the menu stayed open; keys, Escape and clicks outside worked. A
  native menu still releases the pointer as before.
