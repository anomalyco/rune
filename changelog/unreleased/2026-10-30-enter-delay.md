- **`El::enter_delay` holds a whole entrance.** A new element waits the given seconds at its
  transition's insertion effect, then offset, scale, blur, opacity and reveal play together on one
  progress, so a staggered list (`.enter_delay(Stagger::new(0.05).delay(i, n))`; `Stagger` and
  `StaggerFrom` are now re-exported) no longer shows rows sliding before they fade. It takes its
  place in layout at once, runs on the element's and the window's time scale, and keeps its
  delay under Reduce Motion, where the entrance is the short fade. The first frame, instant and
  settle commits, hidden windows and rows out of sight ignore it; removed while it waits, a row
  that fades in goes at once. A window that only waits for a delayed start, from `enter_delay` or
  a delayed `Anim`, draws nothing and wakes once when it starts, and the store no longer samples
  such tracks every frame.
