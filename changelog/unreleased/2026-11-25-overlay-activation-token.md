- **A layer-shell overlay can hand a program it launches an XDG activation token.**
  `OverlayHandle::activation_token(reply)` asks the compositor for an `xdg_activation_v1` token
  from the overlay's layer surface, with the serial of the user's last key or button press on
  it, so a program launched with `XDG_ACTIVATION_TOKEN` set comes to the front under
  focus-stealing prevention (KWin, Mutter). `reply` runs on the next event-loop turn, before
  that turn's show or hide, so `activation_token(..)` then `hide()` still gets a token. Other
  platforms and backings reply `None`.
