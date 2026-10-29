- **Elements that never animate keep no motion tracks.** A property gets a track only when its
  value first changes, starting from what was last drawn, so a static tree holds none: on a
  60-card settings column (about 5,300 nodes) a commit's apply phase takes about a third less
  time and the heap is 12 MB smaller. A node mounted with the first frame has no
  `prop::PRESENCE` track until it leaves (`Scene::get` still reads 1.0), and fill, text,
  border, shadow, gradient, outline, hover and press follow the same rule, so
  `AnimationStore::track_len`, `has` and `get` see only what has moved. A `Scene::set` or
  `Scene::animate` that leaves a property where it already rests no longer marks the node
  changed.
