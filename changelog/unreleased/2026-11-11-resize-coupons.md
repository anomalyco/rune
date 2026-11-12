- **A window's size request for the size it already has no longer claims the next user resize.**
  On platforms that resize at once, a jump request holds a coupon for the one `Resized` it
  causes. A request for the pixels the window already has gets no reply, so its coupon used to
  stay pending and make the user's next resize lay out as a step of an animation; the request
  now holds one only when it asks for different physical pixels, rounded at the window's scale,
  so a sub-point request that really moves the window keeps its coupon. A reply that changes
  nothing, which the host returns early on, now spends the coupon it answers.
