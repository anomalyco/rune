- **`SegmentedControl::badge(i, label)` puts a small badge after a segment's label.** The badge
  is kit's `Badge` (key `badge`), 6 points after the label inside the segment's padding, and
  counts toward `fit()`'s equal widths; the segment's accessible label reads "Label, Badge".
  Segments without a badge are unchanged.
