# 39. List markers sized in device pixels

Draft. A question before code: how should a view get a size that is whole in device pixels, not
in points?

## The case

A browser draws a bulleted list's marker (`disc`, `circle`, `square`) as a shape, not as the `•`
glyph. Blink sizes it from the font's ascent in device pixels, in integer arithmetic
(`list_marker.cc`, `RelativeSymbolMarkerRect` and `InlineMarginsForOutside`):

- `A` = the primary font's ascent, rounded to a whole device pixel.
- Width and height: `(A * 2 / 3 + 1) / 2` device pixels.
- Top: `3 * (A - A * 2 / 3) / 2` device pixels below the top of the ascent.
- Left edge: `A * 2 / 3 + 8` device pixels before the item's text.

Each `/` truncates, so the marker's size isn't a fixed number of points: at 13 px Inter it's 5 px
at 1x and 8 px at 1.875x, where 5 × 1.875 would be 9.4. `rune-kit`'s markdown draws `•` as text
(`markdown/render.rs`, `Block::List`). Next to Chromium the dot is lighter and sits 6 px off at 1x,
4 px at 1.875x. It's a small difference (about 120 device px per nested list), but it shows on
every list in an app that matches a web build.

## Option A: the view reads the window's scale

- **API:** `rune::device_scale() -> f32`, readable while a view builds, registered as an input
  the way `memo` inputs are, so a scale change re-runs what read it.
- **Touches:**
  - `crates/rune/src/runtime/commit/apply.rs` (it already has `env.scale`): publish it to the build.
  - `crates/rune/src/overlay/any_host.rs` and `platform/web.rs` (`ScaleFactorChanged`): invalidate
    the readers.
  - `crates/rune-kit/src/markdown/render.rs`: size and place the marker from it.
- **Cost:** a view's output then depends on the monitor. A window moved to another scale rebuilds
  every reader. Kept subtrees and regions that contain one are keyed by the scale too.

## Option B: the renderer resolves a device-pixel rule

- **API:** a marker element that carries its rule, not its size: for example
  `rune::list_marker(Marker::Disc, ascent_pt)`. Layout reserves its box in points; commit or paint
  turns it into whole device pixels with `env.scale`.
- **Touches:**
  - `crates/rune/src/el.rs` and `crates/rune/src/shape.rs`: the marker kind and its builder.
  - `crates/rune/src/runtime/commit/apply.rs` or `runtime/paint/build.rs`: the device-pixel rule.
  - `crates/rune-kit/src/markdown/render.rs`: use it in place of the `•` text.
- **Cost:** the view stays scale-free. The renderer gets one rule that's specific to CSS lists.

## The question

Which one fits the commit design? Option A is general but couples the built tree to the monitor.
Option B keeps the tree as it is and puts a CSS rule in the renderer. Nothing is built until this
is answered.
