# Changelog

Rune has no releases yet; consumers pin `dev` revs. This lists changes that can move an app's output
without a code change (goldens, layout, input), newest first, with the rev that introduced each.

## dev

- **List arrows stop at the ends.** Up on the first row and Down on the last row of a
  `list`, `tree` or `grid` focus group now stay put, like NSTableView, instead of wrapping.
  Menus, select popups (`listbox`) and segmented or radio groups still wrap.
- **ShortcutRecorder can retry an unchanged chord.** `ShortcutRecorderState::mark_unregistered()`
  says the shown shortcut isn't registered (its launch registration failed, say). Recording
  the same chord then goes through `on_change` again instead of being a no-op, until a change
  succeeds. Without the call, behavior is unchanged.
- **Global shortcuts need a modifier.** `App::global_shortcut`, `register_shortcut` and
  ShortcutRecorder refuse a chord without ⌘, ⌃ or ⌥ (Ctrl, Alt or Super elsewhere), such as
  Space, Enter or ⇧A, which would take the key from typing in every app. F1–F24 still work
  alone. Registration returns `InvalidAccelerator("space needs ⌘, ⌃ or ⌥")`, and the recorder
  shows the refused chord with "Add ⌘, ⌃ or ⌥." without calling `on_change`. Opt out with
  `App::allow_unmodified_shortcuts()` and `ShortcutRecorder::allow_unmodified()`.
  `Shortcut::require_modifier()` checks a chord. Apps that bound a bare key now get an error.
- **A window's first updates can register on a later window.** `run_windows` now builds every
  window's host before any of them opens and drains its queued updates, so an update queued on
  the first window that calls `register_shortcut` or `set_menu` on the second gets `Ok` instead
  of `NotRunning`. Each window's state initializer and startup shortcuts now run before the
  first window opens; windows still open in order.
- **Incremental release builds; benchmarks move to `--profile perf`**. This repo's
  `[profile.release]` is now incremental, so a one-line edit in `rune` rebuilds an example in
  about 1.5 s instead of about 23 s. The new `perf` profile keeps the old release codegen
  (opt-level 3, 16 codegen units, no LTO, not incremental), and `cargo bench` stays
  non-incremental. The runtime benches (`bench-ui`, `bench-startup`, `bench-live`,
  `bench-gpu-springs`, `text-bench`, `native-pacing`) time only `target/perf/` binaries and exit
  with an error on anything else. Tests in the docs now run in debug. Apps that depend on Rune
  use their own profiles and are unaffected.
- **`El::border_gradient(width, top, bottom)`**. A border whose colour runs from `top` at the
  element's top edge to `bottom` at its bottom edge, such as a top-lit hairline rim on a capsule.
  It has `border`'s corners, coverage and antialiasing, and like `border` it doesn't affect layout.
  Whichever of the two is called last wins, and both stops spring like `border`'s colour. Kind 4
  instances (`KIND_GRADIENT`) with a nonzero `params.x` now draw a filled rect with a gradient
  ring (`border` at the top, `uv` at the bottom) and honour `params.z` softness. Rune only emits
  them for this border, but hand-built kind 4 instances that left a border width in `params.x`
  now draw a ring. `rune-ref` matches and its canonical hashes change.
- **macOS Quit runs `on_close`.** The app menu's Quit, Cmd-Q, the Dock's Quit and logout now
  run `on_close` once, and flush clipboard clears, before the process ends, as the close button
  does. A window already hidden under `keep_running` has closed, so its hook doesn't run again.
  `AppHandle::quit` and `Tx::quit` still skip it.
- **Headless `{"physical": "alt+super+space"}` releases its modifiers.** After the key's down
  and up, the script now sends the modifiers coming up, as a keyboard does, so a recorder no
  longer shows ⌥⌘ held in later stills. Scripts whose stills showed held modifiers change.
- **ShortcutRecorder shows the refused chord and lets the app word the error.** After
  `on_change` refuses a chord, the keycaps keep showing it (not just the held modifiers) until
  another modifier starts the next chord, which also clears the error. The message under it
  defaults to sentence case naming the chord ("⌃J is already in use.") instead of the
  `ShortcutError` Display text; `.error_message(|error, chord| ...)` replaces it, with
  `rune_kit::shortcut_error_message` as the fallback, and `ShortcutRecorderState::refused()`
  reads the chord. Recorder text changes; no goldens show it.
- **`AppHandle::shortcut_owner(&Shortcut)`** names who holds a chord after a
  `ShortcutError::Conflict`, when Rune or the desktop knows: this App or another Rune App in
  the process (its title), or on macOS an enabled system shortcut ("Spotlight", "Mission
  Control", or "macOS" for unnamed ones), named at the user's remapped chord. None otherwise,
  and headless runs never ask the desktop. `Conflict` is still a unit variant. Registering a
  chord another Rune App in the process holds now returns `Conflict` on every backend, the
  portal included. No output change.
- **Keyboard focus scrolls into view.** Tab, Shift-Tab, arrows, Home, End and type-select
  reveal the control they reach inside its scroll view, as `Tx::scroll_into_view` does, so
  focus never moves off-screen.
- **Overlay drags report screen travel.** When an overlay moves its own window (`place`,
  `set_placement`, `set_align`) during a drag, `Drag::dx`/`dy` keep measuring pointer travel
  instead of travel relative to the moving window, so a dragged capsule follows the pointer
  without feeding back. New `El::on_hover_change` reports entering and leaving an element's
  rect, children included, without taking their presses or hover; a held press stays inside.
- **Mac keyboard navigation for menus, selects and lists**. Groups with role `menu`, `listbox`,
  `list` or `tree` move with Up and Down only (Left and Right no longer move them), and those
  plus `tablist` jump to the item whose label you type. `El::focus_highlight(color)` gives a
  row one highlight for pointer and keys, as in `NSMenu`: hovering focuses the row (never
  taking focus from a text field) and the first arrow reveals the current or checked item.
  `El::focus_follows()` makes a group press each item it moves to (a source list). A
  non-input `combobox` opens on Up and Down. rune-kit `Menu` and `Select` items use the shared
  highlight, and with `on_close` an open menu or select traps focus, closes on Escape and
  returns focus to its trigger. Text fields take macOS's Control bindings (Ctrl-A, E, B, F, P,
  N, D, H, K). On macOS, with Keyboard navigation off in System Settings, Tab visits only text
  fields and lists, as in AppKit; `Runtime::set_full_keyboard_access` overrides it.
- **Hover lands at once.** A `.hover(color)` overlay (and a hover group's `group_opacity`) used
  to spring in, so a highlight trailed the pointer. Without `.hover_motion`, hover now appears on
  the first frame under the pointer and fades out over 0.1 s, like a native sidebar or menu.
  `.hover_motion(enter, exit)` still sets both curves.
- **App icons on Windows and Linux; aspect-fit images**. `ImageCache::app_icon` now rasterizes
  an executable's or `.ico`'s icon on Windows and a `.desktop` entry's hicolor or `pixmaps`
  icon (PNG or SVG) on Linux, where it used to resolve to an error, so apps that fell back on
  error now draw the icon. `El::pixels_fit(&pixels, SurfaceFit::Fit | Fill)` keeps an image's
  aspect ratio; `El::pixels` still stretches, unchanged.
- **Indeterminate `Progress` animates.** `Progress::new(None)` used to draw an empty track; it
  now sweeps a quarter-width segment across it every 1.4 s, centred and still under Reduce
  Motion. The sweep is `LoopSpec::slide(period, from, to)`, a new looping offset in fractions of
  the node's own size that descendants and hit tests follow. Like other loops it draws no frames
  while hidden, scrolled away or reduced, including when the segment is outside its clip.
- **`--script` taps links by id**. A script event's `"key"` (and
  `Runtime::find`) falls back to the link whose id matches when no element has that key, so
  `{"type":"tap","key":"https://x.dev"}` presses an `El::link` or Markdown link, aiming at its
  first line box. Before, the run stopped with "no element with key".
- **Late children of virtual rows play their entrance**. A child added
  to a row of a `.virtualize(..)` stack after the row first appeared (a "Copied" chip, say) now
  fades in with its transition, as it would in a plain stack, instead of popping in. Rows
  scrolled back into view still mount settled with their whole subtree.
- **Timers drive motion in windowless dev runs**. With the dev panel
  attached and no visible window, a timer that starts a transition now animates it in real time
  on hidden ticks, as a shown window would. Before, the timer's frame left the motion frozen
  until the next input or panel command.
- **Stuck scrolls hold their bottom across fitting and overflowing**.
  When a stick-to-bottom scroll's content goes between overflowing its viewport and fitting it
  while the viewport resizes, the rows no longer jump by the size change and slide back.
  Bottom-aligned rows (`justify_end` in a filling stack) stay on the bottom edge every frame;
  top-aligned rows glide between the bottom and the top. A row added to a bottom-aligned stack
  that fits now pushes the rows above it up as it grows, where they used to jump up first.
  Settled frames and `--snap` goldens are unchanged.
- **Linux portal shortcuts use zbus 6.** zbus 5's zvariant, zbus_names and zvariant_derive are
  flagged unmaintained (RUSTSEC-2026-0370, -0371, -0372) and no longer appear in Rune's default
  dependency graph. Linux builds need Rust 1.89 or newer. The opt-in `native-file-dialog` feature
  still reaches zbus 5 through `rfd`'s `ashpd`, which has no zbus 6 release yet.
- **Steady 60 Hz in Low Power Mode on ProMotion Macs.** Low Power Mode caps the built-in panel
  at 60 Hz while macOS still reports 120 Hz, so Rune drew a few fast frames after each change
  before settling. Windows and overlays on the built-in display now pace at 60 Hz in Low Power
  Mode, with no fast start and about half the input-to-screen latency. `--trace` headers report
  60 Hz there.
- **`LayerSpec` literals need filter fields**. The public `blur` and
  `clip_radius` fields break external `LayerSpec { opacity, blend }` literals. Add
  `..Default::default()` for their previous unfiltered, rectangular behavior.
- **Rounded subtree clips and dormant dissolves**. `clip_rounded(radius)`
  clips ordinary paint directly and isolated output after filtering, preserving blur from sources
  beyond the clip. `dissolve()` skips isolation at opacity endpoints, where overlapping opaque
  children now use source-over; it activates additive isolation during fades, including sequences.
- **Blur filters the whole subtree**. `blur`, `live_blur`, `blur_motion`
  and presence blur now soften text, images and video together. `Transition::blur_replace()` peaks
  at Gaussian sigma 1.5 pt instead of the old shape-only 4 pt coverage radius; its scale and timing
  stay the same. The packed instance remains 128 bytes, with clip radius in attribute 7 `fade[3]`.
- **Opt-in guarded Rust view editing**. `scripts/rune-dev` patches
  Settings and fixture-only OpenCode views while retaining state, scroll and springs.
  Declaration, signature and other build-input changes require a restart. `rune/hot`
  is off by default; measured edit-to-pixels latency remains above the one-second target.
- **`SecretCopy::pending()`** makes a copy that is never written and stays `Pending`, so a
  scene can show a copy in flight without building a `Tx`. No output change.
- **`NativeSurface::set_mirrored(bool)`** flips video horizontally at draw time, for
  front-camera previews. It is off by default, so existing output doesn't change.
- **Exiting password fields drop their secret.** When a secure text input starts its exit
  animation, its copy of the text is wiped and replaced by its mask, so it paints the same dots
  (no placeholder, no caret) through the exit without holding the password.
- **`Tx::copy_secret` returns a `SecretCopy` handle.** `state()` / `holds()` tell whether the
  copy is still the last thing Rune wrote and the clipboard hasn't changed (the worker watches
  the change count while a secret is held), whether a write failed, and whether it was cleared or
  replaced; `clear_now()` clears it early. `Tx::clear_secret_clipboard()` clears the latest
  secret from any window. Tests can share `rune::clipboard::memory()` between runtimes with
  `Runtime::use_clipboard`. The worker now keeps a keyed fingerprint of the secret, not its text.
- **Command chords don't type, and single-line fields stay on one line.** Text typed with Cmd
  (macOS) or Ctrl held, such as Shift-Cmd-Space, no longer inserts into any field; it goes on to
  the app's `on_key` instead. Ctrl+Alt (AltGr) still types on Windows and Linux, and Option still
  types on macOS. Secure and `single_line()` fields drop control characters from typing, paste and
  IME commits: pasted line breaks and tabs are stripped, like `<input type=text>`, and
  Shift/Option-Enter submits instead of inserting a newline. Multi-line fields are unchanged.
- **RollingText sets like one text.** Each rolling glyph sits at its advance in the whole label
  shaped once, with kerning and contextual forms, where each used to round up to a device pixel
  on its own. Rolled labels get narrower and tighter (IBM Plex Mono 11 pt "Copy password": 91 →
  86 pt) and at rest draw the same glyphs at the same positions as the plain string. New
  `El::advance_in(run, at)` measures and draws a text as its grapheme in a longer run.
- **Stuck scrolls hold their bottom while they resize**. A stick-to-bottom
  scroll whose own height animates (a banner opening above it, a footer below) now keeps its last
  row on its bottom edge every frame. Before, the rows jumped by the size change and slid back.
  Settled frames and `--snap` goldens are unchanged.
- **Editing scrolls the caret into view**. Typing, pasting, IME composition
  or caret keys in a field inside a scroll viewport now scroll its nearest viewport just far enough
  to show the caret line, as a browser textarea does. Before, the caret could run off the bottom.
- **Growth rows play an explicit `.transition`**. Rows inserted into a
  growth stack (and elements inserted inside them) used to ignore `.transition(...)` and only grow
  open. One that sets a transition now also plays its entrance; rows without one are unchanged,
  and rows a virtual list remounts on scroll still appear settled.
- **Absolute children size against their parent**. `absolute`/`pin` children
  now resolve `w_full`, `w_frac`, `h_full` and right/bottom pins against their parent's padding
  box, as documented, instead of the window (Taffy hands them to the nearest positioned ancestor,
  and elements were static). Left/top offsets were already parent-relative. Root overlays (scrims,
  dialogs, toasts, anchored popovers) are unchanged; nested ones move, e.g. rune-kit `List::divider`
  now spans its row.
- **Accessory apps get an app menu when showing their Dock icon.** On macOS,
  `set_dock_icon(true)` installs About, Services, Hide and Quit when no main menu exists,
  preserving app-supplied menus and composing with Rune's custom menu items.
- **Headless runs resolve `place()`.** A `--script` object can declare `"displays"` and a
  `"pointer"`; `AppHandle::place` resolves against them (one 1920x1080-pixel display by
  default), prints a `rune-placed` line and adds `"placed"` to `--tree`. A script object
  without `"events"` now means no events instead of an error.
- **Launcher panels never reveal stale pixels.** A launcher shown in the same turn as an
  `update()` renders one fenced hidden frame before its reveal, where it used to show the
  cached pre-update frame for a frame. The update's motion is not settled; it runs on after
  the reveal. Reveals with unchanged state keep the cached fast path.
- **Launcher panels run `poll` on a plain wake.** `AppHandle::wake()` and the setup `Waker`
  used to skip a launcher's `poll` unless an update came with them. A shown launcher now
  redraws; a hidden one polls and renders its cached frame at alpha zero when `poll` reports
  a change, with no frame loop.
- **Linux builds link `libxkbcommon`** (`a7f0f56`). Display-targeted overlays added the `xkbcommon`
  crate, so CI and build hosts without `libxkbcommon-dev` (Fedora: `libxkbcommon-devel`) fail at
  link time. See "Linux system packages" in `crates/rune/README.md`.
- **`.truncate_fade()` fades 1.5 em instead of 1 em**.
  Overflowing labels fade over a wider edge; labels that fit don't change. New
  `.truncate_fade_width(em)` sets another width.
- **Emoji with `U+FE0F` draw in colour.** `❤️`, `☀️`, `✔️` and other characters a text font also
  has use the colour emoji font when the emoji selector follows them; without it they keep their
  text form, as before.
- **The caret and backspace step over whole graphemes.** Arrows, Shift-arrows and Backspace treat
  a ZWJ family, a flag or a skin-toned hand as one character instead of splitting its code points.
- **Inline code chips draw as one rect.** Touching span backgrounds on a line merge even when
  their runs have different fonts or sizes, at the height of the widest run. Markdown's padding
  spaces no longer draw lighter slivers at each end of a code chip.
- **Italics.** `TextStyle::italic()` and `Span::italic` pick the family's italic face (SF's on
  macOS) or lean the upright face 12° when it has none (bundled Inter). rune-kit `Markdown` draws
  `*emphasis*` in italic, so its emphasis runs change pixels; advances don't move.
- **Math alphanumerics and non-Latin scripts render instead of tofu.** Math alphanumerics and
  symbol blocks, plus Hebrew, Arabic, Devanagari, Bengali and Thai, load system fonts the first
  time a text uses them. Text that used these characters changes pixels and advances; Latin-only
  text and the existing goldens don't move. New `El::justify()` spreads wrapped lines to the
  width, from the right in right-to-left text; `Align` behaves as before.
- **Colour emoji** (#7). Emoji fall back to the system's colour emoji font (Apple Color Emoji,
  Segoe UI Emoji, Noto Color Emoji) and draw as colour images from the image atlas, where they
  used to draw as blank gaps. Text without emoji renders as before.

- **Prewarmed overlay reveals preserve elapsed motion.** A hidden commit keeps its animation
  timing when `show()` arrives in the same poll or a later turn: a recent transition reveals
  in flight, an old one at rest. Content, geometry and live values refresh before reveal;
  hidden motion does not drive a frame loop. Initial preparation and ordinary overlay shows
  still settle their first frame.
- **rune-kit menus open instantly**. `Menu`,
  `context_menu`, submenus and `Select`'s list now appear fully on the next frame instead of fading
  and scaling in from 0.96, and close with a plain `.25 / 0` fade (no scale). Menu item highlights
  land the frame the pointer arrives and leave over 100 ms ease-out. No `--snap` goldens move:
  they capture frame 0, where overlays already mount without an entrance.
- **Wrapped centred and end-aligned text aligns each line** (#6). A wrapping `Align::Center`/`End`
  text now centres (or right-aligns) every line inside its wrap width, instead of moving the whole
  block as one left-aligned paragraph. `--snap` goldens with multi-line centred text move.
- **Single-line text fields** (#6). `El::single_line()` keeps a text input on one line: it scrolls
  sideways to keep the caret visible, clips to the field, and rests at the start when unfocused.
  rune-kit `TextInput` (except `multiline`) and `List` search opt in; core `text_input` is unchanged.
- **Scroll views clip their content** (`3333f37`). `El::scroll` now clips its children to its box with a
  hard edge, so overflowing content no longer draws, or takes presses, outside it. An explicit
  `clip_soft`/`clip_fade` still sets the edge. No `--snap` goldens move.
- **Optical sizing on by default** (`2eae09d`, from `edc9892`). Text with an `opsz` axis now uses each
  run's point size, as AppKit does: system SF at 13 pt renders as SF Text, so labels are slightly
  wider. Layout is otherwise unchanged, but `--snap` goldens with SF text move (Scrawl: all 7, PAE
  ~0.85). Opt out per style with `.optical_sizing(OpticalSize::FontDefault)`. Fonts without `opsz`
  are unaffected.
- **Presses inside scrolled views land** (`b3c7bdb`, #5). A captured press is released against
  where the element was drawn (after scrolling), not its unscrolled layout rect, so `on_press` now
  fires for rows scrolled into view.
