- **Programmatic scrolls pick their spring, and an app's own glide holds hover.**
  `Tx::scroll_to_with(key, x, y, spring)` glides like `Tx::scroll_to` on the given spring; a
  request sent mid-glide keeps its speed, as `scroll_to` already did, so ↑/↓ key repeat keeping a
  selection in view scrolls as one motion on a quicker spring than `GLIDE`. `scroll_to` itself is
  unchanged. A viewport an app moves with `Tx::scroll_to_instant` (or `scroll_to_x`, `scroll_to_y`,
  `scroll_key_to`) now counts as moving layout for that frame and the next, so hover under a
  still pointer holds while an app steps its own glide frame by frame, as it does during a glide,
  and resolves once at the pointer after the last step. A single instant jump under a still pointer now
  re-hovers a frame later than before. A held hover whose row the scroll carried out of view lets go at once
  instead of fading where nothing shows. The new `scroll_keys` example and its
  `scripts/motion-audit` scenario hold ↓ at 30 Hz key repeat.
