- **Windows: an animating overlay paces its frames with its swap chain's room, as windows do.** An
  overlay drew and presented every turn with nothing bounding the frames in flight, so a swap
  chain's queue grew while it animated (on NVIDIA, tens of MB a second and several GB in all) and
  hiding or parking it later waited in wgpu-hal for that queue to drain, freezing the window. Its
  live surface now owns a `window::slot::Slot`: before a frame it takes the room, and when there is
  none it skips the turn until a thread-pool wait wakes the event loop (`WakeEvent::FrameSlot`), so
  the frame is built from what changed meanwhile. Waiting isn't a missed frame and arms no timer,
  and a park or release with a wait outstanding unregisters it before the surface goes. Overlays
  elsewhere draw as before (#887).
