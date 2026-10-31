- **A video scrolling across a clip edge no longer commits.** A list holding a video re-ran its
  views each time the video scrolled into or out of sight, because only drawn videos counted
  toward the state views read. Every video counts now, drawn or not, so a pure scroll stays
  live-only. Frames are still pulled only while the video draws, and one scrolling back into
  sight shows its current frame.
