- **Linux: `org.freedesktop.Application`, opened files and activation tokens for
  `single_instance`.** The first process now also owns its id on the session bus (when it's a
  valid bus name, with the default `dbus` feature) and serves `org.freedesktop.Application` at `/`
  plus the id with dots as slashes: `Activate` and `ActivateAction` reopen the app
  (`App::on_reopen`), and `Open` sends links to `App::on_open_url` and `file://` URIs to
  `App::on_open_files`, which before never ran on Linux. Local `file://` launch arguments (a
  desktop entry's `%U`), the first process's and every later launch's, now go to
  `App::on_open_files` too; before, they went to `App::on_open_url` as links. A
  `DBusActivatable=true` entry named `<id>.desktop` with a session service file reaches the
  running app. A later launch's activation token (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`,
  or the call's `activation-token`) now activates the window it brings forward (also one already
  showing) or the next window the app opens, within 20 s, and a process started for a launch
  that came while the app quit gets it as `XDG_ACTIVATION_TOKEN`; before, a reopened window on
  Wayland only asked for attention.
