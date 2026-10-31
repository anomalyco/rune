- **`Cx::selection_rects` finds selected text inside a reused `memo`.** A commit that reused
  the memo holding the selection moved its nodes out of the last build before the view's
  layout reads settled, so the boxes came back empty and a menu placed over the selection
  never showed. Since commits under `El::hover_motion_within` started reusing memos, this hit
  every app that sets it on its root. The selection is now read by path before the measure.
