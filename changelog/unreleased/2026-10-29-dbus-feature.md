- **Linux builds can leave out D-Bus.** zbus, tokio and futures-util, which Rune compiled into
  every Linux build, now sit behind a new default `dbus` feature (26 fewer crates without it). It
  covers the desktop's light or dark preference, the system tray, global shortcuts in a Wayland
  session and the FileChooser portal, which `native-file-dialog` now turns on itself. Off, a
  window winit gives no theme draws dark, `Tray::new` returns `TrayError::Unsupported` on Linux
  and a Wayland session's global shortcuts return `ShortcutError::Unsupported`; X11 shortcuts
  keep working. An app that sets `default-features = false` and wants these adds
  `features = ["dbus"]`. Other platforms are unaffected.
