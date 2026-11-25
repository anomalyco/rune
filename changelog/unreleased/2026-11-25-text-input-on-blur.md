- **`TextInput`, `TextArea` and `TextField` take `on_blur`, and any element `El::on_focus_lost`.**
  It runs when the focused editor loses focus: the window losing focus (another app, a hidden
  window), Tab, a click elsewhere, or `Tx::focus` moving focus off it. A form can commit a
  half-typed value without matching on `PhysicalInput::FocusLost` itself. Return stays
  `on_submit`. It doesn't run for a disabled field or for a field removed while focused. The view
  builds again with what the handler changed, and the keyboard is untouched: the element's
  `on_physical_input` still hears `FocusLost`, and held modifier keys still send their
  `Key::Modifier` releases.
