- **A numbered line inside a paragraph starts a list.** CommonMark lets only a list starting at 1
  interrupt a paragraph, so `**More juice**` followed by `4. Hash ripple.` on the next line read
  as one paragraph with "4." mid-line. Markdown now ends the paragraph there and starts a list
  at 4; later `N. ` lines in it are items. Escaped markers (`4\.`), markers inside emphasis and
  bare `4.` stay text. Source ranges, find highlights and streaming are unchanged.
