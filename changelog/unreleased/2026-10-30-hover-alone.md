- **`El::hover_alone`: a hover target that keeps the row around it unlit.** While the pointer is
  on an element marked `hover_alone`, the elements it sits inside show no hover overlay and
  their `group_*` styles rest, as if the pointer had left them; back on the row they light
  again. For a face or button inside a hoverable row. Paint only, with no commit.
