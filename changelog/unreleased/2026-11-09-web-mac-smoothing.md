- **Web text in a Mac's browser has the page's weight.** `FontSmoothing`'s default on the web is
  now `On` when the browser runs on a Mac (not an iPhone or iPad), since Safari, Chrome and
  Firefox draw the page's own text with macOS font smoothing there. Against Chrome's Inter on a
  Mac, Rune's text had 12–14% less ink on a light page and 9–12% less on a dark one, at 1× and
  2×; now it's within 3%, with the same share of solid pixels and edge contrast. Elsewhere it
  stays `Off`. `App::font_smoothing` still picks either.
