- **Focus falls back to a control that stayed.** When a commit removed the field or control that
  held the keyboard and also rebuilt the tree around it, the fallback only compared old paths,
  which building the new tree had already taken from the nodes that stayed, so nothing took
  focus and Tab started over at the top. It now resolves each old node to the node that replaced
  it and hands focus to the next control still in the tree, as it does when a row is deleted.
