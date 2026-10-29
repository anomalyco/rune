- **`rune::set_app_icon(bytes)` sets the Dock icon from image bytes.** macOS shows an `.icns`, PNG
  or any image `NSImage` reads as the app's Dock and app-switcher icon while it runs, so an app no
  longer needs its own `NSImage` and `setApplicationIconImage` glue. Call it from any thread,
  before `run` (applied once the app finishes launching) or after; it fails on bytes that aren't
  an image and does nothing elsewhere or in a headless run.
