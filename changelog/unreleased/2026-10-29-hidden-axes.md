- **A variable font's hidden axes follow its named instances at the text's weight, as Core Text
  draws them.** SF Mono's regular is wght 400 with YAXS 324.3; Rune set only wght, leaving YAXS
  at the Light instance's default, so code drew thin horizontal strokes (2% less ink than
  CoreText, mean error 0.66 at 2×; now 0.997 ink, 0.14). SF Italic's YAXS follows the same way.
  Fonts without hidden axes (Inter, SF upright) draw as before. `coretext_compare --set mono`
  measures SF Mono against AppKit's monospaced system font.
