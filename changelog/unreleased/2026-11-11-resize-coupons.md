- **A window's size request for the size it already has no longer claims the next user resize.**
  On platforms that resize at once, a jump request holds a coupon for the one `Resized` it
  causes. A request for the pixels the window already has gets no reply, so its coupon used to
  stay pending and make the user's next resize lay out as a step of an animation; the request
  now holds one only when it asks for different physical pixels, converted as the platform
  converts it (in f64 with winit and Win32, through whole logical pixels on Wayland), so a
  sub-point request that really moves the window keeps its coupon. Such a request still retires
  an earlier jump to other pixels. A reply that changes nothing, which the host returns early
  on, now spends the coupon it answers.
