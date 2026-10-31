- **Commits under `El::hover_motion_within` no longer copy every node's visuals.** Each node
  inherited the pair by writing it into its own visuals, which gave every node below such a
  root (the docs suggest setting it on the root) its own copy of the rarely-set visual fields
  on every commit, and sent apply down its shadow and gradient path for all of them. The pair
  now rides beside the visuals. A 2,300-element card's commit drops from 1.9 to 1.6 ms
  headless, and a long board card's committing frames from about 11 to 9 ms. Removed nodes
  that drew nothing are also matched against the last build in step instead of through two hash maps.
