- **One-shot sequences can release back to the element's own pose when they end.**
  `Sequence::release()` opts a one-shot `El::sequence` into CSS `animation-fill-mode: none`
  semantics: it plays its beats once and returns the element to its own opacity, scale, offset
  and rotation before any `delay`, after the last beat, under Reduce Motion, or on settle, then
  sleeps with no further frames. Unchanged one-shots still hold their final pose by default.
