- **Key names follow the app's language.** `Label::Key` names each keycap word of more than one
  character (`Ctrl`, `Shift`, `Page Up`, `Space`, `F5`) that `Shortcut::keycaps`,
  `Shortcut::menu_keycaps` and `modifier_keycaps` return, through `rune::set_labels`, so a
  German app writes `Strg+Umschalt+S`. macOS's glyphs stay as they are. A drawn pop-up menu
  now writes an item's shortcut as the platform does (`Ctrl+Alt+0`, `⌃⌥0`) instead of the
  accelerator string (`ctrl+alt+0`).
