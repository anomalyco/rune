- **Motion a slow frame starts no longer jumps on the next frame.** A frame whose own work ran
  50 ms past its target (a send that commits a long timeline) started its springs at its target
  time, so the next frame charged them the whole overrun and they leapt ahead. Motion a frame
  starts now holds its first pose for that frame's overrun (`FrameClock::presented`, built on
  `AnimationStore::started` and `AnimationStore::delay`), so it moves about one frame next, as Core
  Animation starts what a transaction adds when it commits. Motion already running still keeps
  real time through the hitch.
