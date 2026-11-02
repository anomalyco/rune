- **A Wayland layer overlay commits an input region that changes in a frame it doesn't draw.**
  Hit regions paint nothing, so turning on a full-screen region (`OverlayInput::Regions`) can
  build the same pixels the overlay already shows; that quiet frame presented nothing and the
  new region stayed pending, so the pointer kept reaching the app underneath. The frame now
  commits the surface when its region changed.
