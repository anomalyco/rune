- **Typing or streaming into a text no longer fills the layout cache with each edit's copy.**
  The cache kept every version of an edited paragraph as a stale layout until its budget ran
  out: about 100 copies, over 3 MB, of a 600-byte paragraph being typed into, pushing out the
  layouts of views that would be shown again. When a node's text changes into an edit of
  itself (most of it shared at the start and end), the old version's layouts now go at the end
  of that commit unless another node still shows them; text replaced by something unrelated,
  as a reader switching notes, stays cached for when it comes back. The stale budget is now
  counted in estimated heap bytes (3 MB) rather than bytes of text.
