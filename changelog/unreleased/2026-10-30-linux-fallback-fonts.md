- **Linux finds fallback fonts through fontconfig too, and ⎋ draws on stock Ubuntu.** Symbol,
  emoji, CJK and script fallbacks are also looked up among the files `fc-list` names, so fonts
  outside the XDG font directories (NixOS's store paths) are found. DejaVu Sans Mono follows
  DejaVu Sans in the symbol fallbacks, since DejaVu Sans has no ⎋ (U+238B).
