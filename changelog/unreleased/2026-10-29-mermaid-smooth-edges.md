- **Mermaid flowchart and state edges are smooth curves.** Each edge leaves and enters its nodes
  straight along the rank axis, keeps only the layout's bend points it needs to stay clear of
  other nodes, labels and subgraph titles, and joins them with cubics, so there are no kinks or
  wobbles and arrowheads sit on the line's last tangent. A label breaks the line around it
  instead of sitting on a filled chip (a label left on a subgraph's border still gets one).
  Hovering a node lights up its edges: `DiagramStyle::hover` (`Hover::Off`, `Edges`,
  `Neighbours`, which also rings the nodes they reach) and `hover_color`, or
  `MarkdownStyle::diagram_hover(color, neighbours)`. Each node's hover area is keyed `node{i}`.
