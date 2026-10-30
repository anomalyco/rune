- **`Tx::scroll_center` and `Tx::scroll_center_in` reveal and vertically centre elements, virtual
  rows and `TextHighlight`s.** Passing `"row/mark"` mounts an unrealized virtual row `row` first
  and centres `mark` inside it, gliding on the scroll spring (`scroll_center_instant` and
  `scroll_center_in_instant` jump, as `Tx::instant` does). A centring sent mid-glide keeps the
  glide's speed without carrying past its new stop. `Tx::scroll_into_view` takes `"row/mark"`
  too, and `Tx::scroll_key_to` also resolves `TextHighlight` keys.
