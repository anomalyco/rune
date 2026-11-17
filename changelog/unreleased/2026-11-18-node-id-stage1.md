- **Commits resolve node identity and subtree containment by `(parent_id, LocalKey) -> NodeId`
  instead of hashing and comparing full path strings.** Each node now carries its parent's
  `NodeId`, an interned segment `LocalKey`, and its DFS `subtree_end` beside `Node::path`,
  moving flatten's linear cursor, focus sync, hover, caret tracking, and input path lookups
  onto integer ids and index ranges while keeping `Node::path` intact for existing path readers.
