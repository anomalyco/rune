- **Resizing a window lays the last tree out again instead of running the view.** When only
  the viewport changed since the last commit (same state, scale and env, no layout or caret
  read that moved), a commit re-solves layout on the nodes it already has and re-breaks their
  text at the new widths, skipping the view and flattening. A view that reads layout
  (`Cx::layout`, `Cx::layout_size`), the caret, a breakpoint or the env still runs when what it
  read changes. Virtual lists keep their mounted rows and lay them out again; the view runs
  only when the viewport nears the edge of the mounted rows, or rows outside the overscan add
  up to more than a viewport. A notes-sized Markdown page resizes in 2.5 ms per step, down from
  3.6 ms, with 42% fewer allocations; a virtual list of Markdown rows in 0.62 ms, down from 0.82.
