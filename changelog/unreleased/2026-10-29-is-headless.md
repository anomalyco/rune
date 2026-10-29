- **`rune::is_headless()`, and `--script` alone runs headless.** `is_headless()` answers from the
  process's arguments, parsed once, whether the run renders offscreen, so an app can skip a tray,
  global shortcuts or network work before `run` without scanning argv itself; it leaves
  `extra_args` for an app that passes `App::args`. A window never replayed a script, so
  `--script FILE` without `--snap`, `--video` or another output used to open a real window and
  ignore the script; it now replays it headless like `--frames` (`--native` still opens the window).
