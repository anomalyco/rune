- **The web mirror joins a row of texts into one line (t1478).** A view that draws a line as one
  text per token (a code editor that animates its tokens) mirrored every token as its own
  paragraph, so screen readers read it a token at a time, Find in page missed a phrase across
  tokens (`type Port`) and a copied line came out one token per line. A text now joins the text
  item before it when it is a sibling in the same face carrying on the same line, at most about
  half an em after it; a visible gap there becomes a space. Columns set further apart stay
  separate. On a code tour step the mirror drops from 48 elements to 17.
