- **`El::fold_glide(spring)`: a growth-stack row that folds like a disclosure.** Its height
  changes glide on its own spring in place of the stack's, for the spring's own time however far
  they go (no `WHOLE_REACH` stretch, no `Tx::motion` floor), and keep their velocity when
  reversed mid-glide. Only what shows of the row moves: shutting, the part below the window goes
  at once; opening, the spring carries the row to the window's edge and the rest lands there, so
  a 2,000 pt group shuts as fast as a 200 pt one. Its feather rides the spring and ends with it.
  Implies `El::arrives_whole`. The growth-stack builders (`growth`, `streaming`,
  `arrives_whole`) moved from `el.rs` to `el/growth.rs`.
