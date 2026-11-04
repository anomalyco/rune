- **`El::virtual_rows_positional(count, estimated_row_height, build)` keys rows by index and
  stores nothing per row.** Its first commit and a commit that changes `count` cost the rows it
  mounts, however many rows there are, where `virtual_rows` calls `key` for every row and builds
  a set of every key (about 0.35 s at a million rows, on every count change). Row `i` is keyed
  `i`, keeps its measured height by index and is reached with `Tx::scroll_into_view(i.to_string())`;
  a count that grows or shrinks keeps the reader's row in place. For grids and logs whose rows
  are positions, such as a table that is still loading. The `virtual_rows_version` docs now say
  that a count change also rebuilds every key.
