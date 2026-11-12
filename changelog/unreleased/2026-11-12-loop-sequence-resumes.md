- **A finite sequence that cools and is enabled again now carries on from the phase it had
  reached, instead of restarting.** `LoopTrack::enabled` never reset a looping track's phase, but
  a cooling finite sequence was marked done and a later enable started it over at now plus its
  `LoopSpec::delay`, so an element that dropped and regained `El::looping` played its sequence
  again from its first pose. It now keeps its clock: it carries on where it was, one that ran out
  while cooled rests at its end pose, and a sequence that has played out stays put when enabled
  again rather than replaying. A cooled sequence asks for no frames until it is enabled again. A
  changed spec still restarts it, as it did before, so a new key is how an app replays one.
