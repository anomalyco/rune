- **`El::fold_glide(spring)`: a growth-stack row that folds like a disclosure.** Its height
  changes glide on its own spring in place of the stack's, for the spring's own time however far
  they go (no `WHOLE_REACH` stretch, no `Tx::motion` floor), and keep their velocity when
  reversed mid-glide. A change taller than the window moves only the window's height on the
  spring and lands the rest at once beyond it, so a 2,000 pt group shuts as fast as a 200 pt one.
  Its feather rides the spring and ends with it. Implies `El::arrives_whole`. The growth-stack
  builders (`growth`, `streaming`, `arrives_whole`) moved from `el.rs` to `el/growth.rs`.
