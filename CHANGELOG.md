# Changelog

Rune has no releases yet; consumers pin `dev` revs. This lists changes that can move an app's output
without a code change (goldens, layout, input), newest first, with the rev that introduced each.

## dev

- `RollingText` now remembers at most 512 labels (and 256 KiB of text) per thread, dropping the least recently built; a label that comes back after that starts its roll over (t841).

- Docs: a "Fast apps" page (rules with before and after snippets, the memo lesson) and `scripts/app-audit <repo>`, a quick grep for the slow patterns it lists; no API change.

- **`Tx::scroll_center` and `Tx::scroll_center_in` reveal and vertically centre elements, virtual
  rows and `TextHighlight`s.** Passing `"row/mark"` mounts an unrealized virtual row `row` first
  and centres `mark` inside it, gliding on the scroll spring (`scroll_center_instant` and
  `scroll_center_in_instant` jump, as `Tx::instant` does). A centring sent mid-glide keeps the
  glide's speed without carrying past its new stop. `Tx::scroll_into_view` takes `"row/mark"`
  too, and `Tx::scroll_key_to` also resolves `TextHighlight` keys.

- **`Cx::layout` and `Cx::layout_with` read an element inside `virtual_rows` (or `virtualize`)
  where it draws.** With rows culled above the mounted window, the rect read was the culled
  rows' height too far down.

- A `Tx::launch` flight re-aimed mid-flight (its row or an ancestor moved) keeps the speed it was drawn moving at, including the part a scroll view's correction under it added, instead of slowing for a frame and picking up again. `Scene::shift_travel`, which nothing called, is replaced by `Scene::retarget_drawn`, a `set_frame` that starts the new path where the renderer draws the node and at the speed it's drawn moving. The motion audit reports such a one-frame speed dip as `stall`.

- A `memo` hit no longer deep-clones its subtree: the cached subtree is shared, its flattened nodes and layout subtree are reused when its parent didn't change, and apply skips unchanged reused subtrees. On a chat-sized test window, a keystroke's commit takes about half the CPU and 79% fewer bytes. No API change.

- Growth stacks: a row that moves to a new place and changes height in the same animated commit now slides from where it was drawn, instead of stepping by its height change on the commit's frame. A `Tx::launch` flight whose row (or an ancestor) is moved mid-flight now heads for the new place from where it was drawn, instead of jumping by the move. And when a commit changes the padding, gap or margin above a row's contents (a row that stops sitting close to the one above it), those contents slide from where they were drawn; a sibling's new size still moves the contents under it at once.

- **`Tx::keep_on_screen` holds a stuck header in place.** Shutting a group from its `sticky_top` header while the header is stuck (for example in a `stick_bottom` timeline scrolled into a group taller than the view) keeps the header where it's drawn for the whole commit, and the rows below fold up to it on the growth spring. Before, the header unstuck as its group shrank, the scroll jumped to bring it back, and the rows below dipped hundreds of points while the group's first rows flashed for a frame. Anchors that aren't stuck behave as before.

- **`Latest` gains `set_waker`, `request_if_changed` and `request_opt`.** `set_waker` attaches the waker in `setup` to a `Latest::default().debounce(..)` built in `init`, keeping its debounce, pending request and answer. `request_if_changed(&key, work)` (`K: Clone + PartialEq`) does nothing when the key equals the last such request's, so `poll` can call it every time without restarting work; `request`, `request_opt` and `cancel` forget the key. `request_opt` takes work returning `Option<T>`: `None` lands nothing and doesn't wake the window, but still supersedes older requests.

- `El::anchored` overlays draw at their anchor's zoom: inside a subtree drawn at `El::scale(1.6)`, a popover and its text draw at 1.6 (crisp once still) beside where the anchor draws, growing away from it, with the gap zoomed and flipping, `anchored_fit` and hit testing on the drawn size. An overlay with its own `El::scale` keeps it instead of the inherited zoom, so popovers already wrapped in `.scale(zoom)` by hand don't zoom twice. One that scales a child inside it instead needs `El::anchored_unscaled()`, which keeps an overlay at window scale.

- `El::help_when_truncated` now shows its tooltip when an `El::truncate_fade` text runs past its box, as it does for `truncate` and `max_lines`, so a faded label no longer needs a "…" to get its tooltip. The fade still draws no ellipsis.

- **`rune::Latest<T>`: latest-wins background requests.** `Latest::new(waker)` (the `setup`
  waker), optionally `.debounce(wait)`; `request(key, |key| work(key))` from a handler runs the
  work on a small shared worker pool (up to four threads, which exit when idle) and supersedes
  every earlier request: one not yet started never runs, and a stale answer is dropped even when
  it finishes last. The landed answer wakes the window once and `take()` returns it once, so
  `poll` returns true only when it changed. `cancel()` and dropping the `Latest` stop its
  requests; `is_pending()` drives a spinner. On the web the work runs on the main thread from
  `spawn_local`. The debug stall warning points here.

- A drag in a selection group reaches only text that shows: text clipped out of sight but laid out under the words (a folded part above them) no longer takes the drag, which selected everything between it and the press.

- Growth stacks and virtual lists: rows an animated commit puts in a new order slide from where they were drawn into their new places on the layout spring (or the transaction's motion), instead of jumping there in one frame. Only rows whose order changed move; inserts, removals, window shifts, instant commits, Reduce Motion and rows out of sight before and after still land as before. A virtual list read from its top no longer re-anchors to the row that moved.

- **Typing or streaming into a text no longer fills the layout cache with each edit's copy.**
  The cache kept every version of an edited paragraph as a stale layout until its budget ran
  out: about 100 copies, over 3 MB, of a 600-byte paragraph being typed into, pushing out the
  layouts of views that would be shown again. When a node's text changes into an edit of
  itself (most of it shared at the start and end), the old version's layouts now go at the end
  of that commit unless another node still shows them; text replaced by something unrelated,
  as a reader switching notes, stays cached for when it comes back. The stale budget is now
  counted in estimated heap bytes (3 MB) rather than bytes of text.

- **Resizing a window lays the last tree out again instead of running the view.** When only
  the viewport changed since the last commit (same state, scale and env, no layout or caret
  read that moved), a commit re-solves layout on the nodes it already has and re-breaks their
  text at the new widths, skipping the view and flattening. A view that reads layout
  (`Cx::layout`, `Cx::layout_size`), the caret, a breakpoint or the env still runs when what it
  read changes. Virtual lists keep their mounted rows and lay them out again; the view runs
  only when the viewport nears the edge of the mounted rows, or rows outside the overscan add
  up to more than a viewport. A notes-sized Markdown page resizes in 2.5 ms per step, down from
  3.6 ms, with 42% fewer allocations; a virtual list of Markdown rows in 0.62 ms, down from 0.82.

- **A dimmed box with a solid fill fades as CSS `opacity` does.** `El::dim`, which disabled
  kit controls use, no longer lets the box's outer shadows show through its faded fill, and
  a fill under a gradient (a contrast button's sheen) fades with it as one, still without a
  layer. A disabled contrast button on white now paints the browser's light grey (about 176
  instead of 159 mid-height with the OpenCode theme), and a disabled neutral button no longer
  shows its drop shadow through. A crisp offset shadow under a native material, backdrop blur
  or dimmed box now leaves the interior clear too.

- **Resizable playground columns** (`Playground::widths`): the story list and the right-hand controls column in `App::playground` drag by their separator edges to resize and double-click back to their default widths, persisted in `Prefs` by default for windowed apps or through `Playground::widths(load, save)`.

- **Theme colours authored for sRGB blending are painted as authored now that windows blend in
  sRGB.** A theme with `"blend": {"mode": "srgb"}`, the OpenCode theme's translucent tokens and
  shadows, `Kit::alpha`, `Kit::on` and `Kit::from_palette`'s borders and tints re-solve alpha
  only in a window that blends in linear light (`srgb_blend(false)`), so fills, selected rows,
  disabled controls and status banners look as they did before the sRGB default instead of
  double-compensated. Colours resolve for the blend mode of the window building the view
  (`rune::theme::blends_srgb`, `rune::theme::for_blend`, `rune::theme::with_blend`), a live
  `Tx::set_srgb_blend` re-resolves them (memoised rows included), and
  `rune_theme_opencode::kit` returns the kit for the current mode, so get it in the view.

- **Debug builds warn about two slow patterns, once per site, on stderr.** A `virtualize` list
  handed more than 200 rows and four times what it mounts names its path and `.virtualize` call
  site and points to `El::virtual_rows`, which builds only the mounted rows. A handler, `poll`,
  `update` closure, `init` or `on_ready` that spends over 8 ms on the main thread and over four
  times its CPU time warns as "`poll` blocked 263 ms (CPU 2 ms) on the main thread: move IO off
  it with `rune::Latest` or a thread". `RUNE_PERF_WARN=0` silences both; release builds contain
  neither. On macOS `libc` is now a regular dependency for the thread CPU clock.

- Text: underlines sit on whole device pixels, and one that ends its line runs to the text box's rounded-up edge, so a link split into an element a word draws one line with no gaps at the spaces.

- **Elements that never animate keep no motion tracks.** A property gets a track only when its
  value first changes, starting from what was last drawn, so a static tree holds none: on a
  60-card settings column (about 5,300 nodes) a commit's apply phase takes about a third less
  time and the heap is 12 MB smaller. A node mounted with the first frame has no
  `prop::PRESENCE` track until it leaves (`Scene::get` still reads 1.0), and fill, text,
  border, shadow, gradient, outline, hover and press follow the same rule, so
  `AnimationStore::track_len`, `has` and `get` see only what has moved. A `Scene::set` or
  `Scene::animate` that leaves a property where it already rests no longer marks the node
  changed.

- `ElementInfo` implements `Default` (a plain, opaque `el()` at rest) and adds `ElementInfo::stub(key)`, so a test that stands in for `Runtime::elements` can write `ElementInfo { focused: true, ..ElementInfo::stub("send") }` and keep building when Rune adds a field.

- **`rune::is_headless()`, and `--script` alone runs headless.** `is_headless()` answers from the
  process's arguments, parsed once, whether the run renders offscreen, so an app can skip a tray,
  global shortcuts or network work before `run` without scanning argv itself; it leaves
  `extra_args` for an app that passes `App::args`. A window never replayed a script, so
  `--script FILE` without `--snap`, `--video` or another output used to open a real window and
  ignore the script; it now replays it headless like `--frames` (`--native` still opens the window).

- `MenuItem::keys_first()` lets the window's own key handling see a menu item's key equivalent before the item does: the focused element, modals and `on_key` get the press first, and the item runs only if none takes it (picking it in the menu always runs it). Without it AppKit hands the chord to the menu before any view, as before.

- **`rune::set_app_icon(bytes)` sets the Dock icon from image bytes.** macOS shows an `.icns`, PNG
  or any image `NSImage` reads as the app's Dock and app-switcher icon while it runs, so an app no
  longer needs its own `NSImage` and `setApplicationIconImage` glue. Call it from any thread,
  before `run` (applied once the app finishes launching) or after; it fails on bytes that aren't
  an image and does nothing elsewhere or in a headless run.

- `Transition::none()`, `SpringPair::NONE` and `Spring::NONE` give a node no motion at all: it shows whole the frame it mounts and unmounts the frame after it's removed, focus returns as with any transition, and the rest of the change still animates. A spring of `Spring::NONE` becomes `Curve::Instant`, so Reduce Motion no longer stretches it into a fade the way it does a 0.001 s spring. The split shell's panes use it.

- **A compositor that asks for `wgpu::Features::IMMEDIATES` can use immediates.**
  `Compositor::features` returning `IMMEDIATES` now also raises the device's
  `max_immediate_size` to the adapter's (4 KB on Metal), so per-draw values set with
  `RenderPass::set_immediates` travel inside the pass instead of in a uniform written through
  `Queue::write_buffer` or `Frame::write_buffer`, which costs a macOS overlay frame another
  command buffer.

- **An overlay whose compositor animates over a still view no longer steps or rebuilds the view
  each frame.** When the last frame asked for no other and nothing has changed since (no input,
  state change, timer, live value, poll or playground), a frame only advances `Runtime::clock`
  and `Runtime::t`, then draws the last build again, so a compositor reading the clock still
  animates. The macOS overlay frame also waits on its present thread only once a frame is due,
  reads the panel's occlusion once per loop turn, and doesn't sample the pointer for a
  click-through overlay that follows nothing.

- On web, `App::canvas_id` mounts onto an existing `<canvas>` without locking `document.body`, `App::fit_content_height` sizes the canvas to its content height, `OsCursor` and `Tx::set_title` apply, macOS `⌘C`/`⌘A` and synchronous `copy` events work on `El::selectable` text, default web fonts include Inter Medium and SemiBold (with an `embedded-fonts` feature to omit them when `AppSpec::fonts` supplies its own), and a hidden semantic DOM layer mirrors headings, inline links and buttons over the canvas for assistive tech and Find-in-page.

- **A variable font's hidden axes follow its named instances at the text's weight, as Core Text
  draws them.** SF Mono's regular is wght 400 with YAXS 324.3; Rune set only wght, leaving YAXS
  at the Light instance's default, so code drew thin horizontal strokes (2% less ink than
  CoreText, mean error 0.66 at 2×; now 0.997 ink, 0.14). SF Italic's YAXS follows the same way.
  Fonts without hidden axes (Inter, SF upright) draw as before. `coretext_compare --set mono`
  measures SF Mono against AppKit's monospaced system font.

- **Structured communication notes in `rune_kit::notes` (`markdown` feature).** `Note::new(src)`
  and `notes::render_note` render `facts`, `compare`, `steps`, `size`, titled/marked code fences
  (`title="…" {2-4}`), and `> [!note]` callouts alongside `Markdown` prose, tables and diagrams.

- **Wayland overlays take right-clicks.** A layer-shell overlay passed on only the left
  button, so `on_secondary_press` never fired on an overlay. Its right button now opens the
  secondary gesture, as a window's does.

- **On Linux, Rune draws on the GPU the Wayland compositor uses.** Rune took the lowest-power
  adapter, so on a desktop with an integrated GPU beside the discrete card driving the display
  (Hyprland on NVIDIA, for one) every window and overlay showed as a black rectangle: the
  compositor couldn't show the integrated GPU's buffers. Rune now asks the compositor for its
  main device (`zwp_linux_dmabuf_v1` feedback) and takes the adapter on the same PCI device,
  falling back to the lowest-power adapter as before, which keeps laptops that drive their panel
  from the integrated GPU on it. `WGPU_ADAPTER_NAME` (part of the adapter's name) and
  `WGPU_POWER_PREF` (`low`, `high`, `none`) override the choice on every platform. A machine with
  more than one GPU prints the choice and its reason to stderr once; `RUNE_GPU_LOG=1` prints it
  anywhere.

- `ConfiguredOverlay::animate_reveal`: showing an overlay no longer settles motion its state started while hidden, so a notification stack's first card plays its enter transition from the first visible frame. Without it an unretained overlay still shows settled.

- **Text: a glyph no longer lands a pixel off when a colour span splits its line.** The pen is snapped to Parley's 1/256 px grid before it floors to CoreGraphics' subpixel steps, so f32 noise from summing advances in a different order (syntax highlighting, rich text spans) can't tip a glyph onto the next step.

- **Mermaid flowchart and state edges are smooth curves.** Each edge leaves and enters its nodes
  straight along the rank axis, keeps only the layout's bend points it needs to stay clear of
  other nodes, labels and subgraph titles, and joins them with cubics, so there are no kinks or
  wobbles and arrowheads sit on the line's last tangent. A label breaks the line around it
  instead of sitting on a filled chip (a label left on a subgraph's border still gets one).
  Hovering a node lights up its edges: `rune_mermaid::render_with_hover` (`Hover::Off`, `Edges`,
  `Neighbours`, which also rings the nodes they reach, and a colour), or
  `MarkdownStyle::diagram_hover(color, neighbours)`. Each node's hover area is keyed `node{i}`.

- **`FontSmoothing` picks how text is rasterised: `On` (the macOS default, CoreText with font
  smoothing on) or `Off` (the default on Linux, Windows and the web: CoreText with font
  smoothing off, as `-webkit-font-smoothing: antialiased` draws).** Both run CoreGraphics'
  glyph pipeline; `On` adds font smoothing's dilation. Set it app-wide with
  `App::font_smoothing` or `WindowSpec::font_smoothing`, or while running with
  `Tx::set_font_smoothing`; each mode keeps its own glyphs in the atlas. Blending is separate
  (`srgb_blend`, on by default). `Runtime::set_font_smoothing` and `Runtime::font_smoothing` do
  the same for a runtime an app draws itself.
- **Breaking: `FontSmoothing::Calibrated` (Rune's previous rasteriser) is removed, and `MacOS` is
  renamed `On`.** Its quarter-pixel pens, 0.1 pt outline
  grow and luminance/scale coverage curve are gone, with `rune::text::glyph_coverage_exponent`.
  `TextSystem::apple_pipeline` returns an `ApplePipeline` and `TextSystem::set_apple_pipeline`
  takes one (no `Option`). The `glyphs_for_layout` and `glyphs_in` `scale` arguments are now
  unused.

- Glyphs rasterize the way CoreText draws them (`FontSmoothing::pipeline`): CoreGraphics' curve flattening, its y autohinter at integer sizes 7–18 px and, for `FontSmoothing::On`, the outline dilation font smoothing applies (radius from the ink's luminance bucket and the ppem, sides and tops grown, bottoms fixed). Masks land within a few levels of CoreGraphics', and glyphs in light and dark ink rasterize separately. `TextSystem::set_font_smoothing` picks the mode for a text system used directly; `TextSystem::set_apple_pipeline` overrides its stages (`ApplePipeline::SMOOTHED`, `UNSMOOTHED`).

- **Glyph pens floor to CoreGraphics' subpixel steps,
  as CoreText's quantisation does.** The step depends on the glyph's size in device pixels:
  fifths of a pixel below 8⅓ ppem, quarters to 11.1, thirds to 16.7, halves to 33.3 and whole
  pixels above, at every scale. Glyphs used to round to the nearest quarter pixel. Measured
  against CoreText, the report paragraph's mean error falls to 0.06 levels at 2× and 0.10 at 1×.

- **Windows and overlays blend in sRGB-encoded space by default on every platform, as Core
  Animation, CoreText and CSS do.** Text drawn at the colour's full coverage (no compensating coverage curve)
  now has native weight in light and dark themes, and translucent fills, shadows and blurs
  composite as AppKit's would. `App::srgb_blend(false)` (or the overlay's) keeps linear-light
  blending. Rune's Metal layer is now tagged sRGB, so a wide-gamut display colour-matches its
  pixels instead of showing them as display-native values.

- **macOS overlays join other apps' full-screen Spaces.** Overlay panels add
  `NSWindowCollectionBehaviorCanJoinAllApplications` to their collection behaviour, so a HUD shows
  over another app's full-screen window on every macOS that has the flag, not only where the
  system already allows it.

- **macOS overlays finish, submit and present off the main thread, and compositors can stage
  buffer writes.** An overlay frame with no system materials, no presents-with-transaction and
  no native video surface now hands its encoder and drawable to a per-overlay present thread,
  which runs `CommandEncoder::finish`, `Queue::submit` and `Queue::present`; the main thread
  waits for it only before it next touches the surface. An overlay animating at 120 Hz spends
  about half the main-thread time per frame it did. `Frame::write_buffer(buffer, offset, data)`
  writes a buffer through Rune's staging chunks and records the copy into the frame's encoder,
  so a compositor's per-frame uniform writes reuse staging memory instead of
  `Queue::write_buffer`'s per-call staging buffer, and land ahead of what it records next.
  A `Queue::write_buffer` made outside the compositor callback, after the frame is drawn, can
  now land in that frame's submission instead of the next one; write per-frame data from the
  callback, or with `Frame::write_buffer`.

- **A scaled layer snaps the same wherever its scale is anchored.** Content within a pixel of
  a layer's anchor snapped to the unscaled grid while the rest of the layer drew exact, and a
  layer's resting correction faded out by how far that layer's own edges moved, so outlines on
  an overlay anchored elsewhere drifted up to 0.2 px off the art beneath them during a zoom.
  The unscaled-grid weight is now one value per layer, taken from the layer's edges. Each box
  lets go of its resting correction over the first pixel that box moves, so content at the
  anchor stays put instead of drifting. A layer counts as moving by how far it moves points
  of the window.

- **Linux builds can leave out D-Bus.** zbus, tokio and futures-util, which Rune compiled into
  every Linux build, now sit behind a new default `dbus` feature (26 fewer crates without it). It
  covers the desktop's light or dark preference, the system tray, global shortcuts in a Wayland
  session and the FileChooser portal, which `native-file-dialog` now turns on itself. Off, a
  window winit gives no theme draws dark, `Tray::new` returns `TrayError::Unsupported` on Linux
  and a Wayland session's global shortcuts return `ShortcutError::Unsupported`; X11 shortcuts
  keep working. An app that sets `default-features = false` and wants these adds
  `features = ["dbus"]`. Other platforms are unaffected.

- **Decoding a large image no longer needs a full-size float copy.** `ImageCache` builds mip 0 by
  copying opaque pixels straight through and premultiplying the rest with sRGB lookup tables, and
  builds each smaller mip from the 8-bit level above it (mip 1 from the source), so the only memory
  it holds is the mips themselves. An 8192×8192 image peaked at 1.5 GiB and took about 1.25 s; it
  now peaks at its 341 MiB of mips and takes 0.15–0.25 s. Mip bytes match the old float chain within
  one level per channel.

- **Scaled content lands on whole device pixels once its scale holds still.** Pixel snapping
  skipped everything under an `El::scale` other than 1, so a canvas zoomed to 1.37 drew its
  boxes at edges like 548.61 px, soft and up to half a pixel off unscaled outlines drawn over
  it. A node with its own scale now heads a layer: while the layer's drawn rect moves (a zoom,
  a pinch, a spring) its content keeps its exact sub-pixel edges, and once it holds still the
  correction to the grid eases in over 0.1 s, so a zoom ends with no pop. When it moves again,
  the resting correction fades out over the first pixel its edges travel. Cached subtrees snap
  at once. `El::pixel_snap(false)` still opts a subtree out.

- **Linux follows the desktop's light or dark preference.** winit reports no theme on Wayland
  (and layer surfaces have none), so windows and overlays there always drew dark. When winit has
  nothing they now read the XDG desktop portal's `org.freedesktop.appearance` `color-scheme`: a
  worker thread reads it once and follows `SettingChanged`, a change asks every window to redraw
  (as a new accent does) and the redraw rereads it, and overlays read it each time they show. The
  first window waits at most 100 ms for the first answer. "No preference", no portal or no session
  bus keep dark as before, and `--theme` and winit's own theme still win.

- **An invisible effect layer no longer keeps the frame loop awake.** `FxLayer::is_animating` is
  false for a layer that draws nothing whatever the time: zero opacity, or a zero intensity,
  heat, amount or sparkle count (and a motion blur with no source). Such layers already drew
  nothing but kept an idle window ticking at the display rate. A `PulseStyle::Ping` now reports
  animating until 2.2× its duration, when its head finishes cooling, instead of stopping at 2×
  and freezing the bloom mid-fade.

- **rune-light allocates GI only when GI runs, and a bigger scene keeps its render targets.** With
  `Settings::gi` off, `LightRenderer` makes none of the GI textures (media, five cascades, fluence)
  and records no empty GI passes when timing; they're made the first frame GI is on.
  `LightRenderer::inspect` returns black views for the GI buffers until then, and `Timings::gi` is
  0 for a frame without GI. Growing past the primitive buffer's capacity now rebuilds only the two
  bind groups that read it instead of reallocating every render target.

- The playground's list and side column keep only the keys they use (arrows, Return, J K, triggers, Tab, Esc, and typing in the filter). Every other key, ⌘-chords and digits included, reaches the open story's `AppSpec::on_key`, so its shortcuts work as soon as it opens from the list, without a click on the stage or ⇧Tab.

- **Periodic loops keep the app's time.** Animation time drops what a covered, idle or stalled window spends, and a `.looping` track kept its phase on animation time, while apps work out `LoopSpec::delay` from their own clock. So two loops created in different commits fell out of step by whatever animation time had dropped between them. A text shimmer's window and the bright copy that slides back inside it stopped cancelling, and the copy sat a fixed distance off its label for every pass. Running periodic loops now also get the time animation time dropped, and a loop a frame's commit creates starts at that frame's presented time, so loops with delays from the app clock stay in step however late each mounts. A newly created loop starts one frame later in its cycle than before; finite sequences are unchanged.

- **Removing an animating track completes it.** `AnimationStore::remove` and `remove_node` (so a node unmounting mid-animation) now emit `MotionEvent::Completed { finished: false }` for an animation started with a completion token, as replacing it already did, instead of dropping the token silently.

- **`Runtime::events` stays small.** A live window, overlay or web canvas no longer formats an input marker (`press …`, `wheel …`, `key …`) on every input unless `--trace` is on, and a headless runtime keeps at most 512 unread markers, dropping the oldest half when it fills. Tests that read `rt.events` after a few inputs see the same markers as before.

- **A steady animating frame allocates no GPU buffers.** Per-frame uploads in `Renderer`,
  `FxRenderer`, `LightRenderer` and `GlassRenderer` go through reused staging chunks (the new
  `rune-staging` crate) instead of `Queue::write_buffer`/`write_texture`, which allocate a fresh
  staging buffer each call on native backends. `Renderer::prepare` writes only the instances that
  changed and writes globals only when the viewport or sRGB flag changes. Its uploads now land
  ahead of the renderer's next draw, recorded into that draw's encoder, or are submitted ahead of
  `draw_range*` and `Renderer::submit`, so submit the encoder a renderer drew into. One dropped
  unsubmitted is detected, and the next frame uploads everything again. On the web, writes still
  go straight to the queue. A headless 2,000-instance frame takes about half the CPU time to
  record and submit (`examples/upload_bench.rs`).

- A `Tx::launch` or `El::matched` flight into a container that is still entering (its scale or fade animating) keeps the still scales above that container, such as an app-wide zoom. It used to drop every ancestor scale, so under a zoom it flew at 1/zoom size and snapped to the zoom when the container's entrance ended. The entering container's own scale is still dropped.

- **`Recording::counts_roll`: an app whose live numbers roll can gate a count that ticks in place.** Set `rec.counts_roll = true` and a count or clock whose words change in one frame ("9s" → "10s") is a `text-swap`, as other words are; a label over its own rolling glyphs still passes. It's off by default, where a tick passes as before.

- **The motion gate finds four more things: `lands-unseen`, `remount`, `unpinned` and `twice`.** `rune::motion_gate::check` now reports an element that travels in view and then its clip hides it the frame it rests (`lands-unseen`), words that start leaving as the same words mount under a path one segment different (`remount`: a row lost its key), a scroll pinned to its end whose content's bottom slides below the viewport with no input since (`unpinned`; an input named `event…` is data arriving and doesn't count), and an element Rune renamed `key#dupN` because two siblings share a key (`twice`). Each has a `RULES` row. An app's gate can fail on these where it passed before: fix the motion, or allow the finding with its reason.

- **New crate `rune-mermaid`: native Mermaid diagrams in Rust.** Extracted from `rune-kit` so
  apps can render Mermaid diagrams (`rune_mermaid::render` with `DiagramStyle`) without pulling
  in `rune-kit` or `pulldown-cmark`, while `rune-kit`'s `mermaid` feature continues to draw
  closed ```` ```mermaid ```` fences inside `Markdown`. Adds `stateDiagram` / `stateDiagram-v2`
  (`[*]`, composite states, `<<choice>>`), theme-adapted node tints from `style` / `classDef` /
  `:::class`, quoted edge labels, and nested brackets inside unquoted node labels.

- **New opt-in crate `rune-register`: self-registration for stories, scenes and knob groups.** A module adds its items to a list from its own file with `register!(LIST: Type = [...])`, the list is declared with `slice!`, and `collect` reads it ordered by module path (so link order doesn't matter); `merge` folds extra items in by name. Nothing in `rune` or `rune-kit` depends on it. It uses linkme (Linux, macOS, iOS, Windows, Android and more); on wasm32 lists are empty, and on any other target linkme lacks the build fails with its compile error.

- `rune-mac` posts system notifications: `notify::post(&Notification::new(id, title).body(..).info(k, v))` goes through `UNUserNotificationCenter` (silent, asking for permission on the first post), `notify::remove(ids)` takes them back out, `notify::on_click(f)` hands back the clicked id and user info, and `notify::app_focused()` / `notify::bring_to_front()` cover "only when the app isn't in front" and "a click opens it". Only a bundled app can post; elsewhere `notify::available()` is false and posting does nothing.

- A child mounted under a parent past half its entrance enters on its own instead of appearing at once at the parent's opacity: a `RollingText` digit changing while its row's fade has a percent left rolls in rather than popping in place.

- **The playground's story list is a tree, filters as you type, and the keyboard is in one column at a time.** Each `/` in a story's path nests a group, as deep as the path goes: `"Prototypes/Onboarding/Option · Pill"` lists the story in Onboarding, inside Prototypes. Group headers are bolder, with a chevron and a count, and each level indents 12 pt. Every level folds, by click or ← → ↵. `Playground::folds(load, save)` names groups by their full paths (`"Sidebar/Filter buttons"`), and saved one-level names still load. New: `Playground::start_folded(true)` starts every group folded except the open story's, which unfold each time the playground opens, including from a link at launch; with it, `folds` saves the unfolded groups instead. `Playground::group_order(&[..])` lists the named top-level groups first, scenes and stories alike, so stories can come before scenes; a name that matches no group is logged in debug builds. When two stories share a name, a lookup by name (`open_playground_at`, `--playground`) now picks the first in list order, not the first declared. A filter field over the list keeps the stories whose path contains every typed word, along with their groups. `/` or a click focuses it; ↓ moves into the matches, Return opens the first match, and Esc clears the field, then leaves it. The keyboard is in the list, the stage or the side column, and a click in a column moves it there. The column that has it shows its selection in the accent, or a ring around the stage, and the hint lists that column's keys. **Behavior change:** 1–9 pick a variant and `side_keys` hooks hear keys only while the keyboard is in the side column (→ or Tab from a story, Esc back), and the stage's `on_key` hears keys only while the stage has the keyboard, except Esc and `Playground::typing`.

- **`TraySpec` and `TrayIcon` are built with constructors, and the tray enums can grow.** `TraySpec { id, title, tooltip, icon, menu }` becomes `TraySpec::new(id, title, icon).tooltip(..).menu(..)`, and `TrayIcon { width, height, rgba }` becomes `TrayIcon::rgba(width, height, pixels)`. Their fields are private, and `TrayItem`, `TrayEvent` and `TrayError` are `#[non_exhaustive]`, so a match on them needs a `_` arm.

- **macOS app icons at small sizes are the app's real icon.** `app_icon_rgba` (and
  `ImageCache::app_icon`) now draws the icon at 256 pixels or more and scales it down. On macOS 26
  and later, asking for a small size directly could return the system's "legacy" rendition, a
  shrunken tile inside a grey rounded frame.

- **`Tx::scroll_into_view` leaves a row taller than the viewport where it fills it.** Revealing
  such a row from above lands its end at the viewport's bottom, as before, but a virtual row's
  re-placement after its rows measure (or a follow frame while the list grows) no longer flips it
  to its start. Before, where it ended up depended on how many placements ran, so one extra
  trailing row could move it.

- **A typed line stays clear of a scroll viewport's faded edges.** After an edit, the focused
  field's caret now stops `El::scroll_margin` short of its viewport's edge, as
  `Tx::scroll_into_view` does and as CSS `scroll-padding` does for a textarea, so the line being
  typed never sits under a `scroll_edge_fade`. The content's own ends still stop flush.

- **A window with placed traffic lights no longer crashes entering full screen on macOS.** Going
  full screen, AppKit observes the titlebar container and puts its own KVO subclass above the one
  `Chrome::traffic_lights` gave it, so the layout hook called AppKit's `-layout` through its own
  class again and overflowed the stack. The hook now finds Rune's subclass in the chain and calls
  the class above it, and the buttons still return to their spot on leaving full screen.

- Add `rune-perf`, an opt-in bounded local recorder (`Recorder::start(Config::new(dir, app))`, `Frame::new(window, seq, outcome).update_ms(..)`) with rolling stats, `perf.json` snapshots, rotating logs and capped warning incidents; it reads procwatch, which gains `procwatch::usage()` (footprint, peak, resident and fds without a thread walk) and `procwatch::alloc::thread_allocated_bytes()` (this thread's requested bytes, `None` without `procwatch::alloc::Counting`).

- **`FrameEncoded::update_ms` and `FrameEncoded::encode_ms`.** A `Drawn` frame's report says how
  long the runtime took to build it and how long encoding took until submit, the numbers a
  compositor gets. Logging frame timing from `App::on_frame_encoded` no longer needs a
  compositor, which would turn off the window's skip of unchanged timer frames. Other outcomes
  report `None`.

- `Tx::launch` leaves from its source under an app zoom or a scrolled view when another commit landed earlier in the same frame (a state update or event just before the key that sends). It used to skip the zoom and scroll then, so the new row started off to one side, sometimes off-screen or right of where it rests.

- **Fills, borders and clips land on whole device pixels.** A box laid out at a fractional
  position (a 1.1 pt gap at 2×) drew soft, half-covered edge rows. Each painted rect now rounds
  its four edges to the device grid at paint time, so edges are hard, boxes that share an edge
  meet with no seam, a 0.5 pt border at 2× is one crisp pixel, and text keeps the same offset
  from its fill wherever the box lands. Layout, hit testing, `Cx::layout_size` and
  `Runtime::elements` keep the exact fractional rects. Motion stays smooth: a moving element's
  correction eases from the one it drew with toward the rounding of where it comes to rest, in
  step with the distance left, so it glides between pixels with no jump at the start or the
  landing; a scroll snaps its content as one
  translation, and a scale fades the snap in over its last pixel. `El::pixel_snap(false)` turns
  it off for a subtree an app moves itself at sub-pixel speeds.

- `TextSystem::layout_rich_with(text, spans, style, scale, max_width, RichLayoutOptions::new().wrap_anywhere())` lays out rich text that breaks inside a word wider than the wrap width (a long URL, hash or token) instead of running past it, as `El::wrap_anywhere()` does for elements; `RichLayoutOptions::align(..)` aligns lines as `layout_aligned` does. Apps that lay out their own text, such as editors, can now wrap an unbroken word.

- **A key press on a resting macOS window draws at once.** A window that had stopped its
  display link started it on input and waited for its first tick before drawing, so the first
  frame after a key, click or paste ran its handler about 12 ms late (median, `--profile perf`).
  When the last frame reached the display at least a period and a half ago, the window now draws
  in the same run-loop turn, and its handlers run about 0.3 ms after the event (median). Frames while
  something moves still wait for the display's tick, and a Low Power cap still spaces them.
  `FrameEncoded::presents_at()` is `None` for such a frame.

- `Tx::launch` and `El::matched` flights under a still ancestor `El::scale` (an app-wide zoom) fly at the size and place they land at. They used to draw unscaled, off the source's text, and snap to the zoom on landing. An ancestor scale that is animating is still dropped.

- macOS: `Chrome::traffic_lights` holds. Since macOS 26 AppKit put the buttons back at (9, 9) whenever it laid the titlebar out on its own (a title or appearance change, a zoom), so they drifted off the spot and out of line with the app's title-bar controls; the titlebar views now put them back after each layout.

- **Icons honour SVG opacity.** `opacity`, `fill-opacity` and `stroke-opacity` on an icon's
  elements now scale that element's coverage, so one `Icon` can hold a graded shape (a faint
  track under an arc, dots fading round a spinner). Before, they were ignored and drew solid.

- `Tx::replay_playground()` makes the open playground story again and runs its `r` trigger, as R does, and `Tx::playground_trigger(key)` runs one of its triggers, so a deep link can open a story and replay it.

- **Separators in application and Dock menus.** New `MenuItem::separator()` puts a separator
  line among an app's custom items, placed with `.menu("File")` or `.dock()` like any item
  (`NSMenuItem.separatorItem` on macOS). Separators need no id, can't be fired and are skipped
  by id/title lookup. Windows and Linux still report `Unsupported` for native menus.

- **The knobs panel's filter is a real text field.** It was a row that drew `Knobs::query` with a fake caret, so ⌘A did nothing and ← → stepped knobs and folded sections while typing. Now `/` or a click starts a single-line `text_input` bound to a private `TextEdit` in `Knobs`, with ⌘A, ← → and ⇧ to select, ⌥ and ⌘ with arrows and Backspace, paste and IME. `query` stays the field's text on every edit (`Knobs::set_query` sets both), and the filter runs as the text changes. While it edits, the field owns ← →, digits, `[` `]`, `/` and Backspace; ↑ ↓ and Tab still move the panel's row, Return stops, and Esc stops, then clears, then closes. Stopping gives focus back to the panel. `Knobs::filter(query)` starts it with text selected, and `side_key` leaves ← → to the field.
- **A text field's selection highlight no longer stretches to a fill behind it.** The highlight joined runs and took any recently drawn fill behind the first rect on a line for a chip, so a field inside a filled pill selected from the pill's edge. Fields draw their selection rects as they are.

- **Wayland overlays take clicks while idle.** A layer-shell overlay reads input from its own
  queue on winit's connection, and winit turns its loop only for its own events, so a click
  on an overlay that wasn't animating waited for an unrelated wake (a timer, the pointer
  leaving) and often landed late or never. A watcher thread now wakes the loop when the
  compositor writes to the display, at most once per read.

- **Overlays can hand focus to another app.** New: `rune::press_activation_token(app_id)`, an
  XDG activation token from the latest pointer press on a Wayland layer-shell overlay, made with
  that press's serial, seat and surface. A notification server emits it as `ActivationToken`
  before `ActionInvoked`, so the app it opens may raise its window. None off Wayland, before any
  press, or when the compositor lacks `xdg_activation_v1`.

- **Filtering a `Combobox` or `PopupList` is instant by default.** A keystroke's rows and the
  popover's height used to crossfade and spring (0.3 s in, 0.24 s out); they now land in the
  frame, and a `PopupList` lands any change to its `items` at once, even in a commit that
  animates the field it follows (`animate_edit_layout`). Opening, closing and the hover
  highlight keep their motion. `.animate_filtering(true)` on either brings the crossfade back.
  New: `El::instant_changes()`, which lands every change inside an element (layout, opacity,
  transforms, presence of what it holds, growth rows) in any commit while its own entrance,
  exit and colours keep theirs. A scroll-into-view that mounts new virtual rows now commits
  them as instantly as the change that asked for it.

- **Filtering a virtual list crossfades what shows, and motion nobody sees ends at once.** A
  removed element that drew nothing last frame used to skip its exit only when it had already
  faded out. It now also skips it when it, and everything it holds, lay out of the window or
  clipped away, so filtering a long list no longer runs dozens of fades under its viewport's
  clip. In a virtual list, rows a change to the rows (a filter, a sort) mounts now enter with
  their insertion transition, and those that land out of view finish it as soon as they draw.
  A change that keeps none of the rows in view no longer anchors the scroll offset to a kept
  row further down, and the rows it removes no longer hold their space: they fade where they
  stood while the matches fade in at the top. A kept row that was out of view and lands in
  view enters too, rather than appearing whole. Rows scrolled into view still mount as they
  are. New: `Scene::enter_now` and `Scene::reenter`.

- `ConfiguredWindowOptions::macos(|native| ..)` amends the AppKit settings a preset made, so `WindowOptions::launcher_panel().macos(|n| n.corner_radius(16.0))` keeps its launcher behaviour.

- **Trailing `RollingText` digits hold still in a width-leading row.** A trailing label's glyphs
  slid on the label's own spring to cancel their row's slide, so in a parent that slides on
  another spring (`width_leads`, say) they drifted 2–3 pt out and back as the label widened. The
  label and its glyphs now ride their parent's trailing edge: `El::ride_trailing()` (and
  `rune_motion::Scene::set_ride_trailing`) keeps an element against its parent's right edge while
  the parent's width animates, so glyphs that stay, leave or enter hold their columns whatever
  springs the rows around them use. The motion audit's odometer scenario adds header rows with a
  usage group, and `analyze.py` no longer splits a motion at a frame or two of rest, so an
  out-and-back through its start reads as a `reversal`.

- **Big shadows and border rings cost a fraction of what they did.** A window-sized panel's
  blurred shadow shaded every pixel under the panel each frame, running the erf shadow math
  where the panel hides it, and a border-only ring shaded its whole empty middle. A shadow
  under a solid box, and a ring with no fill, now draw as four bands around the middle they
  leave blank, and a shadow skips its erf rows well inside the box. Output matches to within one
  colour level; on a 5K window with a large panel the frame pass went from about 13 ms to about 4.5 ms.

- **Playground: `--whole-window`.** A headless `--snap` with `--playground` is now the story's stage only, by default, and `--crop KEY` looks for its key on the stage. `--whole-window` snaps the whole window, list, stage and side column, and lets `--crop` take any key in it, such as `playground-list`, `playground-side` or a group header's `playground-group-N`. `--out`, `--video` and `--strip` frames are still the whole window.

- **A story picks how the playground's stage lays it out.** `Story::stage(Stage::Scroll)` and
  `Playground::stage(scene, Stage::Scroll)` make it a page for content taller than the stage:
  laid out at the stage's width inside a 24 pt margin, at its own height, from the top, and the
  stage scrolls. `Stage::Fill` puts it edge to edge across the whole stage, whatever size is
  picked beside it. `Stage::Center`, the default, lays out as before. `rune::Stage` is new at
  the crate root, so an app that glob-imports `rune::*` beside another `Stage` names one of them.
  The story list now shows each group's story count on its header, and a motion mark on stories
  and scenes with triggers.

- **`Combobox` reads like a command menu.** Its search field sits borderless under a search
  icon, over a divider, with no focus outline (the caret shows focus), and each option shows its
  `detail` in muted text after the label. The result list fits the window like the other
  dropdowns, never shorter than one row, and scrolls with faded edges. New builders:
  `trigger(El)` replaces the default field trigger, `placement`, and `max_height` (192 by
  default). `Popover::padding` sets the body's inset (12 by default; 0 for edge-to-edge bodies).

- **A motion watcher can read every frame cheaply.** `Runtime::motion_elements(&mut out)` fills a
  reused list with what a frame-by-frame watcher needs of each presented element (id, parent,
  drawn and visible rects, opacity, scale, loop phase, a hash of its words and `MotionElement`
  flags) with no strings copied, and `Runtime::element_path(id)` gives its path and key to cache.
  `elements()` is unchanged. Next: the `--trace-all` motion tracer will read the same walk,
  so its flags and the in-app watcher's can't drift.

- **Apps can watch their own frames.** `App::on_frame_drawn(|rt| ..)` runs after each drawn frame
  of a desktop window with the runtime as that frame presented it (`rt.elements()`, `rt.state`),
  and `rune::last_input()` says when the thread last took an input event from the platform, so an
  app can log a motion glitch it sees in real use. Drawing doesn't change.

- **`MacWindowOptions::corner_radius(r)` rounds transparent, borderless macOS windows and
  panels.** The content view's layer clips everything with a continuous curve
  (`.corner_curve_circular()` for a circular arc), so the root view no longer needs to clip
  itself to the radius; a border inside the edge is still the app's. Opaque and bordered
  windows ignore it. Transparent windows with a system shadow now call `invalidateShadow`
  once the first frame after each show, resize, scale change or launcher prewarm reaches the
  screen, so the shadow traces the drawn shape instead of an empty or older frame.

- **Menus show the accelerator's own key on every keyboard layout.** A `MenuItem` or
  `PopupItem` key equivalent is now the character its accelerator names: `cmd+b` is ⌘B on
  Dvorak too, and fires from the key that types "b", where it used to show and take ⌘X (the
  key in QWERTY's B position) away from Cut. `cmd+shift+=` is "=" with ⇧⌘; arrows, Return,
  Delete, Escape, Tab, Space and F-keys use AppKit's characters. The app, named, Dock and
  context menus all follow it, and a custom item with a standard item's key (⌘X) now takes it
  on any layout as it does on QWERTY. New `Shortcut::menu_keycaps()` gives the same label for
  menus and hints; the playground bar and rune-kit's button shortcut tooltips use it.
  `Shortcut::keycaps()` still names the key at the shortcut's position in the current layout,
  for global shortcuts and the shortcut recorder.

- **The playground takes an app's story table.**
  - `Tx::open_playground_at(story, variant)` opens it on a story from app code, by path, name or
    slug, through the same lookup as `--playground`/`--variant` and `Runtime::open_playground`.
    From `App::on_open_url` a link opens straight onto the story at launch. A bad name at runtime
    is logged and opens the list. `rune::playground::slug(path)` makes a story's stable link
    slug (`"Timeline/Send"` is `"timeline-send"`), and every lookup takes one. The `playground`
    module is public for it.
  - `Story::side_keys(view, key)` and `Playground::side_keys(scene, view, key)` are `side` for
    controls that take keys: → or Tab puts the keyboard on them, the view gets `active` to mark
    what the keys move, and the hook gets each key first. Esc and Tab leave; ← no longer does
    there. `rune_knobs::side_key(groups, lens)` is the hook for `rune_knobs::inline`: ↑↓ pick a
    knob, ←→ step it.
  - `Story::new`, `from_real`, `overlay`, `poll`, `trigger`, `variant` and `side`,
    `Play::after`, and `Playground::page`, `trigger`, `variant`, `side` and `side_real` take any
    `Fn` (`FnOnce` for a step) where they took a `fn`, so a story table can build its stories in
    a loop, each capturing its row. Code passing fns compiles unchanged. `Story::trigger_tx` and
    `Playground::trigger_tx` take a step that needs only the `Tx`.
  - Folded groups stay folded after the playground closes, for as long as the app runs, where
    they used to unfold. `Playground::folds(load, save)` keeps them from one run to the next.

- **Children can leave the way the commit that removes them says.** `El::child_exit(t)` gives
  children removed from an element in a commit the transition `t` in place of their last render's.
  `RollingText` uses it so old digits leave the way new ones enter, including on a first count
  down or a value that turns back mid-roll. A trailing `RollingText` now holds leaving digits in
  their columns too, matches from the right like an odometer ("19s" → "100s" keeps only the
  "s"), and keeps its staggered roll spring when its width changes. `rune::text::advance` gives a
  line's unrounded advance.

- rune-knobs: the side panel's keyboard choice numbers its options and 1–9 pick one. Typing no longer filters on its own: `/` (or a click on the filter) starts it, Return or Esc stops typing and keeps the matches, and a second Esc clears them.

- Playground: a trigger's `Play::after` steps land on the same frame on every replay; due times allow for rounding in the running clock, which used to push a later replay's step one frame late.

- **An instant commit no longer lands motion it didn't change.** `Tx::instant` still lands
  every layout, colour and opacity change the commit makes, but a node already springing toward
  a place or value the commit leaves alone keeps springing, so a quick key press during a layout
  spring (a row of chips opening, a selection moving) doesn't snap it to the end mid-flight.
  `Tx::settle` still lands every running track; use it where an instant commit was used to stop
  motion in place.

- rune-knobs: right-click a knob to put it back to its default, and a hovered group header offers Random and Reset for that group (right-click resets it). Clicking anywhere on a choice row puts ←→ on its options, and moving the pointer takes the highlight from the keyboard's row so only one row is lit.

- **`rune_knobs::inline` fills its parent's width, so knobs fit a playground's side column.** Rows lay out from the width they get: a slider's fill and handle are a share of its track instead of math on a fixed 300 pt, and a choice reads its card's laid-out width to sit beside its label or under it (rebuilding only when the width changes). The floating ⌘⇧K panel keeps 300 pt, set by the panel and not the rows; `inline`'s signature is unchanged, and `actions` now fills its parent up to 300 pt. New `Style::light()`. `examples/knobs` hosts a story's knobs in `Story::side` (`-- --playground Knobs`).

- **Tests can drive the playground.** `App::headless_runtime(size, env)` builds a windowless
  runtime with `App::playground` attached. On it, `Runtime::open_playground(story, variant)`
  opens a story as `--playground NAME --variant V` does, `Runtime::playground_trigger(key)` runs
  a trigger as its key does, and `Play::after` steps run as `Runtime::frame` moves the logical
  clock. `Runtime::playground_state::<P>()` reads the stage's state and
  `Runtime::playground_element(key)` finds an element on the stage; `rt.state` stays the real
  state. `Play::clear()` drops the stage's queued steps, a loop's included, so a trigger can
  take over from a running one.

- **A toolbar can float over a text selection.** `Cx::selection_rects()` returns the selection's
  line boxes in window points (empty with nothing selected) and re-runs the view when one moves
  more than half a point, as `Cx::layout` does; a `memo` that reads it is rebuilt when they move.
  `TextSelection::backward()` says the selection was made dragging up or left, so a toolbar can
  open at the pointer's end. `SelectionChange::dragging()` says a press is still making the
  selection, and **`El::on_selection_change` handlers the selection is inside now hear one more
  call when that press releases**, with the same selection and `dragging()` false, so a handler that treated every
  call as a change sees one extra call per drag. `El::keeps_text_selection()` makes a press on an
  element (a Quote button) leave the selection and the keyboard focus as they are, so its
  handler can read the selection, and a drag that starts on it doesn't start a new one.

- **`El::backdrop_blur(sigma)`: frosted glass over the window's own content.**
  `el().backdrop_blur(8.0).fill(tint)` blurs what is already drawn behind the element, inside
  its rounded or continuous corners and its clip, and draws the fill over it as a tint, with the
  border and children on top. It works through plain `rune::run`, follows scrolling, springs and
  opacity, and renders the same headless. The renderer splits the pass at the element, blurs a
  copy with a half-resolution pyramid cut to the element plus three sigma, and composites it
  through the corner mask. A frame blurs at most `MAX_BACKDROP_BLURS` (4) elements; later ones,
  and any drawn into a target without `COPY_SRC` or a caller's own pass (`Renderer::draw_range`),
  draw only their fill. Frames without one are unchanged. The web backend now asks the canvas
  for `COPY_SRC`. `examples/frosted-bar` shows a list scrolling under a frosted bottom bar.

- **Every `RollingText`'s glyphs now slide on the row's width-leading X spring**, not the plain
  label spring, so glyphs that stay hold still against the row's edge while the width changes. This
  moves the glyphs of every rolling label. **A right-aligned `RollingText` rolls new digits in their
  columns:** `RollingText::trailing()` says the label widens to the left, so a glyph that rolls in
  starts where it will rest instead of riding in from the right with the label's slide ("9s" →
  "10s" no longer rolls "10" over the "s"). A trailing label's row and glyphs slide on the plain
  label spring (no width lead), so a container that right-aligns it should slide on that spring
  too.

- **A `RollingText` can roll the way its number moves.** `RollingText::value(v)` is SwiftUI's
  `.numericText(value:)`: while the value rises, new glyphs enter from `from` (digits move up, the
  default); when it falls, they enter from the opposite edge. Text that changes while the value
  holds, or with a value that isn't finite, rolls from `from`. Labels without a value are unchanged.

- **Text selection across virtual rows.** `El::selectable` and `El::selection_group` now keep
  working across unbuilt and scrolled-out rows of an `El::virtual_rows` or `El::virtualize` list:
  the virtual list pins the rows holding the selection's anchor and focus (storing endpoints by
  row identity, relative path and byte offset so they survive rows mounting and unmounting around
  them), Cmd+A and `Tx::select_all_text(key)` select from the first row to the last row of the
  virtual list, `Tx::select_text(key, range)` pins and builds an unbuilt target row without
  scrolling, and Cmd+C / `Runtime::selected_text()` / `Tx::text_selection()` build any unmounted
  rows between the selection's ends on demand (view only, capped at 10,000 unbuilt rows and 4 MiB
  of UTF-8 text) using the same `\t` and `\n` separators as mounted rows.

- rune-knobs: a choice's options can say what they are (`Knob::hints`), shown under the panel's list while one is hovered or picked; an app can describe all its knobs from one table (`Note`, `describe`), which wins over a knob's own label, about and hints; `Knob::describe` and `Knob::hint` read them, and `Knobs::hover` puts the pointer on a knob or option for a still or a test. `Knob` gained a pub `hints` field.

- **Transparent windows and overlays stay clear on Windows, on every GPU.** A surface whose
  adapter can't composite alpha (a Win32 Vulkan surface on AMD's integrated GPUs, any DXGI
  swapchain made from an HWND) fell back to opaque, so an overlay drew a black box around its
  content, silently. DX12 surfaces now present through DirectComposition, which composites alpha
  on any GPU. A translucent surface gets the lowest-power adapter that can composite it, and
  reuses a shared device only if that device can. An overlay that still can't be translucent
  logs it, as a window already did.

- **Playground: folding groups.** A click on a group's header folds or unfolds it, and so do ←, → and ↵ while the header is the list's selection. ↑↓ walk headers and the stories that show; J K skip the headers. Groups stay folded until the playground closes. ← leaves the side column, as Tab and Esc do. `Playground::scenes(|name| ..)` picks which of `AppSpec::scenes` the list shows. Choosing a scene, or a story whose state is new, runs its `'r'` trigger as R does, so its motion plays at once, headless `--playground NAME` included. `Playground::fresh(|real| ..)` starts scenes from the real state, and `Story::from_real(name, |real| .., view)` derives a story's state from it, each time it's made. `Playground::keep(|stage, real| ..)` copies a scene's stage back before R, a switch or Back, so tuning sticks. `Story::variant(name, |p| ..)` and `Playground::variant(scene, name, |s| ..)` add variants, listed in the side column and picked with 1 to 9. `Playground::typing(|s| ..)` gives the stage's `on_key` every key first while it's true. `Playground::dark(|s| ..)` makes the chrome follow the app's own light/dark setting. `Playground::side_real(scene, |stage, real, cx| ..)` draws a scene's side column from both states, and `Tx::keep_playground()` runs `keep` on demand. Headless, `--variant N|name` opens `--playground`'s story on a variant, and `--crop KEY` crops a `--snap` to the element with that key or path, the stage's first; `--crop x,y,w,h` still crops strip tiles.

- **Inline code chips get real padding and a gap from the text around them.** Markdown pads
  the chip `MarkdownStyle::inline_code_pad` points around the code (3.5 by default) instead of
  painting one narrow space each side, and sets two unpainted narrow spaces each side, so what's
  left is a small margin. Code in a link is a chip in the link's colour without an underline.
  Span backgrounds snap to whole device pixels, sit balanced on the cap height rather than a
  mono face's tall line box, and merge across a link's underline instead of leaving a seam at
  the chip's edge. `MarkdownStyle::list(indent, gap)` sets the marker column and the gap between
  items (32 and 4, as before), and `code_block_padding(x, y)` the room inside code blocks and
  diffs (12 by 12).

- **`App::on_frame_encoded` reports each frame of a desktop window.** The hook (also on
  `WindowSpec`) runs on the UI thread with a `FrameEncoded`: its `FrameOutcome` (`Drawn` once
  encoded, submitted and presented, the first frame and a hidden launcher's included;
  `Unchanged` for a timer's frame that drew what was on screen; `Skipped` when the window was
  covered, minimised or occluded or no drawable came), when encoding finished, the window's key,
  a per-window sequence number, and the display link's target refresh when it paced the frame.
  Frames, skips, pacing and redraws are the same with it as without; without it each frame
  checks an `Option`. Headless runs, overlays and the web don't call it.

- **`Cursor::Image` shows an app's own cursor.** `Cursor::Image(CursorImage::png(bytes, hot))`
  takes a PNG drawn at 2x (a 64×64 px PNG is a 32×32 pt cursor) and a hot spot in points,
  clamped into the image. The OS draws it, so a pencil over a canvas keeps up with the pointer
  and reaches the window's edge. It comes and goes like the system cursors under hover, press
  capture, overlays and focus changes. On macOS it's an `NSCursor` sized in points, with the
  window's cursor rects off while it shows; elsewhere it's a winit custom cursor made at the
  window's scale, set before the loop sleeps the first time each image is used. Each image is
  decoded once and its OS cursor cached (32 images, 8 MiB). A PNG that doesn't decode, or is
  over 256 px a side, shows the arrow and is logged once. `Cursor` is still `Copy` and `Eq`;
  two images are equal when they have the same bytes and hot spot, which for the same bytes in
  memory is an address check. New example: `examples/sketch`.

- **`Tx::input_time()` says when a handler's input arrived.** Key, IME, pointer, wheel and
  trackpad gesture handlers get `Some(instant)`: on macOS the timestamp of the `NSEvent`
  AppKit dispatched, converted to an `Instant`, elsewhere the moment the window took the event
  from winit. Input that piles up between frames keeps each event's own time. Timers, polls,
  updates, layout callbacks and input fed to a `Runtime` directly see `None`. Nothing else
  changes; handlers that don't ask pay nothing but a stamp on each queued action.

- **`scripts/mac-app` installs and relaunches a Mac app in place.** It copies a built binary into
  `~/Applications/NAME.app` with an icon, an optional URL scheme for `App::on_open_url` and an
  optional microphone usage string, then starts the new copy before the old one quits, so with
  `rune_mac::remember_frame` the window doesn't blink. `--wait-idle` holds an unattended swap
  while the app is in use and relaunches behind the app in front; `rune_mac::busy` holds it while
  the app is busy. `MAC_APP_SIGN` picks a signing identity (ad hoc by default). See "Relaunching
  a Mac app" in `crates/rune/README.md`.

- **`rune::motion_gate` lets an app gate its own motion in `cargo test`.** `Run` records the
  app's real `Runtime` frame by frame on the virtual clock, `check` lists what a person would see
  go wrong (pops, jumps, lurches, overshoot, flicker, text swaps, motion that never settles or
  keeps waking) against the `RULES` thresholds, and `gate` runs the app's cells in parallel and
  fails on findings its allowlist doesn't explain. `scripts/motion-audit/gate` runs it with a
  cell filter, `--dump` and `--sheet` evidence; `scripts/motion-audit/motion-trace.py` reads an
  app's own frame-cost trace from a real run.

- **`scripts/rune-dev --local` hot-reloads on one machine.** The compiler, the edit guard and the
  app run here with no build Mac, and an edit that needs a restart rebuilds and relaunches the
  app. `RUNE_DEV_WRAP` runs it under a command prefix (default: plain `cargo`).
  `scripts/rune-hot --manifest … --patch … [--port …]` is the same with an app script's flags.

- **New crate `rune-knobs`: live tuning values and the ⌘⇧K panel.** A `Knob` is a named
  slider or choice with a range and a default, grouped by the keyed element it tunes. The panel
  comes as a side panel that pushes the page over (`side`), a floating panel (`layer`), or rows
  for an app's own layout (`inline`, `actions`), such as `App::playground`'s side column.
  Without the `panel` feature, knobs read their defaults and the panel isn't built.
  `examples/knobs` shows it.

- **New crate `rune-mac`: small macOS helpers for apps.** `remember_frame` and `saved_size`
  reopen a window where and at the size it was, `toggle_full_screen` and `is_full_screen` drive
  the green button's full screen, and `launch` and `busy` pair with the new `scripts/mac-app`
  to relaunch a rebuilt app behind the app in front, and to hold a relaunch off while the app
  is busy. Off macOS they do nothing.

- **Scroll viewports measure all direct children.** `El::scroll`, `El::scroll_x` and
  `El::scroll_xy` now measure their content extent across every present direct child instead of
  only the first, so a scroll container holding several rows or tiles directly scrolls to its
  end, follows `stick_bottom`, and reveals later children with `Tx::scroll_into_view` without
  needing a wrapper column or row. Growth-stack tracking (`El::growth`) finds the growth-stack
  child among direct children and warns once in debug builds if `.growth(..)` is placed on the
  scroll viewport itself or followed by trailing sibling elements.

- **Pills in static text.** `El::pill(range, PillStyle)` and `El::pills(..)` draw byte ranges of a
  `text` or `rich` element as the rounded chips a field draws for `Mark::Pill`: chip colour,
  label colour and icon from the `PillStyle`, a label that never splits across lines, and chip
  padding that takes room in the line. Selection and copy treat the label as text, and `El::spans`
  called afterwards keeps the label colours. On a field it does nothing (use `El::pill_style`).

- **An editor the app draws gets the input method: `App::on_ime` and `El::ime_cursor`.** The
  input method was on only while a `text_input` had focus, so dead keys, CJK input, the emoji
  picker and dictation couldn't reach an app-drawn editor. Mark its caret `.ime_cursor()`:
  while it's drawn and no field has focus, the window turns the input method on with the
  candidate window at the caret, and `App::on_ime` (and `WindowSpec::on_ime`) hears
  `Ime::Preedit` with its cursor range, `Ime::Commit`, and the new `Ime::Enabled` and
  `Ime::Disabled`. Plain typing still arrives as keys, so nothing arrives twice; Return and
  Escape mid-composition go to the input method. A focused field keeps the events and a
  focused secure field keeps the input method off. An exhaustive `match` on `Ime` needs the
  two new arms.

- **File drops say where they land: `Tx::drop_point()` and `El::on_drop_move`.** In
  `on_drop_files`, `on_drop_rejected` and `on_drop_hover(true)`, `tx.drop_point()` gives the
  drag's point on the element (its own points, following scroll and transforms, and the
  window's), so an editor can insert a dropped file where it landed. `.on_drop_move(|s, p, tx|
  ..)` hears the point when the target is reached and as the drag moves over it, at most once a
  frame, for a drop caret. Hosts already passed every position (AppKit's at every step, the
  sampled pointer elsewhere); `Runtime::drag_files_to` now also queues a move, and asks for a
  frame, when the drag moves over a target with `on_drop_move`. Nothing changes for targets
  without one.

- **Images: EXIF orientation, and a decoder panic fails only its image.** `ImageCache` turns a
  JPEG upright by its EXIF orientation (a phone photo stored sideways used to draw sideways,
  with its width and height swapped). A panic inside a codec on a malformed file used to end
  the cache's worker, leaving that image pending forever and failing every later one with
  "image worker stopped"; it now fails that image with "image decoder panicked: …" and the
  worker goes on.

- **Dropdowns fit the window, and a click shows no focus ring.** `El::anchored_fit` places an
  overlay like `anchored` and, when it fits on neither side of its anchor, puts it on the side
  with more room and caps its height (or width) to that room, so a scroll viewport inside
  (`.shrink().min_h(0.0)`) scrolls instead of running off the window. rune-kit's `Popover`,
  `Menu`, `Select` and `PopupList` use it above or below their trigger, and their rows scroll
  with faded edges; a `Popover` body now shrinks. Focus shows only after keyboard navigation,
  as CSS `:focus-visible`: Tab, the arrows, Home, End, Page Up and Page Down (and type-select
  that moves focus) show it, while Space, Escape or typing after a click no longer do.

- **`Drag::clicks` counts a run of presses for `on_drag`.** 1, then 2, 3 and on for presses on
  the element each within the double-click interval and 4 pt of the one before (AppKit's
  `clickCount`), the same on every phase, so an app-drawn editor can select by words or lines
  and extend by them as a double- or triple-click drag moves. A press that drags past 4 pt
  ends the run. `on_double_press` is unchanged. Code that builds a `Drag` itself (Rune's own
  only; its private field keeps apps from it) sets `clicks`.

- **`Tx::open_folder(options, on_answer)` and `FileDialogs::open_folder` pick a folder.**
  An Open dialog that chooses an existing folder: rfd's `pick_folder` on macOS and Windows,
  the portal's `OpenFile` with `directory` on Linux. It waits in `--tree` as a `fileDialog`
  of kind `"folder"` and scripts answer it with `file-dialog` as they do a file. New
  `FileDialogKind::Folder`: an exhaustive `match` on `FileDialogKind` needs its arm.

- **`Tx::set_title(title)` sets the window's title** from a handler or `update`, without
  asking for a frame. The window opens with `AppSpec`'s title and, closed and opened again,
  with the last one set. Headless `--tree` output gains a `"title"` field.

- **`Tx::key_base()`: the character a key types with no modifiers.** In a key handler it gives
  the key's character under the current keyboard layout with no modifiers held, from winit's
  `key_without_modifiers` on macOS, Windows and Linux: `Some('a')` for ⌥A, which arrives as
  `Key::Text("å")`, and for AZERTY's A key, where `Tx::physical_key` says `KeyQ`. Bindings by
  letter that follow the layout (as `DefaultKeyBinding.dict` binds ⌥-letters) match on it. A
  script key with a `"code"` takes a `"base"`, else its one-character name pressed without
  Shift or Option.

- **`Player::preload` decodes a clip's first frame before anything draws it.** A clip's poster
  was only requested the first frame it drew, so a page opening on a clip showed an empty box
  for a few frames while a worker opened it. An app that knows which page comes next can now call
  `preload()` on its clips; the first frame that
  draws one shows its picture. It does nothing for audio, once a poster or player has started,
  or headless, where frames decode as they draw.

- A selection highlight also bridges the small gap a chip's icon and padding leave, and runs as tall as the line's tallest text, so words and chips on a line share one rect.

- **`OverlayHandle::set_exclude_from_capture` changes a live overlay's capture exclusion.**
  `OverlayOptions::exclude_from_capture` was fixed when the native window was made, so an app
  that lets recorders see its overlays on demand (a demo mode) had to recreate them. The new
  call flips macOS `sharingType` between none and read-only, and the Windows display affinity,
  on the existing window, shown or hidden; `capture_exclusion` reports the new state.

- Selectable texts flowed side by side (words around a chip) draw one continuous highlight rect per line instead of one per word, so no seams show between them.

- **`rune::text::ink` says where text's ink sits.** Text is placed by its line box and
  advance, so symbols like ↵, ⇥ and ⌘ draw off the middle of a box that centres them.
  `ink(text, style)` rasterizes the text with the renderer's fonts and fallbacks at 8× and
  returns a `TextInk` with the ink's bounds and centre of mass in points from the text's box,
  cached per text and style; `TextInk::optical(pull)` gives a point between the two for
  centring a glyph by eye.

- **Apps receive links opened with them.** `App::on_open_url(|s, url, tx| ..)` (and
  `WindowSpec::on_open_url`) runs on macOS for each `myapp://...` link the system opens with the
  app, including the one that launched it, which runs before the window's first frame: the event
  loop installs a get-URL Apple event handler before it starts (winit 0.30 doesn't forward these)
  and keeps links until a window whose app has the hook starts, then runs them in order as
  `AppHandle::update`s, waking an idle loop. Without a hook the handler goes once launch is over,
  and an app's own handler is left alone. The app's bundle declares the scheme
  (`CFBundleURLTypes`; see the README) and must validate the string. Other platforms never call
  it.

- **A window an `AppHandle::update` opens opens in the same turn.** The window host took the
  other requests (open, show, focus, place, quit) before running the queued updates, so one an
  update made waited for the next wake; a windowless app opening its window from an update
  dropped the GPU it had prewarmed at launch first. Updates queued before launch now also run
  before the window's first frame.

- **`Tx::scroll_key_to` and `El::sticky_top_showing` land a new page right on its first frame.**
  `scroll_key_to(key, at)` scrolls instantly so element `key`'s top sits `at` points below the
  viewport's top, computed from the layout of the commit the handler causes, so it applies
  before that layout draws (no marker, no correction a frame later). `sticky_top_showing(show)`
  is `sticky_top` held with only the element's bottom `show` points in view, measured from its
  height as laid out in the frame that draws it, so a header whose height changes (or a new
  page's taller one) sticks in the right place at once instead of a frame late.

- `Tx::copy_rich` now writes a `ClipboardContent`'s image (PNG and TIFF on macOS, PNG on the web, pixels on Windows and Linux) and its new file list (`ClipboardContent::with_files`, read back with `files()`): one file URL item per file on macOS, so Finder pastes the files, and the file list on Windows and Linux. Windows and Linux write one form per copy: the files, else the image, else HTML with the text. The text may be empty beside an image or files, and isn't written then. Pastes to hooks that take every form carry the files too.

- The Menu key (new `Key::ContextMenu`, from winit, the web and Wayland layer shells; `ContextMenu` in headless `--keys`) and Shift+F10 open the focused element's context menu: its focused link's `on_link_secondary`, else its own or its nearest ancestor's `on_secondary_press`, at the bottom-left of the link or element. A key handler or focused field that takes the key keeps it, and with no handler nothing happens. A `match` on `Key` with a `_` arm is unaffected.

- `El::on_link_secondary` and `Markdown::on_link_secondary` take a right-click (or macOS control-click) on a link's glyphs with its id and point, for a per-link menu. The link takes the press from ancestors' `on_secondary_press` and the text's Copy menu; elsewhere in the text they still get it.

- **`El::on_scroll_offset` follows a scroll viewport exactly.** On a scroll viewport it is
  called with a `ScrollPos` (the offset and how far it can scroll) once at first, then in
  every frame scrolling moves it (the wheel, a glide, a reveal) or its content resizes, after
  that frame's scroll step and before it draws. A
  `Live` set from it shows in that same frame, so a header that shrinks or fades as a page
  scrolls rides the scroll 1:1, where `on_drawn_layout` hears a frame late and not at all
  while a glide runs. `Scene::present` presents values set after `Scene::frame` without
  stepping time. `El::on_layout`, `on_screen_layout` and `on_drawn_layout` moved, unchanged,
  to `layout_watch.rs`.

- Settled elements resnap to the pixel grid when the display scale changes, without waiting for an animation or hover update.

- Scrolling long lists and sticky sidebars spends less CPU on unchanged hover highlights, preserving their enter and exit animations.

- **`Cx::layout_with` and `Cx::layout_size` read what a view needs from an element's rect,
  re-running it only when that changes.** `layout_with(key, f)` returns `f` of the rect
  `Cx::layout` would, and re-runs the view only when `f` of the new rect differs from `f` of
  the old one: `layout_with("feed", |r| r[1] < 0.0)` re-runs a header on the frames the feed
  scrolls past the top, not on every scrolled frame. `layout_size` is `layout_with` the
  `[w, h]`, so a view sizing something from an element inside a scrolling viewport no longer
  re-commits on every scroll frame. Both settle on the first layout as `Cx::layout` does, so
  the first frame has the right value. `memo::layout_with` and `memo::layout_size` do the same
  for components without a `Cx`, and memos that read them are reused while the value holds.
  `Cx::layout` keeps its half-point threshold, but movement below it now adds up until it
  counts instead of being lost frame by frame, and a memo holding a `Cx::layout` read is
  reused through moves below it. Layout reads and `on_layout` report an element's width and
  height as layout sized it, so moving an element no longer changes its size by a rounding
  error.

- **A tall whole row glides in no harder than a short one.** An `El::arrives_whole` row glided
  on the growth spring's fixed duration however far it went, and a critically damped spring
  opens with an acceleration of its distance over its duration squared, so a tall insert (a
  restored turn, a long pasted prompt) and the pinned viewport following it lurched as they
  started. Past `growth::WHOLE_REACH` (300 pt) the glide now lengthens with the square root of
  its distance, so it opens no harder than a 300 pt glide does; shorter glides are unchanged.

- **Markdown: an outlined table's header no longer cuts off the outline's rounded top corners.**
  The header row's fill now rounds by the frame's radius less its inset (and at the bottom too
  when the table has no body rows), as the `table_body` card layout already did.

- **`App::playground`: a storybook built into the window.** ⌘⇧D (Ctrl+Shift+D off macOS, or
  `Playground::shortcut`, `None` to turn it off) shows the app's scenes plus its own
  `Playground::page`s and `Story`s, grouped by the part of their names before a `/`. One shows at
  a time on a plain stage, live and interactive, with a column beside it for the story's `side`
  view and its single-letter triggers. R makes the story's state again and runs its `'r'`
  trigger; `Play::after(secs, step)` queues timed follow-ups on the logical clock, so a replay
  runs the same every time. ↑↓/J K move between stories, ⌘↑ ⌘↓ go to the ends, → or Tab move
  into the side column, Esc leaves. An element focused in the stage keeps its keys until Esc.
  Triggers and side views name their scene as `--playground` does, by path or just name. Root
  `on_paste`, `on_drop_files` and `on_window_focus` still reach the app above the bar, and the
  stage's root while it's open. Scenes get a fresh state, stories their own; the real
  state keeps polling and is untouched. It's on in debug builds (`.enabled`, `RUNE_PLAYGROUND=0`);
  its bar takes 26 pt off the bottom of the app's content, and `.bar(false)` leaves the app's
  layout as it was. Snapshots, `--scene` and goldens never include it unless `--playground
  [story]` is passed. `WindowSpec::playground` gives each window its own; hot reload drops it.
  `examples/playground` shows each kind.

- **Breaking: the browser dev panel is gone.** The `rune/dev` feature (and `rune-motion/dev`),
  `App::dev_panel`, the `rune::dev` module and the `tune!` macro were removed, along with
  `Runtime::motion_blur_enabled` and `examples/dev-panel`. Replace each `tune!("key", default, ..)`
  with its default as a plain constant (hot reload picks up edits), drop `dev = ["rune/dev"]`
  from the app's features, and audit scenes, pages and overlays with `App::playground`.
  `rune::scene_clock()` works without any feature.

- **A window with a still compositor rests.** A window with a `Compositor` never counted as
  resting, so each due `wake_after` built and presented a frame even when `poll` changed
  nothing (four presents a second for a 250 ms timer). When `Compositor::animating` is false,
  the window now rests like any other: a due timer runs `poll` alone and draws only when that
  poll changed something, with one frame for that poll. A compositor's frame still never counts
  as unchanged, since the same instances may composite differently. A compositor that reports
  `animating` (even one that starts while the window rests) draws every frame and every due
  timer, as before. For every window, a resting wake's frame that runs covered or misses its
  drawable no longer polls a second time, the timers that poll armed survive the window being
  occluded or minimized before its frame, and a frame that leaves a commit pending (a
  compositor that updates or invalidates the state while drawing) no longer lets the window
  go idle.

- **Hover waits while layout moves content under a still pointer.** When a row expanded and
  the rows below slid, each row passing under a pointer that hadn't moved took the hover for a
  frame or so, flashing its hover-revealed controls. The glide hold now covers all layout
  motion: frame and offset springs, growth stacks, growths, flights and app-asked glides. As on
  macOS, hover lands on the row under the pointer once the motion settles, or at once when the
  pointer moves or presses. The hovered element, `on_hover_change`, hover styles, the cursor
  and tooltips all hold together. A wheel or trackpad scroll still hovers each row it brings
  under the pointer, a press still shows and lets go, and an element that leaves the tree,
  starts its exit or turns inert (a modal opening) lets go of hover at once. An element on its
  way out now hears `on_hover_change(false)`, which it used to miss; with `hover_delays`, only
  an element whose enter was reported hears the leave, now also when it unmounts. A held press
  no longer keeps every element it passed inside: only what contains the pressed element.

- **Adding or dropping `El::group_opacity` fades, and a shrunk-end reader sticks in the tail.**
  An element that takes on a group-opacity rest level, or drops one (a parked queue row's
  actions shown at rest), now fades from or to full on its motion instead of cutting, and a
  dropped one leaves nothing behind once it lands; an instant opacity motion drops it at once.
  A reader a collapse glided to the new end sticks to it once the rows are within half a point,
  without the rows' last fraction lifting it, so rows arriving in the collapse's long spring
  tail grow in under a pinned end instead of mounting below the viewport. `on_far_from_bottom`
  measures from the end collapsing rows head for, so a glide landing early or rows regrowing
  mid-glide don't report far for a moment.

- **A glide the app asked for holds a still pointer's hover until it stops.** During
  `Tx::scroll_to_bottom` or `Tx::scroll_to`, each row passing under a pointer that hadn't moved
  took the hover for a frame, so hover-revealed controls flashed down the list. As on macOS,
  hover now waits for the pointer to move or the glide to end, then lands on the row under it.
  A wheel or trackpad scroll still hovers each row it brings under the pointer, and ends the
  hold. Presses still let go during the glide, and a row that leaves the tree under the held
  pointer still hears `on_hover_change(false)`.

- **A new `El::group_opacity` rest level fades instead of cutting.** Changing an element's rest
  level (a row's actions shown or hidden at rest) used to multiply the new level in on the next
  frame, so they snapped 0↔1. The rest now moves on the element's `opacity_motion`, or the
  commit's spring without one; an instant commit, Reduce Motion and a new mount land it in the
  frame.

- **Rows leaving a padded growth stack all at once hold their place.** With every row removed,
  the stack forgot its top padding, so the leaving rows jumped up by it on their first frame.
  They now collapse where they stood.

- **Content shrinking under a reader who scrolled up glides to its new end.** When a growth
  stack's rows left or shrank until it ended above the reader's offset, the viewport sat still,
  then rode the presented end down at the collapse's own speed (thousands of points a second
  from a standstill in one frame). It now glides to the layout's new end from rest on
  `motion::GLIDE`, as `Tx::scroll_to` does, never past the presented end, and sticks to the end
  once the rows get there. The glide follows a further removal to the newer end, ends if rows
  come back past the offset, and leaves the viewport to a wheel, a fling or a kept row until
  the rows settle. Settled, instant and reduced-motion changes still land at once. Any glide
  whose end the content shrinks past now lands there instead of overshooting and springing back.

- **A new window holds two drawables until it rests, on macOS.** A window's Metal surface now
  starts at frame latency 1 (two drawables) and goes to its configured 2 (three) once the window
  first goes idle on screen, or after its first second of frames there. A window that drew frames
  back to back while coming on screen (an app animating at launch, or launch frames waiting on
  the window server or on shader compiles) made CAMetalLayer allocate a third window-sized
  drawable and keep it: about 12 MB more settled memory for a 980x760 window. Raising the count
  allocates nothing until a frame needs it, so a window that animates later still gets its third
  drawable then.

- **A resting window's timer runs `poll` alone and draws only when it changed something.**
  A due `wake_after` on a shown window with nothing moving used to build, lay out, draw and
  present a frame every time, even when `poll` returned false. Now `poll` runs first, as it
  does for a hidden window, and the next timer is armed; the view, layout and a frame follow
  only when the poll returns true, asks for a scroll, a window frame or the next frame
  (`wake_after(ZERO)`), or leaves a request the window must draw for, and that frame doesn't
  poll again. An app polling a few times a second while idle builds no frames for those
  polls. Motion, scrolls, queued input, hover and tooltip delays, loops, live values,
  `SurfaceDraw` content and polls that step every frame take their frames as before.

- **A resting window presents nothing for a timer frame that draws what's on screen.** When
  only a due timer (`wake_after`, `poll`) wakes an idle window, the frame is built before a
  drawable is taken, and when its instances, clear colour, surface size, format, scale and
  blending match the last frame presented, with no glyph or image upload, `SurfaceDraw` or
  system material in it, the window acquires and presents nothing. Cursor, IME, secure input,
  help tags and the clipboard still follow the frame. A `poll` that returns true on every tick
  with nothing new on screen no longer costs a GPU frame each time. Input, resizes, scale and
  occlusion changes, animation, traces and compositors draw as before.

- **`markdown::find(src, query, case)` (`Markdown::find`, `Case`) searches rendered Markdown text and returns source byte ranges for `Markdown::highlights`.**
  Each outer entry of `Vec<Vec<Range<usize>>>` is one non-overlapping match in document order within a single block (paragraph, heading, list item block, table cell, code block or diff block), and each inner `Vec<Range<usize>>` lists the source UTF-8 byte ranges it covers, merged where adjacent and skipping inline markers (`**`, `*`, `~~`, `` ` ``), link and image URLs, and container prefixes. `Case::Insensitive` folds characters with `char::to_lowercase` (and Greek `'ς'` to `'σ'`) while preserving exact UTF-8 byte boundaries when lowercasing changes byte length (such as `'İ'` or `'ẞ'`/`'ß'`). `Markdown::highlights` now accepts `find` matches directly (`Vec<Range<usize>>`, `&[Range<usize>]`, or paired with `Highlight`) as well as single `Range<usize>` items.

- Cmd+A (and the Select All menu item) with no selection now selects all of a selectable group, so a read-only `Markdown` view needn't call `Tx::select_all_text` itself: the group holding the selection, else the one last clicked or selected in the window while it's mounted, else the window's only selectable group, else nothing. An app's `on_key` still runs first and a focused text field keeps its own. `Tx::select_text` and `select_all_text` document that the key is the element's final key, which a `memo` id replaces.

- **A launch whose place moves mid-flight turns toward it instead of jumping.** A
  `Tx::launch` flight now blends from the offsets outside the scene it left under to the ones
  its destination has each frame, so a row above that grows under a growth stack's offset
  moves where it lands, never where it is (a prompt growing its copy line as the steer under
  it stops joining its run lurched a steer in the air by 25 pt). When a growth row launched
  itself and its slot moved, the row's instant vertical place used to land the whole flight
  in one frame; it now restarts its path from where it is, on a spring.

- **Markdown table cards: review fixes.** A scrolling full-width table no longer shifts its columns while its last row streams in. `table`, `table_outline` and `table_like` now replace an earlier `table_frame` or `table_card` (`table_like` also drops the card and the header padding), the table setters treat non-finite points as zero, a table with no body rows is sized by its header's padding alone, and a header's overflow stops at the frame's rounded top corners.

- **Markdown tables on a card.** `MarkdownStyle::table_body(fill, radius, shadows)` puts the
  body rows on a card of their own, rounded and raised, with rules between body rows only and
  none under the header, which sits on the frame. `table_frame(fill, ring, pad)` draws a
  filled, rounded frame with an inset ring in place of the outline, `table_header_pad(x, y)`
  pads header cells apart from body cells, `table_full_width(true)` stretches a table to the
  text's width (columns widen in proportion to their content), and `table_margin(top, bottom)`
  sets the space around a table. `table_card(&kit)` sets all of these from the kit's tokens:
  a raised `bg_base` card 2 points inside a `layer_01` frame, a faint medium header and 13-point
  cells. Defaults are unchanged. `MarkdownStyle` now lives in `markdown/style.rs`; its path is
  the same.

- **Instant changes, live resizes and invisible exits land in the frame; a still pointer keeps
  its hover while rows mount.** An instant commit lands a pinned viewport's slack at once
  instead of gliding it closed a moment later (zoom). During a window resize, rows that rewrap
  land at their new height in the frame, so a pinned tail stays put instead of sliding back
  after the drag; a streaming row still waits for the width to settle. A removed element that
  drew nothing last frame (faded out already, or under a parent that hid at once) unmounts
  with no exit, and an exit stops once it draws nothing. An instant close of a modal keeps a
  focus request outside it, as an animated close did. Nodes mounting earlier in the tree no
  longer drop the hover of a still pointer for a frame (a hover-revealed row's actions
  flashed out while a timeline streamed). Scripts take `{"type": "resize", "dw", "dh",
  "dur"}`, a person dragging the window's corner. `scripts/motion-audit/analyze.py` adds
  `cut-exit`, `reversal` and `dog-leg` findings, script `expect`ations (`instant`, `follow`,
  `smooth`, `leaves`) and an allowlist with `--fail`.

- **`macos_drop` tests skip their pasteboard round trips in a session with no pasteboard.**
  A headless or SSH-only Mac (no window server) has no pasteboard server, and the six tests that
  write a private pasteboard panicked there. They now probe once, print a one-line skip note and
  pass; the pure parts (hover stand-ins, promised types, file URLs, numbered names, URL names,
  operations, step coalescing) run everywhere. No change to apps.

- **`Tx::launch(source, destination)`: a new element leaves from another's text.** The element
  keyed `destination` that mounts in this commit starts with its first text on `source`'s text
  (a composer's typed words), at its own size, and travels one straight screen path to its
  place on `tok::motion::REFLOW` (or the commit's `Tx::motion`). It draws above the window and
  outside every clip until it lands, so a sent message can rise out of a panel into a scroll
  view while the composer clears. Reduce Motion, instant commits and an already mounted
  destination leave it in place. Matched flights also settle later: travel progress settles
  like a scale instead of in points, so a long flight no longer ends a couple of points short
  and snaps the rest in one frame.

- **Markdown tables: the header's rule, fill and weight are set on their own.**
  `MarkdownStyle::table_header_rule(c)` draws the hairline under the header in its own colour
  (a web `th` with a stronger `border-bottom` than the `td`s), `table_header_fill(c)` sets the
  header fill without the border, and `table_header_weight(w)` gives the header a medium weight
  beside bold `strong` text. Defaults are unchanged.

- **Multi-click and click-drag selection in `text_input`.** Double-clicking a text field selects
  the word under the pointer (sharing `El::selectable`'s Unicode word boundaries), triple-clicking
  selects the line in a single-line field or the `\n`-delimited paragraph in a multi-line field,
  dragging after a single, double or triple press extends by characters, whole words or whole
  lines from the original unit, and Shift-clicking extends from `TextEdit::anchor`. Secure fields
  select the whole value on double- and triple-click without marking the secret edited.

- **The app takes Cmd+A and Cmd+C before selected text, and code can set and read the
  selection.** Key routing offers plain Cmd+A and Cmd+C to selectable text last, after both
  `on_key` calls and element key handlers, so an `on_key` that returns true for them now wins
  where the selection used to take them first. A focused text field keeps its own. New:
  `Tx::text_selection()` (a `TextSelection` of `SelectedText`s), `Tx::select_text(key, range)`,
  `Tx::select_all_text(key)`, `Tx::clear_text_selection()` (applied after the handler's
  commit), `El::selection_copy(|sel| ..)` to choose what Cmd+C and the Copy menu copy, and kit's
  `Markdown::copy_source(true)`, which copies the Markdown source of whole top-level blocks.
  `El::on_selection_change(|s, change, tx| ..)` hears a selection start, change and clear
  (`SelectionChange::started`, `cleared`, `selection`), at most once a frame.

Entries landed since the last fold are in `changelog/unreleased/`.

- **macOS windows take drops of file promises and image data.** winit's window took only
  drags with file paths, so the screenshot thumbnail, Photos, Preview and browsers' images were
  refused. The content view is now the drag destination: a file promise is received into
  `$TMPDIR/rune-drops/…`, image data (PNG, JPEG, GIF, WebP, HEIC; TIFF as PNG) is written there,
  and the drop reaches `El::on_drop_files` as paths. While hovering, a promise or image shows as
  a stand-in path (`File.png`, `Image.png`) so `accepts_files` filters work. AppKit reports the
  drag's position at every step, so macOS no longer samples the pointer for it.

- **`Tx::follow_edge()`: pinned rows ride a moving bottom edge.** Rows stuck to a
  stick-to-bottom viewport's end used to hold the room a shrinking sibling freed as slack, then
  close it late on the growth spring, which suits a send (a new row grows into it) but leaves a
  gap when a panel under a chat folds away. A change made with `follow_edge` carries the rows
  on the viewport's edge, frame by frame, on the change's curve, until the edge reaches its
  layout. Nothing changes for apps that don't call it.

- Markdown source highlights track multiline code spans inside quotes and lists exactly.
  Highlight element reporting and scroll targeting avoid quadratic work for many matches;
  oversized core highlight offsets no longer wrap. Bare URL autolinking accepts uppercase
  HTTP schemes and leaves image alt text alone (bare email addresses remain plain text).

- **Markdown source-byte highlights (`Markdown::highlights`, `Highlight`), strikethrough (`~~struck~~`), and bare-URL autolinking (`Markdown::autolink`).**
  `Markdown::highlights` accepts `(Range<usize>, Highlight)` pairs in source UTF-8 bytes and maps
  them to rendered text across inline formatting, headings, paragraphs, lists, task items, quotes,
  tables, diffs and code blocks without reshaping text or shifting line breaks (`El::text_highlight`
  / `TextHighlight`). Standard matches (`Highlight::new()`) and active matches
  (`Highlight::current()`) draw rounded background fills and optional inset border rings from
  `MarkdownStyle` (`highlight_bg`, `highlight_current_bg`, `highlight_current_ring`, or per-match
  `.bg(..)` / `.fg(..)` / `.ring(..)`), and each highlight can be scrolled into view with
  `Tx::scroll_into_view` by its `.key(..)` or `Markdown::highlight_key(index)`. GFM `~~strikethrough~~`
  spans now paint strike lines through `El::strike`, and bare `https://…`, `http://…` and `www.…`
  URLs in plain text autolink by default (with GFM trailing punctuation and balanced-parenthesis
  trimming, `http://` destination prefix for `www.`, and `.autolink(false)` to opt out).
- **`El::on_far_from_bottom(min, f)`: when a chat shows its jump-to-latest button.** On a
  scroll viewport, reports whether it stands more than its own height, and at least `min`
  points, above its bottom: once after its first layout, then only on change, like
  `on_at_bottom`. A glide back from `Tx::scroll_to_bottom` reports near before it lands. Nothing
  changes for apps that don't use it.
- **`Tx::motion_curve(anim)`: a commit's layout on any curve.** `tx.motion` takes only a
  spring; `motion_curve` takes an `Anim` (a spring, or a `Curve::tween` with an `Easing`), so an
  app can compare eased tweens against its spring for the same layout change. Scroll requests
  and matched flights in that commit, which take only a spring, use a zero-bounce spring of the
  curve's duration. The last `motion` or `motion_curve` in a handler wins. Nothing changes for
  apps that don't call it.

- **SVG images on every platform.** `ImageCache::svg(bytes, size_pt, scale)` rasterizes SVG or
  SVGZ to fit `size_pt` at the window's scale, so a 16 pt mark is crisp at 2×. `load_mime`
  takes a MIME type (a data URL's `image/svg+xml`), `file(path, size_pt, scale)` reads a file
  on the worker and treats `.svg`/`.svgz` as vectors, and `load` now recognises SVG bytes and
  rasterizes them at twice their own size. Before, only Linux app icons drew SVG. resvg moves
  behind a default-on `svg` feature; apps that set `default-features = false` on `rune` add
  `svg` to keep it.

- **Secure label review fixes.** Hidden read-only labels use twelve bullets regardless of length. Secure text cannot fly, retain content hashes for layout invalidation, or expose rich links. `Runtime::element_is_secure(path)` reports protected content without changing `ElementInfo` literals. Pristine tracking stores only an edited flag; use `TextEdit::replace_text` for app replacements (including identical values). Backspace at byte zero also clears pristine hidden multi-line fields.
- **Read-only secure text labels (`text(..).secure().revealed(bool)`) and pristine replacement on hidden multi-line secure fields (`TextEdit::is_pristine`, `TextEdit::mark_edited`).**
  - Read-only `text(..)` and `rich(..)` elements now support `.secure()` and `.revealed(bool)`, sharing the same protections as secure `text_input`: plaintext never enters `TextSystem::layouts`, `TextSystem::shapes`, ancestor `El::cache` rasters (`node_caches` / `resident_caches`), `ElementInfo::text`, `ElementInfo::selected`, `Debug` (`[redacted]`), or `--tree` JSON; selection and copy are disabled (even if `.selectable(true)` is called before or after `.secure()`); and owned `TextSpec` source buffers are zeroized on conceal and `Drop`. Hidden labels show a fixed twelve-bullet mask (`""` when empty), independent of length and line count. When revealed (`revealed(true)`), labels lay out and elide/clamp/wrap normally (including `.wrap_anywhere()`, `.max_lines(..)`, `.truncate()`), snap geometry and ancestor scroll immediately without height animation when concealed back to `revealed(false)`, and conceal to the mask on their first exit frame.
  - `Runtime::element_is_secure(path)` (and `--tree`'s `"secure": true`) identifies both secure labels (`role: "text"`) and secure inputs (`role: "textbox"`).
  - Hidden multi-line `.secure()` fields whose value has not been edited by the user since the app last set it (`TextEdit::new` or `replace_text`) are now pristine (`TextEdit::is_pristine()`, cleared by user edits or `TextEdit::mark_edited()`). While pristine and hidden, any insertion (typing or paste) replaces the entire value with the inserted text and any deletion (`Backspace`, `Delete`, `⌘⌫`, `⌥⌫`, `⌃K`) empties the field, independent of caret or selection; the first edit clears pristine so subsequent edits append or delete at the end as usual. Toggling `revealed` does not reset pristine, an empty field is never pristine, and revealed fields edit normally at the caret or selection.

- **Stacked paint fades into a soft clip as one.** Under `clip_fade` (or a feathered mask), each
  primitive faded on its own, so an image over its placeholder fill, or a label over its bubble,
  let the paint beneath show through the fade: a dark image on a lighter fill brightened towards
  the edge instead of fading to the background. An element that paints over its own paint (two
  of fill, border, gradient, shape, pixels, shadows, text; or one of them under children) now
  composites as one layer while it reaches into a soft edge, and takes the fade once. Elements
  clear of the fade edges draw as before.

- **`examples/opencode` left the repository** (it moved out). Its golden check is now `crates/rune-kit/tests/markdown_fixture.rs`: eight headless Markdown trees (mermaid, code, tables, task lists, narrow wrapping, streaming) in both OpenCode-theme appearances, compared with `crates/rune-kit/tests/golden/*.json`; accept intended changes with `RUNE_BLESS=1`. `scripts/check-ui.py`, `bench-build.py`, `bench-gpu-springs.py` and `text-bench.py` went with it, and `rune-dev`, `bench-ui.py`, `bench-startup.py`, `snap-ui.py`, `text-goldens.py` and the motion audit no longer list it.

- **`El::arrives_whole()`: a growth-stack row whose content arrives complete glides on the growth
  spring alone.** Such a row (a sent message, a finished reply loaded at once) skips the speed
  limits that pace streaming text, so a large insert, and a pinned viewport following it, moves in
  one critically damped glide instead of a long capped climb. Rows without it are unchanged; a row
  that is also `streaming(true)` keeps the limits.

- **`Tx::open_url`, `El::mx`/`El::my` and `rune::prefs::Prefs`.** `Tx::open_url(url)` opens
  `http:`, `https:` and `mailto:` links in the system handler on macOS (`NSWorkspace`), Windows
  (`ShellExecuteW`) and Linux (`xdg-open`), as `web::open_url` already did; other schemes
  (`file:`, an app's own), empty and malformed links return `OpenUrlError` and open nothing.
  Headless runs open nothing and keep the links for `Runtime::take_url_requests`. `mx` and `my`
  set both margins of an axis. `Prefs::open(app_id)` is a JSON settings file in the platform
  config directory with typed `get`/`set`, atomic saves, and a corrupt file moved to
  `prefs.json.bad`; `Prefs::memory()` and `RUNE_PREFS_DIR` keep tests and headless runs off the
  real file. `rune` now depends on `serde` on every target (it already did on Linux).

- **A window comes on screen with its first frame on macOS.** A visible window (not a panel or a
  hidden launcher) now draws its first frame while it's being created, before the window server
  reports it on screen, instead of opening empty and drawing on a later display-link tick. The
  first view build and layout now run inside window creation, right after the window-created
  callbacks. Content reached the screen about 30 ms sooner at launch on an idle Mac, and about
  150 ms sooner on a busy one, where the window server is slow to report a new window visible.
- **`rune::prewarm_gpu()`: the GPU device and render pipeline start on a thread before the first
  window.** On macOS, Rune's run functions now make the device and compile the uber pipeline on a
  thread while AppKit starts, and the first window or overlay opened at launch picks them up
  (waiting if the thread is still working) instead of making them after the window exists. Call
  `rune::prewarm_gpu()` first thing in `main` to start it sooner. A window whose compositor wants
  features the prewarmed device lacks gets a new device as before; with no window at launch the
  result is dropped. On a first launch after an install or update, when Metal compiles the
  shaders, a window's content arrived about 135 ms sooner. Elsewhere it does nothing.
- **`Gpu::instance()` no longer enables wgpu's indirect-call validation.** Rune issues no indirect
  draws or dispatches, and the validation pipeline wgpu builds for every device cost about 40 ms
  on a Mac's first launch after an install or update. Indirect draws a `Compositor` records on
  Rune's device still run; wgpu no longer validates their argument buffers.
- **Rows inserted in the same commit as a `scroll_into_view` land at full height again.** In a
  reader's virtual growth stack (unpinned, or `stick_bottom: false`), a commit that inserts
  rows and calls `tx.scroll_into_view` grew the new rows from zero since growth started springing
  under reveals. A search list that put a new first result above its first row and revealed it
  showed only that row's header for several frames, and a row with no header showed a sliver.
  New rows now land at full height under a reveal in their own commit, as they do under one left
  from an earlier commit, so the reveal shows them whole on the first frame. Rows already in the
  stack whose height changes keep springing, and pinned `stick_bottom` timelines are unchanged.
  **Output change** for an app that inserts and reveals rows in one commit.
- **Metal shader fallback also covers pipeline creation.** If Metal accepts the precompiled
  library but rejects its pipeline, `Renderer::new` retries with WGSL without an uncaptured GPU
  error. Shared layout descriptors and the actual Naga options have semantic fingerprints;
  unrelated Rust edits do not invalidate them. Freshness tests print replacement hash lines
  when MSL is unchanged, without needing Xcode. wgpu/core/hal allow `30.0.x` patch updates.
- **Precompiled Metal uber shader on macOS (`crates/rune/src/uber.metallib`).** `Gpu` now enables
  `wgpu::Features::PASSTHROUGH_SHADERS` on macOS when the adapter supports it, and `Renderer::new` loads a
  committed 33 KB precompiled Metal library (`-std=macos-metal2.3 -mmacosx-version-min=11.0
  -fpreserve-invariance -O2`) via `Device::create_shader_module_passthrough`, skipping WGSL
  parsing, validation, Naga MSL translation, and Metal's front-end `newLibraryWithSource` compile
  on every launch (`232 ms → 85 ms` for `Renderer::new` on a cold Metal shader cache, and
  `3.4 ms → 0.6 ms` warm). Falls back to the WGSL path when `PASSTHROUGH_SHADERS` is absent,
  `RUNE_DISABLE_METALLIB=1` is set, the WGSL/ABI fingerprint mismatches, or library/pipeline creation fails; app
  builds do not require Xcode (`scripts/precompile-metal-shaders.sh` regenerates the `.metallib`
  when generated MSL or Metal compile flags change).
- **macOS: live window resizes no longer stretch the last frame.** While the user drags a window
  edge, Rune draws each new size inside AppKit's resize and presents it in the same Core Animation
  transaction (`presentsWithTransaction` for the drag, back to its own present on the first frame
  after). Before, the old drawable was stretched to the new size until the next display tick, so
  text widened with the window and snapped back. The Metal layer's contents gravity is now the
  window's top left rather than `resize`, so a frame that does arrive late keeps its scale and
  stays pinned to the top left instead of stretching. A slow frame now holds up the drag rather than
  showing stretched. `cargo run --release -p rune --example live_resize_check` checks this
  against a real window. Other platforms are unchanged.
- **`El::wrap_anywhere` and `rune_kit::TextArea::wrap_anywhere` (`overflow-wrap: anywhere`).**
  `text(..)`, `rich(..)`, `text_input(..)` and `TextArea` now offer `.wrap_anywhere()`, which wraps
  at normal word boundaries when words fit and breaks inside an unbroken token (such as a base64
  key or long URL) only when the token alone would overflow the line, while lowering the text's
  min-content width without overriding flex shrinking (use `.shrink()` in a flex row). Multi-line
  `.secure()` fields enable `wrap_anywhere` by default.
- **Multi-line secure fields (`rune_kit::TextArea::secure`, `TextArea::revealed`, and multi-line `text_input(...).secure()`).**
  A multi-line field marked `.secure()` now accepts `Enter` to insert a newline (`⌘Return` /
  `Ctrl+Return` still submits) and preserves newlines and tabs on paste while keeping all secure
  protections (no undo/redo, copy, cut, IME composition, accessibility text, or layout/subtree
  caching, with zeroized buffers and redacted `Debug`). When hidden (`!revealed`), a non-empty
  multi-line secure field renders a fixed single-line bullet mask (`"••••••••••••"`) at a fixed
  height so neither byte length nor line count is leaked; the visual caret pins to the end of the
  mask, pointer hits place the caret at the end of the model, and any active selection highlights
  the full mask. Hidden edits append or replace the whole selected value; navigation moves to
  the end (Shift selects all), and Backspace deletes at the end. Reveal for interior edits.
  Hiding snaps geometry and resets ancestor scrolling rather than animating old line counts.
  When revealed (`revealed(true)`), multi-line fields show their real lines, wrap
  long unbroken tokens with `wrap_anywhere`, scroll inside `TextArea::max_height` / `auto_grow`,
  and conceal back to the fixed mask on the first exit frame.
- **`El::hover_motion_within(enter, exit)`: a subtree default for hover curves.** Every hover track
  below it (and the element itself) without its own `.hover_motion(..)` uses the nearest
  ancestor's pair instead of the core default (instant in, 0.1 s ease-out out), so
  `.hover_motion_within(Anim::INSTANT, Anim::INSTANT)` on the view root makes kit controls'
  inner hovers (`SegmentedControl` segments, `Switch` track, ...) switch at once. Hover groups
  read it too. `rune_kit::root` carries the base's pair to the popups it layers beside it.
  Without it nothing changes.
- **Caret geometry and focus in virtualized growth stacks with a vertical gap.** `VirtualList`
  and `Stack` now include a column container's vertical flex `gap` (`style.gap.height`) in row
  prefix sums, `before`/`after` padding, `stack_extent`, `reveal_row_once` and painted `row_dy`,
  and `caret_rect` includes any active ancestor growth-row shift (`stack_row_dy`). Previously a
  growth stack with `.gap(g)` shifted each mounted row `k` up by `k * g` when painting while
  leaving `caret_rect` at the unshifted layout target and clamping `scroll.max` short by
  `(mounted - 1) * g`. `sync_requested_focus` now also considers newly focused fields in
  offscreen virtual rows (`virtual_inputs`) instead of clearing their `focused` flag before
  `scroll_into_view` mounts them, and `follow_caret` re-commits when its scroll shift crosses a
  virtual window edge.
- **A `dissolve` parent no longer isolates for a child too faint to paint.** The dissolve pass
  counted any child above opacity 0; it now uses the same 0.001 threshold as painting, so a child
  faded to near zero no longer forces its parent into an offscreen layer.
- **Secure `TextEdit` and `TextInput` reveal toggle (`revealed`, `set_revealed`, `is_revealed`).**
  A `.secure()` field can now show its real characters with `TextEdit::revealed(true)` /
  `set_revealed(true)`, `El::revealed(true)` or `rune_kit::TextInput::revealed(true)` (as in a
  password manager's show/hide button) while keeping every other secure protection: no undo or redo
  history, no `⌘C` or `⌘X` clipboard writes, no IME composition (`has_secure_focus()` stays `true`),
  `ElementInfo::text` remains `None`, `Debug` on `TextEdit`, `InputSpec` and `TextSpec` stays
  `[redacted]`, model and rendered text buffers zeroize on replace and drop, and an exiting field
  conceals back to bullets on its first exit frame. Word movement (`⌥←` / `⌥→`) treats a hidden
  secure field as a single masked word and steps over words normally while revealed. Secure
  runs bypass the shared layout cache and use a disposable shaping context; secure fields also
  disable ancestor subtree raster caches even when unfocused. Third-party shaping scratch is
  released, not guaranteed zeroized. Handled key releases in secure fields are redacted in
  the runtime event log too.
- **`El::span_ring`, `El::span_rings`, `LinkPaint::ring`, and `Markdown::link_ring`.** Draws an
  inset border ring `(color, width)` inside a span's rounded `Span::bg` rect (`4 pt` corner radius),
  so link chips remain visible when their fill is close to the page background.
  Touching runs inside one ringed range merge into one rect per line, wrapped lines each get their
  own ring, and the resting ring stays on hover unless `LinkPaint::ring` overrides it. Leaves `Span`
  and `RichStyle` struct literals unchanged. Rings require a background, follow case conversion
  and truncation, and fade with the last revealed word on each line. Ranges are static, not
  rebased by text input edits. The public `Deco` literal is unchanged too.

- **`Pick`: pick fully transparent elements.** `Runtime::element_at_with`, `element_at_in_with`,
  `contains_with` and `Tx::element_at_with` / `element_at_in_with` take a `Pick` (private fields,
  builder methods); `Pick::new().transparent(true)` also finds elements whose own or inherited
  opacity is 0, so a design canvas can select an invisible layer. Exiting elements stay skipped.
  The existing methods use `Pick::default()` and behave as before.
- **GFM task list checkboxes in `Markdown`.** `- [ ] todo` and `- [x] done` items now parse with
  `pulldown_cmark::Options::ENABLE_TASKLISTS` and render as `Checkbox`-styled `"checkbox"` rows
  instead of `"•"` bullets, with `label` set to the item's text and checked state on
  `ElementInfo::item_selected`. Existing `Checkbox` output is unchanged.
  `Markdown::on_task(|s, block_key, marker_offset, checked| ..)` and
  `Markdown::on_task_tx(|s, block_key, marker_offset, checked, tx| ..)` fire on pointer press,
  or focused `Space` with the top-level block's key (`block_keys` or `b{index}`), the
  source byte offset of `[` in the task marker (including nested items), and the new checked
  state. Task rows are read-only and outside Tab order when no `on_task` handler
  is set or when `Markdown::tasks_read_only()` is called. **Output change:** markdown lists using
  `- [ ]` / `- [x]` task syntax now mount checkbox controls instead of bullet text.

- **`Markdown::block_keys`.** Top-level blocks can now take caller-supplied `(source_byte_offset, key)` pairs
  instead of `b0`, `b1`, … (nested blocks inside a list item or block quote keep `b{index}` under
  their parent block's key), so inserting, removing, reordering or editing blocks above others
  keeps unchanged blocks' keys and springs their layout while inserted and removed blocks play
  their enter and exit transitions. Offsets identify the start of each block's first source line,
  including indentation; callers update offsets after edits while keeping IDs stable. Unmatched
  offsets are ignored, so different parser boundaries do not shift later IDs. Missing, empty or
  repeated IDs fall back to an unused `b{index}`. Without `block_keys`, index keys stay unchanged.

- **`TextInput::plain` and `TextInput::leading_icon_size`.** `plain()` draws the field with no
  fill, sheen, shadow, border, hover tint or focus outline, for sitting in a header (focus shows
  as the caret and selection, invalid as red text, and disabled dims to 0.5); text, caret, icon,
  trailing slot, copy/clear buttons, press-anywhere focus and `on_action` work as before, and the
  horizontal padding is unchanged. `leading_icon_size(points)` sets the icon's side (default 16,
  so nothing moves); the icon stays centred and the text start follows.
- **Pose and picking reads.** `ElementInfo::moving` (and so a settle loop built on it) now also
  counts shadow colour and geometry springs and icon turns; rotation and scale already counted.
  `Runtime::presented_pose(key)` returns a `Pose` with the presented rotation, effective scale,
  shadows (geometry and colour as sprung) and a moving flag. `Runtime::element_at(x, y)` returns
  the topmost drawn element at a window point (`Picked`, with `path()` and `key()`), through
  rotate, scale, origin, offset, scrolling, ancestors' clips and paint order, handlers or not;
  `Runtime::contains(key, x, y)` tests one element. Both are made for drivers holding the
  `Runtime`; a running canvas can use `Tx::element_at(x, y, callback)` or
  `Tx::element_at_in(scope, x, y, callback)` with a pointer event's window coordinates.
  The callback runs before the handler's view commit; scoped picking ignores other subtrees.
  Invisible geometry with nonzero element opacity remains pickable, even without a fill.
  Decorative loops still report moving, so bounded settle waits should exclude looping elements.

- **`Markdown::on_link_tx` and `Markdown::link_style`.** `on_link_tx(|s, url, tx| ..)` is `on_link`
  with the press's `Tx` (`on_link` is unchanged; the two share one slot so the later call wins).
  `link_style(|url| -> Option<(Span, LinkPaint)>)` gives chosen links their own resting `Span`
  (colour, weight, `bg` with padding, underline; emphasis, strong and inline code inside a link
  keep their styles over it) and hover paint, whose `bg` takes the resting span's `bg_pad` and
  height; `None` keeps the default look, so nothing moves unless an app opts in. Touching runs of a
  padded `Span::bg` now merge into one rounded rect.

- **`PopupList::padding` and `PopupList::row_radius`.** The popover's padding and each row's
  corner radius were fixed at 4 points; both builders keep 4 as the default, so nothing moves.
  `PopupList::items` now documents that a commit calling `tx.instant()` swaps the rows and the
  popover's height at once.

- **Rows entering a virtual growth stack while a reveal runs land at full height.** A
  `scroll_into_view` (or a `reveal_selected` list's own reveal) outlives its commit while the
  viewport's springs run. Until now any later commit in that window grew its new rows from
  zero instead of landing them, as a reader's virtual list does otherwise. Inside a popover whose
  height springs at the same time, the two springs moved rows past their slots: a `PopupList`
  above its field, filtered to one row and then given more rows while the removed ones still
  collapsed, raised its last row 4 points past its slot before it sank back. **Output change:**
  in a reader's virtual list (unpinned or `stick_bottom: false`), those rows now land at full
  height and ride the popover's edge one way. Rows already growing under the reveal, and
  `stick_bottom` timelines whose reveal is at the tail, keep growing on their spring.

- **`SidebarRow::subtitle_color(Color)`.** A two-line row's subtitle colour, in place of the
  default `text_faint`.
- **Sticky elements stay opaque under `El::scroll_edge_fade`.** `El::sticky_top`, `El::sticky_bottom`
  and `El::sticky_left` no longer inherit their scroll viewport's edge fade on the edge they stick
  to, so a stuck header's fill and text stay crisp and occlude rows scrolling under them, and
  sections nested inside a `memo` growth row bound their sticky children to the animated row height.
  The exemption also applies before the header sticks, without an opacity jump at the threshold;
  explicit `clip_fade` masks and fades from outer viewports still apply.
- **Two-line `SidebarRow`s read as a title over a caption; select-only rows; launcher fields.**
  **Output change** for rows with `.subtitle(..)`: the name is now medium weight (13 / 16) and the
  subtitle the kit's `caption` (11 / 16) in `text_faint`, where both were 13 / 16 and the subtitle
  was `text_muted`; both lines still fit the 40 pt default. The row's accessible name is now
  "Name, Subtitle" (it was the name alone). `SidebarRow::title_style(..)` and `subtitle_style(..)`
  override the two types. `SidebarRow::select_only()` makes a row that shows the selection but
  never takes focus: out of Tab order, a press selects it and leaves an outside field focused.
  `El::preserve_focus_on_press(true)` is documented as working on any element (the press still
  reaches `on_press`; scrolling still cancels it). `TextInput::text_style(TextStyle)` sets the
  field's type (caret, placeholder and selection follow; the box grows to fit) and
  `TextInput::height(..)` its height (at least the style's line height), for a launcher-size field.
  A press anywhere on a container with `role("textbox")` focuses its one reachable editor: a
  supported pattern, now documented on `El::role`. `rune_theme_opencode::ember(dark, step)` (1 to
  12, else panics) and `scale(name, dark, step)` give typed colour-scale steps; `brand(dark)` is
  `ember(dark, 9)`.

- **`Toolbar`'s leading slot gives way.** The leading slot now shrinks (`shrink().min_w(0)`),
  so a long leading title marked `.truncate().shrink().min_w(0.0)` truncates instead of pushing
  the trailing slot past the band's end. Toolbars whose contents fit are unchanged.
- **Expressive press motion.** `El::scale` now springs between values on the element's layout
  spring, or on `El::scale_motion(anim)`, instead of snapping; it lands at once under Reduce
  Motion, in an instant transaction and on mount. **Output change:** an app that changed `scale`
  between frames now sees it animate; pass `.scale_motion(Anim::INSTANT)` (or `tx.instant()`) to
  snap. `El::origin(x, y)` sets the unit point that `scale`, `live_scale`, `press_scale` and a
  transition's scale pivot about (default centre); `offset` ignores it. The first four shadows'
  x, y, blur and spread now spring on the layout spring, as their colour already did, instead of
  snapping. `El::rotate(radians)` turns an element and everything in it clockwise about its
  origin, springing like `scale` (or on `El::rotate_motion`). A turned subtree draws into a
  layer whose composite turns; hit testing follows the turned box while the reported rect uses
  its upright bounds, and a turned subtree never caches. Layer records carry the turn in
  `border[1..4]` (`Instance::turned`), which rune-ref now renders. `demo/motion/press.png` shows
  a keycap at rest and pressed (the `expressive_press` example).

- **`PopupList` and `PopupItem`.** `Combobox`'s virtualised popup listbox is now a standalone
  `rune_kit` component (`PopupList::new(key, anchor)` with `PopupItem::new(id, label)` rows) for
  anchored popovers filtered by an outside field, such as a chat composer's `@`-mention and
  `/`-command popovers. Each `PopupItem` takes an optional `.description(..)`, `.leading(el)`,
  `.trailing(el)` and `.disabled(true)`. `PopupList` takes no keyboard focus and pressing inside
  it leaves the outside field focused, while `on_active` and `on_choose` report pointer hover and
  selection. Changes to `.active(..)` reveal the selected option except on hover frames;
  the listbox's accessible label stays stable. Defaults are top/start, 260 points wide and
  240 points maximum list height. `Combobox` shares the list builder but keeps its existing
  row contents and 192-point viewport. `El::reveal_selected(true)` opts virtualized stacks
  into selection reveal; `El::preserve_focus_on_press(true)` keeps outside focus on presses
  in unfocusable popup content. Ordinary lists retain their scrolling and blur behavior.
  `TextArea::animate_edit_layout(bool)` controls edit-driven layout animation.
- **`LoopSpec::easing` for slides.** `LoopSpec::slide(..).easing(Easing::LINEAR)` sweeps at a
  constant speed, for a shimmer; slides stay ease-in-out by default, and reduced motion still
  holds the midpoint. The curve rides in a new `LoopKind::SlideEased { from, to, easing }` variant,
  so `LoopSpec` and `LoopKind::Slide` keep their fields and literals still build. **Build break
  only for exhaustive matches:** a `match` over `LoopKind` with no `_` arm needs a `SlideEased`
  arm, and code that tests for `LoopKind::Slide { .. }` to find slides should test for both.
- **An anchored box keeps its aligned edge while it resizes.** An `El::anchored` box whose size
  springs (a pill over a composer whose label grows, a popover that shrinks) now keeps the edge
  facing its anchor `gap` points away, and the edge its `Align` names on the anchor: `Align::End`
  its end, `Align::Center` its centre, `Align::Start` its start, on every side. Before, its
  position also sprang from where it last sat, so an End-aligned box growing by 77 points first
  swung its end edge 76 points past the anchor and back, and a `Side::Top` popover shrinking from
  180 to 57 points jumped its top far above its new place and sprang back down. A box whose anchor moves or resizes
  now follows the anchor's drawn edge frame by frame instead of trailing it on its own spring.
- **`rune_theme_opencode::brand(dark)`.** The opencode theme's brand colour (ember step 9) as a
  typed function, so apps no longer look the token up by name.
- **Textbox padding never chooses arbitrarily between multiple editors.** Only a single
  reachable editor takes a container press; hidden editors are excluded. Trailing copy/clear
  buttons keep their hit targets even without handlers, and descendant resolution runs only
  on presses, not hover. Explicit `Tx::blur()` also cancels pending scrim focus restoration;
  only zero-size `focus_return` sentinel leaves use their parent's scope.
- **`IconButton::help` and `IconButton::shortcut` (plus `Button` and `SidebarToggle`).**
  `IconButton::help("Lock").shortcut("⌘L")` (or `.shortcut("cmd+l")`) sets `El::help("Lock  ⌘L")`
  and supplies the accessible label when `IconButton::label` is unset. `Button` supports the same
  `.help` and `.shortcut` builders, and `SplitView`'s sidebar toggle shows `"Hide sidebar  ⇧⌘B"` /
  `"Show sidebar  ⇧⌘B"` (configurable with `SidebarToggle::shortcut`).
- **`rune_kit::shell::Page` for full-window screens in a `SplitView` window.** `Page::new(content)
  .title(..).leading(..).trailing(..).titlebar_inset(..).frame_inset(..).toolbar_in_panel()
  .build(&kit)` reuses `SplitView`'s toolbar band, traffic-light clearance, window-drag region
  and framed content card so switching between `SplitView` and `Page` matches pixel for pixel.
  `demo/kit/shell-page.png` shows it in light and dark.
- **Two-line `SidebarRow`s default to `TWO_LINE_ROW_HEIGHT` (40 pt) when `subtitle` is set** and
  return focus to the selected row when inline rename ends. Previously these rows defaulted
  to 28 pt; explicit `.height(..)` values are unchanged.
- **Hiding a pane or overlay keeps focus where it was outside that subtree.** `El::focus_return`
  restores its trigger only when the focused control was inside the removed subtree (or pane);
  hiding the sidebar with `⇧⌘B` while typing in a main-pane field keeps focus in the field
  instead of moving it to the sidebar toggle, and closing a focused inspector returns focus to
  the main pane first.
- **Pressing any part of a `TextInput`, `TextField` or `TextArea` focuses it.** Clicking the
  leading icon, padding or a non-button trailing area focuses the inner editor and places the
  caret at the nearest end without blurring an already-focused field or starting a window drag
  in a titlebar; trailing buttons keep their own presses. `TextInput::trailing`,
  `TextField::trailing` and `TextField::on_action` attach trailing accessories and copy handlers.
- **`Dialog::fit`.** A dialog sized to its header and body instead of the standard 368 point
  height, for confirmations whose buttons otherwise sit far below one line of text.
- **A field inside a list row keeps its keys.** With focus in a text field inside a focus group
  (an inline rename in a sidebar row), letters, arrows, Home and End go to the field; before, a
  letter matching another row's first letter type-selected that row and took focus from the field.
- **Growth keeps springing under `Tx::scroll_into_view` and `Tx::keep_on_screen`.** Expanding
  or collapsing a row in a growth stack and calling `tx.scroll_into_view` or `tx.keep_on_screen`
  in the same handler no longer settles the row's growth in one frame, including in virtual
  growth stacks. `Tx::scroll_into_view` now measures mounted rows from their presented growth
  box and follows the target row frame by frame while the viewport's growth or layout springs
  run, unpinning a `stick_bottom` viewport when needed until another scroll gesture or request
  takes over. Reveals leave other viewports' reading anchors alone, stop when their target
  disappears, and yield to wheel, fling, paging or explicit scroll destinations. Commits marked
  with `tx.instant()` settle changed non-streaming row heights and that commit's scroll requests
  in one frame, including requests made with the animated scroll methods; existing glides stay
  running unless replaced.

- **Per-corner and continuous-corner rounded clipping (`El::clip_rounded`, `El::clip_corners`).**
  `El::clip_rounded` now follows the element's own per-corner radii (`El::corners`) and corner
  curve (`El::continuous_corners`) with no extra call, and `El::clip_corners([tl, tr, br, bl])`
  overrides the four clip corner radii directly. **Output change:** when paint radii and
  `clip_rounded(radius)` differ, the paint radii now win; use `clip_corners([radius; 4])`
  to retain an independent clip. Mixed radii round to 0.5 px through 63.5 px, otherwise 2 px,
  saturating at 254 device pixels. Mixed corner radii pack into `Instance::fade[3]`
  while uniform radii keep their exact IEEE-754 `f32` representation across the direct path,
  isolated layers, native video surfaces, `CLIP_WGSL`, and `rune-ref`.

- **`SparkleParams::count` cap raised from 32 to 64.** `rune-fx` sparkles now pack and evaluate up
  to 64 four-point stars per layer (`p.count.min(64)`, 128 `vec4f` entries in the pre-allocated
  4096-entry table buffer) so wide banners can keep star density without splitting across layers.

- **Blend modes for `rune-fx` effect and fill slots (`FxBlend`, `FxSlot::blend`).** `FxSlot::blend`
  accepts `FxBlend::{Normal, Multiply, Screen, Overlay, SoftLight}` so warm glows and shader fills
  can composite over light backgrounds (such as `#FAF7F2` cream paper) without washing out to
  white. `Screen` uses fixed-function `wgpu::BlendState` with zero backdrop reads;
  `Multiply`, `Overlay` and `SoftLight` sample the backdrop (`FxFrame::backdrop` or `EffectSite::backdrop` via
  `set_layer_backdrop` / `blur_backdrop`) and fall back to `Normal` when no backdrop is available.
  Separable formulas run in the frame's working colour space (linear light by default, or sRGB when
  `FxRenderer::set_srgb_blend(true)` matches `Tx::set_srgb_blend`).
  Multiply includes the source contribution through transparent backdrop pixels, avoiding dark
  fringes in isolated layers. Overlay and SoftLight inherently have little contrast near white.

- **rune-kit diff: the gutter stays put while code scrolls sideways.** In unified mode with
  long lines, the line numbers and the +/− marker scrolled out of view with the code. Each row's
  marker, both number columns and the comment "+" now sit in one `gutter` element held to the
  viewport's left edge with `sticky_left`, on the gutter's own opaque fill (added, removed,
  selected), so code passes under it. Elements in a row now have the path
  `<row>/gutter/{nn,no,plus}` and the marker is `<row>/gutter/bar`. Annotation cards, folded-gap
  rows and file-note rows hold their content to the left edge the way file headers do, and text
  carets and `El::anchored` overlays inside sticky subtrees follow the sticky shift. Split wraps
  and does not scroll sideways, so it is unchanged.

- **A pane's width survives a wider range.** `PaneState::new(640.0).range(360.0, 960.0)` is 640
  wide and double-click resets to 640; before, `new` clamped to the stock 160–480 first, so it
  was 480.
- **One undo step for grouped edits, and per-pill styles.** `TextEdit::undo_group(|e| ..)` makes
  every edit inside one undo step (picking a mention: `replace_with_pill` then `insert(" ")`);
  groups nest, an empty one records nothing, and typing coalescing and IME are unchanged outside
  it. `El::pill_style(|id| PillStyle::default().bg(..).fg(..).icon(..))` styles each pill from a
  closure that can capture app state (chip colour, label colour, icon; unset parts fall back to
  `pill_colors` and `pill_icon`). `pill_icon` now takes any `Fn(&str) -> Option<&'static Icon>`
  closure instead of a plain `fn` (existing `fn` items still compile; a `fn` pointer type
  annotation on a stored value does not). rune-kit's `TextField`, `TextInput` and `TextArea`
  forward `pill_style`. `demo/kit/pills-styled.png` shows an agent and a file pill.

- Drag cancellation reaches native Escape and restores pane widths, column widths, sliders
  and gutter selections. Delayed hover ends on window blur; raised content keeps ancestor
  clips outside cached groups. Dropping offset-reordered virtual rows no longer scrolls the
  list a second time, and live-only drags cannot make another handler's changes instant.

- **`rune_kit::shell`: what a desktop agent-chat app's sidebar needs.** All additive; an app
  that sets none of these draws as before, except that the sidebar's toggle no longer holds a
  pressed fill while the sidebar is open (it reports `expanded` for accessibility instead).
  - `SplitView::frame_inset` (default 8), `toolbar_in_panel`, `sidebar_padding(start, end)`,
    `slide_sidebar_contents` and `sidebar_header` frame the window.
  - `SplitView::pane_motion(PaneMotion)`: both panes move on one spring, optionally scaled by
    the square root of the width (`scale_with_width`), with the contents fading over the last
    part of the travel (`fade_tail`).
  - `SplitView::sidebar_peek(SidebarPeek)`: resting on the leading edge or on the toggle
    slides the closed sidebar over the content as a floating surface; leaving closes it and a
    click docks it.
  - `SidebarToggle`: the toggle's size, place past the traffic lights, `in_header` (at the end
    of the sidebar's title row while open, travelling on the sidebar's spring) and an `unread`
    dot that fades while closed.
  - `SplitView::reduced_motion(cx.is_reduced_motion())` swaps slides for fades.
  - `Sidebar::header_gap`, `header_padding`, `row_gap` and `inset`.
  - `SidebarRow::avatar`, `subtitle`, `height`, `status(RowStatus)` (busy, attention, unread or
    any element in a 20 point slot), `on_close` (a close button that takes the slot's place
    under the pointer) and `reorder` with `shell::Reorder` (drag to a new place, Escape or
    losing focus puts it back).
  - `cargo run -p rune-kit --example shell` is set up with these.

- **`Kit::interactive`, `interactive_with` and `elevate` are public**, so an app's own
  elements take the kit's hover and press look and its elevation shadows. Additive.

- **Core: `El::offset`, `El::raise`, `El::hover_delays`, cancelled drags, `Tx::animated`.**
  - `El::offset(x, y)` shifts an element and its subtree without moving it in layout, with
    hits following it, on its layout spring or `offset_motion`.
  - `El::raise()` paints an element and takes its input above everything else, keeping its
    layout, focus and accessibility order.
  - `El::hover_delays(enter, leave)` makes `on_hover_change` wait for the pointer to rest.
  - A held drag that ends without a drop (Escape, the window losing focus, the pointer taken
    away) now hears `Phase::Up` with `Drag::cancelled()` true instead of losing its capture
    silently.
  - `Tx::animated()` lets a drag's move animate what it changes.
  - `Cx::is_reduced_motion()` reads Reduce Motion for builders, and `ElementInfo` is exported
    at the crate root.
  - Behaviour change: an `opacity_motion` element only animates on a new target (one that
    mounts at its resting opacity, or re-commits the same target, stays still).
  - Motion traces flag elements that draw and hold nothing (`flags::BLANK`, 64).

- **rune-kit diff: file headers span the viewport in unified mode; new `El::sticky_left`.**
  When a line was wider than the view, the in-list file headers were as wide as the longest
  line, so the `+N −N` counts and `header_actions` sat past the right edge and scrolled away.
  Headers are now exactly the viewport wide and held to its left edge while the code scrolls
  sideways, with a long path truncating before the counts. `El::sticky_left(inset)` is
  `sticky_top` on the horizontal axis: it holds an element to the left edge of its nearest
  `scroll_x`/`scroll_xy` viewport while its parent is in view (`ElementInfo::stuck` reads the
  shift).

- **Pinned growth stacks hold freed room as slack.** A `stick_bottom` viewport whose content
  is a growth stack no longer carries its rows down when it gains room at its bottom while
  pinned (a composer shrinking on send, a row removed at the bottom). The room is held under
  the last row, rows growing in fill it first, and the rest closes on the stack's spring
  about 120 ms after the rows stop growing, so a send only moves the rows up. A window resize,
  `tx.settle` and reduced motion land it at once; scroll offsets and `on_at_bottom` are
  unchanged. A pinned timeline's frames move during a send in an existing app.

- **Five more built-in glyphs.** `rune_kit::glyphs` gains `MIC`, `KEYBOARD`, `CURSOR_TEXT`,
  `ARROW_RIGHT` and `SPARKLE`, drawn like the rest of the set (16-unit view, 1-unit stroke). They
  are plain statics, not `Glyphs` slots, so no theme or app literal changes: draw them with
  `icon(&glyphs::MIC, 16.0)`. `demo/kit/glyphs.png` shows the whole set, light and dark
  (`cargo run -p rune-kit --example glyphs`).

- **Atlas capacity preserves glyphs already drawn in the frame, including text flights.**
  At the eight-page R8 limit, requests fail without overwriting live placements and retry in
  later frames. Empty outlines stay cached across eviction. The 16 MiB colour cache retires
  unused scales rather than flushing visible emoji; a visible working set may exceed that soft
  budget. Direct text users can call `TextSystem::begin_frame` and decode packed atlas origins
  with `GlyphQuad::atlas_page`, `GlyphQuad::atlas_uv` and `Atlas::page_data`.

- **Glyph, icon and colour-glyph atlases never exhaust or permanently drop glyphs across long zoom sessions.**
  `Atlas` now separates genuinely empty glyphs (such as space, cached as `Some(None)`) from
  unplaced glyphs (`Rasterized::Unplaced`, never cached), recycles 2048×2048 R8 shelf pages via a
  two-page steady ping-pong pool with 1 px zeroed borders around each placed glyph (no 4 MiB buffer
   zeroing on reset and no eviction of this frame's placements), spills to on-demand overflow pages when a single
  frame exceeds one page and reclaims idle overflow pages on subsequent frames, and bounds the
  colour-glyph RGBA cache (`COLOR_GLYPH_BUDGET`, 16 MiB). While `live_scale` is actively changing,
  text and icons step through logarithmic $2^{1/4}$ scale buckets at a stable origin and
  re-rasterize crisp 1:1 device-pixel glyphs once the scale holds still for `0.1 s`.

- **Plastic fill, and liquid metal pinned per shape.** `Fx::fill(PlasticParams { .. })` paints
  moulded plastic under one soft light: `color`, `roughness` (polished to matte), `dome`
  (negative dishes the face like a keycap), `bevel` and light `angle`, with presets `glossy`,
  `matte` and `dished`. It keeps rgb within alpha, so it blends the same with `srgb_blend` on or
  off, and it never animates. It shares liquid metal's dome and rim code, and the catalog lists it
  (`fill_styles()`), so `fill-presets.png` gains its presets. `LiquidMetalParams` has a new
  `anchor: FillAnchor` field (`World`, the default, or `Element`, also settable with
  `.anchor(FillAnchor::Element)`): with `Element` the ripples are pinned to each shape's centre,
  so shapes of the same size, seed and time are pixel-identical. A full `LiquidMetalParams { .. }`
  literal without `..preset` must now set `anchor`. Fills now shade at the exact pixel centre
  instead of the interpolated one, which moves output by far less than one 8-bit step.

- **Pasted images reach paste hooks.** `ClipboardContent::image()` holds a pasted image as a
  `ClipboardImage` (PNG bytes via `png()`, pixel `size()`), with `with_image` to build one
  and `clipboard::MAX_IMAGE_BYTES` (32 MiB as PNG) as the cap. Only targets with a hook fetch
  it: a field's `on_input_paste`, the root's `on_paste_content` and `Tx::read_clipboard`; plain
  and hookless rich fields read the clipboard as before. macOS reads `public.png`, else
  converts `public.tiff`; Windows (`PNG`, else `CF_DIBV5`) and Linux (`image/png`) read through
  arboard's `image-data` feature, now on for those targets; the web reads `image/png` from the
  paste event or `navigator.clipboard.read()`. An image-only clipboard now reaches hooks
  instead of nothing. Copies don't write images. Script `paste` steps take `"png": "path"`.
- **Atomic inline pills in rich text (`Mark::Pill(id)`); `Mark` is `#[non_exhaustive]`.** A
  pill is a mention, file or attachment run whose label is the text under the mark. The caret
  steps over it in one move, a selection reaching into it grows to cover it, `Backspace` /
  `Delete` (and word deletes) remove it whole, typing and IME composition never land inside it,
  and undo restores it with its id. Typing never inherits a pill and an edit that cuts into one
  leaves plain text. Apps make pills with `TextEdit::insert_pill(id, label)` or
  `replace_with_pill(range, id, label)` (swap a typed `@query`, one undo step) and list them with
  `pills()`. The field draws each as a rounded chip that never splits across lines,
  `El::pill_colors(bg, fg)` (default: the link colour at 20%) and `El::pill_icon(|id| ..)` (also
  on `TextInput`, `TextArea` and `TextField`) style it. Copy writes the label as plain text and
  `<span data-pill="id">label</span>` as HTML; a paste of the app's own last copy restores the
  pill (`paste_html_with_pills`), while other HTML never makes one (`paste_html` drops it).
  `TextEdit::allow_marks` always allows pills. Exhaustive `match`es on `Mark` need a wildcard arm.
- **Monospace text has no ligatures, and its tabs reach 4-column stops.** Mono rich spans
  (`Span::mono`, so `Mark::Code` and `Block::Code` in a rich `TextEdit`, and Markdown inline
  code) and `Family::Mono`/`Family::SystemMono` styles turn off `liga`, `clig` and `calt`:
  Inter in the mono slot drew `->` as an arrow. A tab in monospace text now advances to the
  next 4-column stop (counted from the line or the mono span's start) instead of one space;
  it stays one byte. Markdown code no longer pads tabs with uncopied spaces when its style is
  mono, since layout does it. Goldens with tabs or `->`-style pairs in mono text move. Code in
  a rich editor uses the mono slot, which is Inter unless `AppSpec::families` names a
  monospaced family; the `rich_editor` example now embeds IBM Plex Mono.
- **Effect slots (`El::effect`) at a zoom scale, under rounded clips, with per-slot backdrops (`El::effect_backdrop`), and in `srgb_blend` frames.**
  `KIND_EFFECT` (`instance.params[3]`) and `EffectSite::scale` now carry the device pixels per point
  at the slot (`sc * s`, combining the window's Retina factor with inherited `scale`, `press_scale`,
  `live_scale` and scale transitions, where `params[3]` previously held only `s`).
  `rune_fx::FxSlot::scale(f32)` overrides `FxFrame::scale` for that layer so a border, glow reach
  or rib width on a zoomed canvas stays in the element's points, and `FxSlot::clip(clip, fade)`
  applies `rune::render::CLIP_WGSL` (`clip_alpha`) in `rune-fx`'s fragment shader and tightens the
  quad to `clip` so a slot outside an isolated layer gets the same rounded container corners and
  scroll-edge feathers as Rune's own primitives. `El::effect_backdrop(true)` marks a slot that reads
  what was drawn before it (`instance.params[0] = 1.0`): `Renderer::draw_with`,
  `Renderer::draw_with_backdrop`, `Renderer::draw_target_range_with` and
  `Renderer::encode_layers_with` / `Renderer::encode_layers_with_backdrop` keep a single render pass
  across ordinary slots and split the pass only at backdrop slots, copying the target into a pooled
  viewport-sized backdrop texture, calling `EffectPhase::Backdrop(encoder)` while the pass is closed
  (for `FxRenderer::blur_backdrop` or `FxRenderer::set_layer_backdrop`), and re-opening the pass with
  `LoadOp::Load` and `EffectSite::backdrop` set (a target without `COPY_SRC` can't be copied, so
  the slot draws in the open pass with `backdrop == None`; for a slot inside an isolated layer, copying that
  layer's `Rgba16Float` target at `origin` in window coordinates). When
  `FxRenderer::set_srgb_blend(true)` is active, `rune-fx` decodes sRGB-encoded premultiplied
  backdrop and blurred-backdrop samples to linear light before shading, so glass, lenses, distortion
  and backdrop dither or halftone are no longer double-encoded on non-sRGB frame targets or inside
  `Rgba16Float` isolated layers when given `EffectSite::backdrop`. Pass slot metadata separately
  to `FxRenderer::prepare_with_slots`; `FxLayer` and `FxFrame` struct literals remain unchanged.
  Explicit `FxFrame::backdrop` inputs still provide linear samples by default, including
  hardware-decoded `*Srgb` views; `FxSlot::backdrop_srgb(bool)` overrides the input encoding
  independently of output blending. Backdrop textures follow live target-format switches and
  resize without reallocating when the format and viewport are unchanged.

- **Sticky elements in scroll views: `El::sticky_top`, `El::sticky_bottom`, `El::stuck_shadow` and `Tx::keep_on_screen`.**
  An element inside a vertical scroll viewport can pin to the viewport's top or bottom edge
  plus an inset while its parent stays in view (`El::sticky_top(inset)`,
  `El::sticky_bottom(inset)`), and is pushed out by its parent's content box (inside the
  parent's padding) as the parent leaves. Sticky children paint and hit-test above later
  normal siblings of their parent (and below `above` and `modal` overlays), move in the same
  frame as the scroll offset without a second layout pass, and expose their stuck displacement
  on `ElementInfo::stuck` (`--tree` JSON `stuck` when non-zero) and through `El::on_drawn_layout`.
  `El::stuck_shadow(Shadow)` adds shadows that fade in over the first 8 pt of stuck offset.
  `Tx::keep_on_screen(key)` holds a keyed element at its drawn vertical position across a
  commit that collapses or expands content inside its scroll viewport (including `virtual_rows`),
   compensating the scroll offset each frame while layout and growth springs keep running.
   A cached subtree containing sticky content draws live, and anchored overlays follow live
   transforms on sticky targets. Horizontal-only scroll ancestors are skipped; virtual headers
   must share a row with their section to remain mounted while that section is visible.
  `Tx::scroll_into_view` clears headers already stuck at the target edge, and `El::anchored`
  follows a sticky target's drawn rect. New API only; existing output is unchanged.

- **`rune_kit::diff`: a diff view for code review.** `diff::parse` reads `git diff` output and
  `FileDiff::from_texts` diffs two texts; `DiffView` draws a `DiffState` unified or split, with
  word changes, syntax colours, folded unchanged ranges, sticky file headers, app widgets
  under lines (annotations), line selection from the gutter and one-side copy, on a virtual
  list that keeps a 50,000-line diff at frame rate. See the rune-kit README. Additive.

- **`El::virtual_rows_version(v)` and `El::on_visible_rows(f)`.** `virtual_rows_version` lets a
  `virtual_rows` list skip calling its key closure for every row each commit: while `v` and the
  row count stay the same, layout keeps the last keys. `on_visible_rows` reports the index range
  of rows intersecting the scroll viewport whenever it changes, committing only when the handler
  returns `true`, so a sticky header or file tree can follow the reading position without per-row
  `on_layout` commits on every scroll frame. Culled virtual rows now also unmount their whole
  subtree from the scene immediately. Lists without these behave as before.

- **Rich fields: soft breaks on rebuilt drafts, leaving code blocks, allow-lists, physical keys.**
  `TextEdit::with_soft_breaks` and `set_soft_breaks` restore the soft breaks `from_parts`
  can't take, and `TextEdit::clear_history` drops the undo and redo stacks while keeping the
  text, formatting and selection. Return on a blank last line of a code block now leaves the
  block (the blank line becomes a paragraph after it, one undo step) instead of adding another
  code line, and Down or Right at the very end of a trailing code block or quote adds an empty
  paragraph after it; both move what a key does in an existing rich field.
  `TextEdit::allow_marks` and `allow_blocks` limit a field to the formatting its app can store:
  shortcuts, `toggle_mark`, `set_block` and paste skip the rest, and a disallowed
  `⌘B`/`⌘I`/`⌘U` reaches the app's key handlers (`allows_mark` and `allows_block` hide toolbar
  buttons). `Tx::physical_key` gives a key handler the physical key behind a logical one, so a
  shifted shortcut (`⌘⇧7`) matches on every layout; a script's `{"type": "key"}` takes a
  `"code"` for it. Fields without an allow-list keep their behaviour otherwise.

- **Live-only review fixes.** A declined element key handler no longer suppresses an
  ancestor's commit. Moving geometry refreshes hover handlers under a stationary pointer in
  ordinary windows as well as overlays, and schedules their delivery. Debug spot checks
  ignore retained exits and unrelated layout feedback before a mid-frame commit.

- **`Tx::live_only`: an event that moves only live values doesn't re-run the view.** A handler
  that changed nothing the view reads (a canvas camera's `live_offset` and `live_scale`) calls
  `tx.live_only()`, and its event skips the view, diff and layout while the frame still draws
  the live values, hit tests where they put elements and reports `on_drawn_layout`. It works in
  pointer and key handlers with a `Tx`: wheel, pinch, rotation, drag, pointer move, keys, key releases,
  physical input, presses and hover. A handler that declines its event takes it back, and an
  event several handlers take is live-only only if each says so. Debug builds check a
  live-only event a quarter second or more after the last commit by re-running the view
  anyway, and print the handler when what it builds differs (`Runtime::check_live_only(false)`
  turns that off; `Runtime::live_only_events` counts the skips). On a 2,000-card canvas a
  120-frame pinch went from one view run and 13.7 ms of CPU per frame to none and 2.0 ms
  (`examples/pinch_bench.rs`). Nothing changes for a handler that doesn't call it.

- **Element-local points for pointer moves and secondary presses.**
  `El::on_pointer_move_local` and `El::on_secondary_press_local` give a `PointerEvent`: `x` and
  `y` in the element's own layout points, as `Drag` and `WheelEvent` give them, and
  `window_x`, `window_y` and `window()` in window points for a menu's anchor;
  `on_pointer_move_local` also gets a `Tx`. `El::on_drawn_layout_tx` is `on_drawn_layout`
  with a `Tx`. `on_pointer_move`, `on_secondary_press` and `on_drawn_layout` report what they
  did. The crate README has a table of each handler's coordinate space.

- **Combobox filtering springs the popup and fades rows.** Typing in a `Combobox` search used to
  resize the list in one frame and pop rows in and out. The search field now animates its edit
  layout, the viewport's height springs, rows fade, and the empty state stays mounted and
  fades in at 0 height. Things apps can notice: the popover's tree gains a keyed `list` column
  with the `viewport` and an `empty` element in it, and goldens taken mid-filter can differ.

- **`El::animate_layout()` springs a growth row's horizontal layout.** Inside a growth-stack row
  everything snaps while the row's height carries the motion, so a label changing width in place
  (`"Thinking"` → `"Thought for 3s"`, `"Used 1 tool"` → `"Used 2 tools"`) jumped its neighbours.
  Under `.animate_layout()` a row's contents spring x and width on their own layout spring, and
  children the element gains while the row is already in place fade in; y and height stay with
  growth. Nothing changes without the call.

- **A child mounted after its parent visibly arrives enters on its own.** A child inserted under
  a parent still entering used to ride the parent's entrance. The parent's presence runs on for
  ~0.7 s after it reaches full opacity, so a child mounted in that tail popped in at full opacity.
  It now rides only while the parent is visibly entering (presence under 1 - 1/255) or exiting.
  Things apps can notice: content swapped into a freshly mounted parent plays its own
  transition (a `RollingText` digit changing soon after the label fades in rolls in instead of
  appearing in place), and goldens that capture it can differ.

- **Layout nobody can see moves at once.** A node that drew nowhere in sight in the last build
  (outside the window, or clipped away by a scroll or clip) and lands out of sight again now
  takes its new frame, radius and opacity at once instead of springing there. Toggling a sidebar
  over a long timeline used to animate ~290 off-screen rows; the first frame's CPU drops by about
  a fifth. Things apps can notice: `is_animating` and settle events skip those nodes, and a
  node scrolled into view mid-change starts from its new place rather than mid-spring. Shared
  (`matched`) and flying elements still animate.

- **Loops sleep below one 8-bit step of opacity.** A loop (`.cycle`, `.looping`) whose brightest
  frame would draw under 1/255 opacity no longer wakes the app; the cut-off was 0.001. Things
  apps can notice: a loop under a group or parent faded to between 0.001 and 0.004 stops
  scheduling frames.

- **Exits unmount once they fade out of sight.** An exiting node used to stay mounted until its
  presence settled at 1e-5, about 40 frames (~325 ms) after its opacity fell under one 8-bit step.
  It now unmounts the frame its transition's opacity drops below 1/255. Things apps can notice:
  `exited`, focus moves, hit-testing and anything waiting on an unmount happen that much sooner;
  goldens that capture the tail of an exit can differ.

- **A tap's release no longer counts as a drag move.** Letting go where the pointer already was
  used to queue a `Phase::Move` drag event, which instant-commits in most handlers: a slider tap
  sprang the knob toward the tap and then jumped the rest of the way on release. A release now
  sends a move only when the pointer moved since the last move. Things apps can notice: a drag
  handler gets no Move for a release at the last position, and tap-to-seek controls animate
  through the release.

- **The `Tx` scroll requests take key paths**, as `Cx::layout` and `Tx::fly_text` do:
  `scroll_into_view`, `scroll_to_bottom`, `scroll_page`, `scroll_to`, `scroll_to_instant`,
  `scroll_to_x`, `scroll_to_y` and `Runtime::scroll_at_bottom` accept `"list/label"` (leading keys
  are the element's nearest keyed ancestors) as well as a key or full path. A bare key still names
  the first element with it.

- **`rune::extra_args()` works before `run`.** It used to return nothing until `run` had
  parsed the command line, so an app choosing what its window opens from its own flags had to
  call `tooling::Args::parse()` first. Called earlier, it now reads Rune's flags from the
  process's arguments as `run` does (`--help` and malformed flags included); the first read
  still wins, so with `App::args` read it after `run` starts.

- **A field that appears on a layout-read re-run takes the focus it asks for.** When a view
  builds a `TextEdit { focused: true, .. }` field only once `Cx::layout` returns a rect (an
  inline editor placed over a node), the field appears on the commit's re-run, which skipped
  the focus sync, so it stayed unfocused until some later commit. The commit now syncs focus
  again after the re-run.

- **`El::inert()` makes a subtree draw and lay out without taking input.** Presses, hovers,
  cursors, scrolls, drops and tooltips go to whatever is under it, as with HTML's `inert`; nothing
  inside takes or keeps focus, and `ElementInfo::inert` is true for it. `on_layout` and
  `Cx::layout` still see its rects. An editor can draw a document over its canvas without a
  transparent shield to catch presses.

- **`Tx::blur()` drops keyboard focus from a handler**, as a click on empty space does: the
  focused field's `TextEdit::focused` turns false through `on_edit` and keys reach the window's
  `on_key` again. An app no longer needs to clear `focused` in its model to hand keys back.

- **`Tx::set_srgb_blend(bool)` switches blending live.** A handler moves its window (or
  overlay, web canvas or headless run) between linear and sRGB blending from the next frame, as
  if it had launched with `App::srgb_blend`. The frame that switches reconfigures the surface and
  switches the renderer's pipelines once, keeping the glyph atlas, images and layer targets; asking
  for the mode the window already has costs nothing. A `Compositor` sees the new `Frame::format`
  on that frame.

- **`Runtime::set_srgb_blend` is public.** A runtime the app draws itself (built with
  `Runtime::with_text` for a headless snapshot, say) can blend in sRGB as an
  `App::srgb_blend(true)` window does, hover and press tints included, without re-encoding the
  draw list. Changing the mode now asks for a frame. Nothing changes for apps that don't call it.

- **`Tx::open_file` and `Tx::save_file`: file dialogs from a handler.** The callback gets
  `FileDialogResult` (`Ok(Some(path))`, `Ok(None)` on cancel or when the window hides or
  closes, or an error) on a later frame, like `tx.read_clipboard`; the window shows the dialog
  without blocking its event loop (a sheet on macOS via rfd, the window-owned dialog on
  Windows, the desktop portal over zbus on Linux) and needs the `native-file-dialog` feature.
  One dialog at a time: a second request while one is open, or a request on a window that
  stays hidden or on an overlay, gets an error. Headless runs show it as `fileDialog` in
  `--tree` until a `file-dialog` (with a `path`) or `file-dialog-cancel` script step answers
  it, and `headless_run`'s `ScriptEvent` has a new `FileDialog` variant (the enum is
  `#[non_exhaustive]`). Tests use `Runtime::file_dialog()` and `resolve_file_dialog`.

- **Rejected file drops: `El::on_drop_rejected` and `El::on_drop_rejected_hover`.** A drop
  target with either stops a file drag carrying none of the files it accepts, instead of
  passing it to the target around it: `on_drop_rejected_hover(true/false)` follows that drag
  (for a "can't drop here" state) and `on_drop_rejected` gets its files on drop. On a mixed
  drop it gets the files `on_drop_files` didn't. `Runtime::drop_rejected_hovered()` reads the
  state. Every host routes drops through the runtime (macOS, Windows, X11 windows, the
  multi-window desktop and headless `drag_files`/`drop_files` steps), so all get it. Targets
  without them behave as before.

- **A `RollingText` reads as text in lookups.** Its row now reports the whole label as its
  `ElementInfo::text`, so `--tree` JSON and tests that find an element by its text find it,
  as they find a plain `text`. Its glyphs still list one grapheme each. Any element of role
  `"text"` with a label and no text of its own does the same; Rune and rune-kit build no other,
  so trees of apps without a `RollingText` are unchanged.

- **rune-kit: `Button::rolling(key)` and `Button::label_with(..)`.** `rolling` draws the label
  as a `RollingText`, in the button's font and ink, so "Import 3 items" → "Importing…" rolls
  only the changed characters; the button's edges spring on the label's curve
  (`RollingText::SPRING`, width leading), so its padding holds while the width animates. That
  spring also carries the button's colour changes. `label_with(|font, ink| ..)` draws any
  label element in the button's font and ink. Either way the text given to `Button::new` stays
  the accessible label. Buttons without them are unchanged.

- **`El::cache()` rasterizes settled subtrees once and composites them across pan and zoom.**
  Calling `.cache()` on an `El` (with an optional `.cache_key(u64)`) rasterizes the settled
  subtree into an `Rgba16Float` layer target (`layers.rs`) and elides its child instances on
  subsequent frames, compositing a single quad while an ancestor's `live_offset` or `live_scale`
  moves or scales it. Invalidation is automatic from the subtree's relative draw output (state,
  hover, focus, text, layout, theme and scale). Subtrees that are actively animating or contain a
  compositor `El::effect` slot, `NativeSurface`, native `SystemMaterial` or focused text input
  draw live without offscreen thrash and re-cache once settled. Changing ancestor scales composite
  the existing cached texture with bilinear filtering and re-rasterize at the new device-pixel
  resolution after `0.1 s` at rest. Cached textures are capped at `4096×4096` px under an LRU
  byte budget (`Renderer::set_cache_budget`, default 64 MiB). Hit testing, accessibility,
  `elements()` and `on_layout` are unchanged, and apps that don't call `.cache()` pay no extra
  passes or allocations.

  Review fixes: image-backed subtrees draw live so same-size publications cannot leave stale
  texels. Link paint, press paint, outside hover groups and the cache root's blur invalidate
  correctly. View commits always check emitted output; committed scale changes draw live until
  settled instead of trusting a partial model hash. Over-budget fallbacks allocate only the
  visible area when isolation is still needed. Reference replays preserve rounded layer clips,
  reject unsupported layer blur, malformed cache tags and excessive nesting.
- **Grain is symmetric.** `Fx::grain` returned its light half as added light with zero coverage,
  so with `srgb_blend` on each +0.02 of grain became about +40/255 on the framebuffer while the
  dark half only dimmed by 2%: white snow over everything. The light half is now premultiplied
  white (`vec4f(n * a)`), so grain moves a pixel toward white or black by the same fraction in
  either blend mode and the mean stays put. Linear blend shifts a little too: its light half is
  weaker than the old additive one, most at bright destinations.

- **Tree and Table in rune-kit, and the core pieces they need.** `Tree` draws an app-owned
  `TreeModel` (`FileTree` builds one from paths, folders first, single-child folders as one
  row) with indent guides, springing chevrons, icons, change counts, type-ahead, filtering
  and `reveal`. `Table` draws app-owned rows with fixed, flex and min/max columns, drag
  resizing, a sticky header that shows the app's sort, zebra rows, cell builders and
  tooltips for cut-short text. Both are virtualised (50,000 nodes and 100,000 rows scroll
  under 4 ms a frame at p95) and keyboard driven, with tree and grid roles. New core API:
  `El::on_key` (a control that takes its keys before focus navigation, is a Tab stop and
  takes focus from a click inside it), `El::level` and `El::expanded` (outline rows),
  `El::icon_rotation` (a glyph's turn, sprung), `El::help_when_truncated`, `Tx::mods` (the
  modifiers held for a press), `Tx::scroll_into_view_in(scope, key)` (reveal a row of the
  list inside one element only), new `ElementInfo` fields `level`, `expanded` and
  `item_selected`, file and folder glyphs in `rune_kit::glyphs`, and
  `MarkdownStyle::table_like(&TableStyle)`. Things apps can notice: the `--tree` JSON gains
  `level`, `expanded` and `itemSelected` on rows that set them, so tree goldens with selected
  list or menu rows move; Tab now also stops at `on_key` controls when Full Keyboard Access
  is off. Nothing else changes for existing apps.
- **rune-kit has an app shell.** `rune_kit::shell` adds `SplitView` (a resizable, collapsible
  sidebar, the main pane under a toolbar, an optional inspector, and a 48 point title bar band
  that drags the window and clears the traffic lights), `Toolbar`, `Sidebar` with
  `SidebarSection`, `SidebarHeader` and `SidebarRow` (keyboard selection, type-select, nesting,
  badges, inline rename, virtualised rows), the app-owned `PaneState` and
  `shell::is_sidebar_toggle`. New API only; existing output is unchanged.

- **Rects under live transforms: `El::on_drawn_layout`, and `Drag` points in the element's
  own points.** `on_drawn_layout` reports an element's rect as drawn (after scrolling and its
  ancestors' `live_offset`, `live_scale`, `live_rect` and `scale`), on every frame a live value
  moves it with no commit, and where a spring brings it to rest; it's the rect hit tests and
  `ElementInfo::rect` already use. `Drag`'s `x`, `y`, `start_x` and `start_y`, and the pointer
  in `WheelEvent`, `MagnifyEvent`, `RotateEvent` and `on_smart_magnify`, are now in the
  element's own layout points: from its top-left as drawn, divided by the scale it draws at.
  They were measured from its unscrolled layout box, so a drag on a row of a scrolled list
  read the scroll offset too, and one on a zoomed canvas read window points from where the
  element would be at rest. An unscrolled, unscaled element reads what it did.

- **`Tx::scroll_to(key, x, y)` glides a viewport to an offset.** It runs on the spring
  `scroll_to_bottom` uses, clamps each axis to the content as the glide runs (virtual rows
  measure as they mount), ignores axes the viewport doesn't scroll, and a wheel, fling or new
  request takes over. `scroll_to_instant` and `scroll_to_y` jump; a `stick_bottom` viewport
  sent to its end follows it again. `scroll_to_x` now also mounts the virtual rows it lands on
  and stops a glide in progress; otherwise it is unchanged.

- **Canvas input: `on_wheel`, `on_magnify`, other mouse buttons and `on_key_up`.** An element
  can take wheel and trackpad deltas before any scroll view (`El::on_wheel`, with modifiers,
  phase, momentum and precision), trackpad pinches, rotation and smart magnify (`on_magnify`,
  `on_rotate`, `on_smart_magnify`), middle, back and forward button presses and drags through
  its own `on_press` and `on_drag` (`El::pointer_buttons`, `Tx::button`), and key releases
  (`El::on_key_up`). Scripts gain `wheel`, `magnify`, `rotate`, `smart-magnify` and `key-up`
  steps and a `button` on presses and drags; `ScriptEvent` and `PointerAction` gain variants
  for them. Nothing changes for an app without these handlers, except that a scripted
  vertical `scroll` goes through `wheel_xy` once some element has `on_wheel`. On Windows and
  Linux a Ctrl + wheel (a trackpad pinch) over an `on_magnify` element pinches instead of
  scrolling.

- **Wakes no longer pile up in headless runs.** A coarse headless step polled once per
  `wake_after` deadline that fell inside it, so a poll that asked for a wake each time it ran
  (a save timer that rearms, a scripted gesture) started a new chain of wakes every frame: one
  that asked for 505 ms on every poll ran 14 polls a frame by frame 400 and 34 by frame 1000.
  Now a step stops for a timer no sooner than a frame interval (1/60 s) after the last poll, as
  a window's timer floor already did, and each poll takes every deadline due by then: one poll a
  frame at 60 fps or faster, whatever the app asks for. Coarse steps (`rt.frame(0.5)`) still
  stop at deadlines at least a frame interval apart. Windows, overlays, hidden hosts and the
  web already ran one poll for all due deadlines. Things apps can notice: a headless run or
  hot-reload session whose deadlines fall less than 1/60 s apart polls fewer times.

- **Attributed rich-text editing (`TextEdit` marks, blocks, decorations and HTML).** `TextEdit`
  now stores inline [`MarkRange`](crates/rune/src/edit.rs)s (`Mark::Bold`, `Italic`, `Underline`,
  `Strike`, `Code`, `Link(url)`) and per-paragraph [`Block`](crates/rune/src/edit.rs) attributes
  (`Paragraph`, `Bullet(level)`, `Numbered(level)`, `Quote`, `Code`) alongside plain `text`,
  without hidden Markdown characters in the string. Rich keys are opt-in with
  `TextEdit::rich()` (on for `from_html` and `from_parts`): plain fields keep passing `⌘B` /
  `⌘I` / `⌘U` to the app, insert a plain newline on `Shift+Return` and paste plain text, and lay
  out and draw exactly as before. Edits shift and split ranges; typing
  inherits the marks of the character before the caret (except at the start of a link or code
  mark's run); `toggle_mark` / `set_mark` / `remove_mark` / `set_link` and (in a rich field)
  `⌘B` / `⌘I` / `⌘U` toggle marks on the selection or arm a pending typing style on an empty
  selection; and
  `has_mark` / `active_marks` / `active_link` / `active_block` report the active formatting for
  toolbar pressed states. List markers, quote bars and code-block backgrounds draw as layout
  decorations; `Return` continues a list or ends an empty list item, `Shift+Return` inserts a
  soft break in a rich field (`soft_breaks()`), `Backspace` at the start of a list item outdents or exits the
  list, `Tab` / `Shift+Tab` change list nesting (`0..=3`), and inside `Block::Code` `Return`
  inserts a newline, `Tab` inserts `\t` (shaped as one space, matching core text), and inline
  marks do not apply. `El::spans` / `El::decorations` (and `TextInput::decorations` /
  `TextArea::decorations`) paint per-frame `rune::Span`s over `text_input` without mutating the
  model, shifting automatically across active IME preedit so `SyntaxTheme::spans(&ranges,
  offset)` from `rune_kit::syntax::highlight` can be passed straight in; overlapping spans merge,
  later ones winning. `Span` gains builders (`weight`, `bold` (600), `italic`, `mono`, `size`,
  `color`, `bg(color, pad)`, `underline`). Undo, redo and IME
  preserve text, marks, blocks and spans together. `TextEdit::from_html`, `to_html`,
  `selected_html` and `paste_html(html, plain)` import and export a safe HTML subset (`b`/`strong`,
  `i`/`em`, `u`, `s`/`del`/`strike`, `a`, `code`, `pre`, `ul`/`ol`/`li`, `blockquote`,
  `p`/`div`/`br`, and inline `font-weight`/`font-style`/`text-decoration` styles; links keep
  only `http`, `https`, `mailto`, `tel` and scheme-less URLs), and `from_parts`, `marks()`,
  `blocks()` and `paragraphs()` (a `ParagraphInfo` per line) expose the attributed model directly.
  A focused rich field joins the rich clipboard: `⌘C` / `⌘X` write `selected_html()` beside the
  plain text, and paste reads the clipboard's sanitised HTML through `paste_html` when no
  `on_paste` hook takes it. Plain fields still copy and paste text only.
- **Rich clipboard: copy and paste HTML and RTF beside plain text.** New `ClipboardContent`
  (`ClipboardContent::new(text).with_html(..).with_rtf(..)`) and `Tx::copy_rich`, which writes
  every form in one clipboard change; `copy_secret` stays plain text. A field's new
  `.on_input_paste(|s, content, tx| -> bool)` hears a paste before its editor (true takes it),
  the root's `.on_paste_content(..)` is `on_paste` with every form, and
  `Tx::read_clipboard(|s, content, tx| ..)` reads without pasting. Pastes hold the richest form
  the source offered: on macOS `public.html`, else `public.rtf` converted to HTML on the
  clipboard worker; Windows CF_HTML; Linux `text/html`; the web's `text/html`. HTML from the
  clipboard passes through the new `clipboard::sanitize_html` (scripts, handlers, frames,
  embeds, script URLs and loading CSS removed) before an app sees it. Windows, Linux and the web
  write no RTF. Tests: `MemoryClipboard::set_content`/`content`, `Runtime::paste_content`, and
  the script step `{"paste": {"text": .., "html": .., "rtf": ..}}` (public as
  `ScriptEvent::PasteContent`). Things apps can notice: nothing, unless they add a hook. Fields
  without `on_input_paste` and text `on_paste` handlers read only the text, as before; a
  clipboard with HTML and no text reaches only rich hooks. On the web, rich reads use
  `navigator.clipboard.read()`, which some browsers confirm with a paste prompt.

- **`srgb_blend` works with a `Compositor`, and compositors can draw inside layers.** Before,
  `.srgb_blend(true)` was ignored (with a warning) once a compositor was set. Now the frame is
  gamma-blended as without one, `Frame::srgb_blend()` reports it, and a compositor that matches
  the encoding (`Frame::format`, `EffectSite`) looks the same in both modes. `El::effect(mark)`
  adds a box that draws nothing but holds its place in the painter stream, and
  `Renderer::draw_range_with` / `encode_layers_with` call a hook there, inside the enclosing
  isolated layer's pass when there is one, so the compositor's pass takes the layer's opacity,
  blur, dissolve and clip. A faded slot always gets its own layer, so its opacity can't be
  skipped, and the hook runs scissored to the slot's clip rectangle.
  `render::CLIP_WGSL` shares the clip maths with compositor shaders.
  Apps without a compositor or effect slots render exactly as before; one that set `srgb_blend`
  alongside a compositor now gets gamma blending, which its compositor must match.
  `rune-fx` follows: `FxRenderer::add_format`, `draw_layer_in` and `set_srgb_blend` draw its effects
  in a layer's float target and in either blend mode (`demo/fx/in-layer.png`).

- **Traces say what animated.** Each `--trace` frame now carries `quality.why` (what kept the
  frame animating: tracks, growth, scroll, reveal, loop, text-flight, …) and `tk`, the motion
  tracks it sampled as `[element, property, active]`. New `--trace-all` traces unkeyed elements
  too, named by path. `scripts/motion-audit/` records the example apps with it and flags snaps,
  pops, neighbour jumps, overshoot, motion that never settles and animation nobody can see.
  Things apps can notice: trace files are larger (the new fields, and every element with
  `--trace-all`); element rows are unchanged.

- **Syntax highlighting is shared, swappable and covers Python and Go.** The scanners behind
  Markdown's coloured fences moved to `rune_kit::syntax`, which builds without the `markdown`
  feature: `syntax::highlight(lang, code)` returns cached `(Range<usize>, Paint)` ranges and
  `SyntaxTheme::spans(&ranges, offset)` turns them into colour-only spans for any rich text,
  such as a composer's code. `Markdown::highlight(|lang, code| ...)` (or `highlighter`) swaps in
  an app's own highlighter, which gets the fence's language as written, including ones the
  built-in scanners don't know. New built-in languages: Python, Go and unified diffs (for
  callers; ```` ```diff ```` fences keep their tinted rows). `Syntax` gains `Inserted` and
  `Deleted`, and `MarkdownStyle::syntax_theme` sets every token colour at once. Things apps
  can notice: ```` ```python ```` and ```` ```go ```` fences (and aliases such as `py`,
  `golang`) are now coloured, so goldens with them move; turn that off with
  `syntax_highlight(false)`. Other output is unchanged.
- **Tabs draw as spaces, and line up in Markdown code.** Text layout drew a tab as the font's
  missing-glyph box (or nothing, depending on the font); it now shapes each tab as one space,
  so byte ranges, carets and copying are unchanged. Markdown code blocks and diff lines pad
  each tab with spaces to the next four-column stop; the padding isn't copied, so a copy
  still gets the tab. Goldens with tabs in text move.
- **`native-file-dialog` passes `cargo deny`.** On Linux the Open and Save dialogs now call the XDG
  desktop portal's FileChooser directly over zbus 6, which Rune already uses for global shortcuts,
  instead of through rfd's portal backend (ashpd and zbus 5, whose `zvariant`, `zbus_names` and
  `zvariant_derive` are flagged unmaintained: RUSTSEC-2026-0370, 0371 and 0372). rfd moves to 0.17
  and is only a macOS and Windows dependency, with no features; the Linux tree loses 75 crates.
  Things apps can notice: on Linux a desktop with no portal FileChooser now returns an error
  instead of falling back to zenity, a dialog dropped before it answers is closed, and the dialog
  is parented to the focused X11 window (Wayland windows stay unparented). Paths, filters, the
  starting folder, the suggested name and cancellation behave as before. Apps need no code
  changes.

- **rune-fx fills.** `Fx::fill(params)` paints a node's shape, or with `.border(w)` a band inside
  its edge: liquid metal (chrome, gold, rose gold, gunmetal; also as a tube border), mesh
  gradient, dither (Bayer 2×2/4×4/8×8, blue noise, white noise; of a ramp, noise, the backdrop or
  an image), grain gradient (linear, radial, conic), fluted glass, metaballs, swirl, voronoi,
  waves and halftone. Each has typed params, `Default` and named presets. `fill_styles()` is a
  catalog for tools: ids, display names, params with ranges and units, presets,
  `FillParams::get`/`set` by param id and `to_rust()`. Things apps can notice: `Fx` has a new
  variant, `Fill`, so an exhaustive `match` on `Fx` needs an arm; `FillStyle` and `FillParams`
  are `#[non_exhaustive]`. `demo/fx/timings.md` was re-measured on an Apple M4 Pro, so its
  numbers for the older effects moved too.

- **Reopening a Mac app brings its window back.** A click on the Dock icon, or opening the app
  again from Finder, Spotlight or `open` while it runs, now shows and activates a window when
  none shows: a minimized one, else the one Rune hid last (⌘W with `keep_running`, `hide`), else
  the main window, and unhides an app hidden with ⌘H. Before, a window hidden that way stayed
  hidden, because AppKit only brings back minimized windows and winit's app delegate didn't answer
  `applicationShouldHandleReopen:hasVisibleWindows:`; Rune now adds that method. Launcher and
  other nonactivating panels, overlays, windows that closed for good and accessory apps never
  come back. New `App::on_reopen(|s, has_visible_windows, tx| -> bool)` (and
  `WindowSpec::on_reopen`) runs first; true skips the default. Other platforms are unchanged.
  Headless scripts take `{"reopen": true}`. Things apps can notice: a hidden window now comes
  back on a Dock click. An app that installs its own reopen Apple event handler replaces
  AppKit's, which is what asks the delegate, so neither the hook nor the default runs; drop the
  handler and use `on_reopen`.

- **Theme tokens hash.** `Color`, `TextStyle` and rune-kit's `Palette`, `StateColors`, `Type`
  and `Radius` implement `Hash`, so a row can pass the tokens it reads to `memo` as its version
  instead of hashing their `Debug` text. Floats hash by bits, `-0.0` as `0.0` (they compare
  equal) and every NaN alike; none of them is `Eq`, since NaN isn't equal to itself.
- **A virtual list's first showing mounts its viewport, not three windows.** A new scroll
  viewport has no measured height, so a virtual list shown for the first time used the window's
  height as its viewport and mounted two of those either side: a 200-cell emoji grid in a
  five-row viewport built all 20 of its rows. Now the first showing estimates the viewport from
  the tree (the window's height less the fixed-height siblings, padding and gaps of the columns
  around it, or the scroll viewport's own height; in a window that fits its content or an
  out-of-flow popover, only the viewport's own height and `max_h`), and an index-built list
  (`virtual_rows`) mounts the viewport's rows and half a viewport of overscan, as it does when
  all its rows are new; the rest of the overscan mounts over the next few frames. An estimate
  short of the laid out height retries once, with the measured viewport, before presenting, so
  no frame shows blank rows. `scroll_into_view` of a row taller than its estimate now lands it
  in view: each placement clamps to the end the rows it mounted measure. `virtualize` lists keep
  their full overscan from the better estimate. In a launcher-shaped bench (`tree_swap --cells
  200 --lazy-grid`) the grid's first showing lays out 288 nodes instead of 629, the swap back
  to a 200-row list 107 instead of 240, and their commits cost 40–50% less. Tests that count
  the rows mounted or built in a list's first frame see fewer.
- **Horizontal scrolling: `El::scroll_x()` and `El::scroll_xy()`.** A viewport scrolls its
  first child sideways (a filmstrip, a wide code block or table, a tab strip), or both ways.
  The content takes its natural width, unwrapped, and at least the viewport's, unless it sets a
  width other than `w_full`. Each axis has its
  own offset, momentum and clamping. A wheel or trackpad gesture locks to the axis of its first
  delta until it ends, momentum included: mostly horizontal moves the innermost horizontal
  viewport under the pointer, and anything else the innermost vertical one, so a diagonal never
  moves both. A web touch pan does the same. `tx.scroll_into_view` and caret follow work on both
  axes, `.scroll_edge_fade` fades left and right, and `tx.scroll_to_x(key, x)` is new.
  `Markdown::scroll_wide(true)` lets code blocks and tables scroll instead of wrapping and
  shrinking (off by default). Scripts take `dx` on `scroll` and `vx` on `fling`.
  Things apps can notice: Shift with a mouse wheel now scrolls horizontally on Windows, Linux
  and the web, and a horizontal trackpad swipe that used to move a vertical list by its tiny
  vertical part only does so when no horizontal viewport is under the pointer. Hosts that drive
  `Runtime` themselves can report wheels through `Runtime::wheel_xy` with `wheel_began` and
  `wheel_ended`; `Runtime::wheel` and `Runtime::fling` are unchanged. Glyphs wholly outside
  their clip are skipped on x as they already were on y: nothing drawn changes, but the glyph
  count in frame stats drops for clipped text.
- **`El::on_drop_files`: drop files in from other apps.** Files dragged from Finder, Explorer
  or a file manager go to the innermost `on_drop_files` element under the pointer that accepts
  any of them, in one call with the files it accepts. `.accepts_files(|path| ..)` or
  `.accepts_extensions(&["png"])` filters them; a target that accepts none passes the drag to
  the next accepting element around it. `.on_drop_hover(|s, over, tx| ..)` hears the drag
  reach and leave the target, for a highlight. A drop over no accepting element goes to the
  root's `on_drop_files` (or, while a modal is open, the modal's, and only inside it), else
  it's ignored. Drop zones take no pointer events, so one laid over other elements leaves
  their clicks, menus and help. winit gives no drag position, so the window
  samples the system pointer while a drag is over it. macOS, Windows and X11; Wayland and the
  web report no file drags. Headless scripts use `drag_files` and `drop_files` events, and
  `headless_run` has `ScriptEvent::DragFiles`, `DropFiles` and `DragFilesLeave`.
- **Covered and minimized windows don't run their view.** A shown window that was fully
  covered (macOS and X11 report it), minimized, or waiting out a missed frame used to build
  its view and lay it out on every update, wake and due timer. Now it runs updates, `poll`,
  timers, `on_close` and queued input as a hidden window does, and the view waits, dirty, for
  the frame that uncovers it, which builds once with the latest state. Uncovering already
  landed running motion; a window restored from minimized on Windows, which gets no occlusion
  event, now lands it too, so it doesn't play out what changed meanwhile, and asks for its frame
  when it comes back at the same size. So does any window whose next drawn frame follows a
  minimize no event reported (X11). On macOS, whether a window can draw follows occlusion alone,
  since `isMiniaturized` stays set into a deminiaturize. A `scroll_to_bottom` asked while a
  window was covered or hidden lands in the frame that shows it, rather than gliding after. A
  modifier change that arrives after an update, before its frame, goes to the focused handler
  of the view that update builds. An app that relied on side effects in view code while covered
  will see them only when the window shows. The dev panel's tree still builds on demand.
- **`Recorder`: record the microphone to a file.** `Recorder::to_file("note.m4a")` records AAC
  (a `.wav` path, or `.format(RecordFormat::Wav)`, records WAV; the format decides the
  file's extension) with `start`, `pause`, `resume`,
  `stop` (returns a `Recording { path, duration }`) and `cancel` (deletes a recording in
  progress). `state()`, `elapsed()` and a smoothed `level()` (peak and RMS in dBFS, `meter()`
  in 0..1) are read without allocating; `live_level()` drives a bound meter that redraws while
  recording and draws nothing while idle. `Recorder::permission()` and `request_permission`
  report and ask for microphone access; starting without it ends in `Failed` with a clear
  message, and a bundled macOS app without `NSMicrophoneUsageDescription` fails to start
  instead of being ended by the system. macOS records with AVAudioRecorder and reports an
  unplugged or switched input as `Interrupted`, keeping what was captured. Other platforms
  fail with "recording isn't supported on this platform". Headless runs record from a
  `FakeMicrophone` on the virtual clock and write a real WAV on stop. New exports:
  `Recorder`, `Recording`, `RecorderState`, `RecordFormat`, `MicPermission`, `Level`,
  `FakeMicrophone`; an app with its own type of one of these names imported by glob needs a
  qualified path.
- **Web keyboard focus and composition stay with the field.** Leaving an editor returns
  keyboard focus to the canvas; secure fields use an emptied password input. Taps no longer
  force focus through the canvas, recomposition replaces its old range, and mobile
  `beforeinput` Enter follows Rune's submit/newline rules. Rust sends only changed text
  windows, continuous pointer input waits for rAF, unsupported window requests warn without
  covering the app, and tab visibility updates `AppHandle::is_visible`.

- **Web follows conditional updates and page navigation.** Browser handles use the same
  `update_if` runtime path as native; the textarea routes PageUp and PageDown to Rune.

- **Build static web apps.** `scripts/web-build.sh` packages a wasm app, JavaScript snippets
  and an HTML entrypoint for HTTPS hosting. `examples/web-hello` exercises message input,
  scrolling and clipboard; `WEB.md` covers app integration and browser limits.

- **Browser clipboard, links and viewport.** Web copies and paste requests use the browser
  clipboard; secret-copy requests fail explicitly. `web::open_url` opens safe link schemes
  without an opener. `Cx::safe_area` exposes browser insets, and the canvas tracks the visual
  viewport when the software keyboard opens. Reduced-motion preferences reach the runtime.

- **Web text and touch input.** A hidden textarea mirrors a sliding UTF-16 context for
  browser composition, predictive replacements, selection and paste. Touch pans use
  momentum and a damped edge return; completed taps focus editors synchronously.

- **Web fonts are self-contained.** The browser target bundles Inter Regular and IBM Plex
  Mono Regular, accepts app-provided font bytes, and never scans system font directories.
  Apps supply additional weights, symbols and scripts through `AppSpec::fonts`.

- **Experimental WebGPU canvas host.** `App::run` on `wasm32-unknown-unknown` starts one
  winit canvas with asynchronous GPU initialization and demand-driven browser frames.
  `rune::time::Instant` uses the browser clock on wasm and the standard clock on native.
  Overlays, extra windows, native window hooks and blocking GPU operations are unsupported.

- **Page keys skip a scroll view with nothing to scroll, and `Tx::scroll_page`.** With a chat's
  short multi-line composer focused, Page Up and Page Down paged the composer's own scroll area
  (a no-op) instead of the message list beside it. Rune now skips scroll views whose content
  fits when it picks one: around the focused element, then around the pointer, then the first
  in the tree. A view that overflows still keeps the keys at its edge, as an NSTextView does.
  New `Tx::scroll_page(key, pages)` pages a viewport by `pages` × 90% of its height with the
  same rules, so an app can route the keys itself from `on_input_key` or `on_key`.
- **Selectable Markdown, and selection across texts with `El::selection_group`.** New
  `Markdown::selectable(true)` lets people select and copy a whole Markdown block as one
  selection: a drag runs from a paragraph through list items into a code block, Shift-click
  extends, Cmd+A selects the block and Cmd+C copies the text as drawn, as a web browser does (a
  newline between blocks, a tab between table cells, no list markers, fences or emphasis
  markers). Links still open on a click. The highlight is the kit's `selection()`, overridden
  with `MarkdownStyle::selection`. Underneath, `El::selection_group(true)` makes any element's
  selectable texts one selection in tree order, and `El::uncopied(range)` marks bytes that draw
  but aren't copied (Markdown's padding around inline code). A selected text no longer loses
  its selection when text is appended after it (a message streaming in); a change inside the
  selected part still drops it, as does its virtual row scrolling out. An elided text
  (`max_lines`, `truncate`) selects through its ellipsis and copies whole.
- **`Tx::fly_text` lands on the destination's text, not its box.** The source was measured by
  its laid-out lines but the destination by the whole element, so a destination wider than its
  text (a message body stretched across a column) scaled the flight into a large ghost and a
  small copy. Both ends now use the same rule each frame: the lines' extent from their offsets
  and advances, placed by the text's alignment, clamped to the content box. A flight between
  same-size text now stays at rest scale, and centred or end-aligned text flies from and to
  where it's drawn.
- **Return and Escape during an input method composition only confirm or cancel it.** A
  Return or Escape key event that reaches a field while it has a preedit (Japanese, Chinese
  or Korean input, say) no longer runs its `on_submit`, presses the default button, inserts a
  newline, blurs the field or reaches `on_key`.
- **`El::on_step` gives a slider its keyboard.** While the element is focused, Left and Down
  call it with -1, Right and Up with +1, Page Down with -10 and Page Up with +10, repeating
  while held; `El::on_step_limit` takes Home (`false`) and End (`true`) to the ends, and
  without it they go on to the app. A slider built from `on_drag` adds them to become
  focusable and adjustable from the keyboard, with role `slider`. Its keys come before its
  `focus_group`'s arrows.
- **Breaking: Return no longer presses a focused button; Space does, and Return goes to the
  default button.** As in AppKit, Space presses the focused button, checkbox, switch, radio,
  segment or popup button, and Return presses the new `El::default_button()` in the window or
  open modal, else reaches `on_key` (with `tx.focused()` naming the focused control). A
  focused element that takes Return keeps it: a text field with `on_submit`, a row of a
  `menu`, `listbox`, `list`, `tree` or `grid` group, a `focus_highlight` row, or a link. A
  text field without `on_submit` passes Return to the default button. An app
  that relied on Return pressing a focused button should mark its primary action
  `.default_button()`, or handle Return in `on_key`. Return no longer opens a focused popup
  button (role `combobox`) either; Space, Up and Down still do. Shift-modified keys other than
  Tab and typed text no longer navigate a `focus_group` or press anything: Shift-Space,
  Shift-Return and Shift-arrows go on to `on_key`. The rune README's "Key routing" has the
  full table.
- **A focusable control with no `focus_ring` draws AppKit's ring.** Focused from the keyboard,
  a `focusable(true)` element whose ring was never set (on it or a child, short of another
  focusable) now draws a 3 pt ring just outside its border box in `cx.focus_color()`,
  following its corner radii. Before, it drew nothing. Text fields, focused links, `focus_highlight` rows and
  the items of `menu`, `listbox`, `list`, `tree` and `grid` groups still draw none, and rune-kit
  controls keep their own rings. `El::no_focus_ring()` turns the ring off for a control that
  shows focus its own way.
- **`cx.accent()` and `cx.focus_color()` follow the user's accent colour.** `rune::system_accent()`
  returns a `SystemAccent`: on macOS the accent picked in System Settings
  (`NSColor.controlAccentColor` and `keyboardFocusIndicatorColor`), elsewhere and headless
  macOS's default blue with focus rings at 50%. Views read it for the window's appearance with
  `Cx::accent` and `Cx::focus_color`. When the user picks another accent every window and
  overlay re-runs its view, memos included. Tests set one with `Runtime::set_system_accent`. A
  rune-kit theme can follow it with
  `Kit::from_palette_focus(seed, PaletteFocus::Custom(cx.focus_color()))`.
- **`Tx::focused()` tells `on_key` whether a control has focus.** A window's `on_key` hears a
  key first while nothing has focus and again after a focused control passes it on, and
  couldn't tell the two apart. In key handlers `tx.focused()` is now the focused element's key
  (`#n` for an unkeyed one), or `None` when nothing has focus, so plain arrows can change a
  settings pane only while no control is focused. The rune README's "Key routing" lists the
  whole order a key takes.
- **Mac apps get standard Edit and Window menus; `App::standard_menus()` opts accessory apps in.**
  A regular app whose menu bar is still winit's default now also has Edit (Undo ⌘Z, Redo ⇧⌘Z,
  Cut, Copy, Paste, Select All) and Window (Minimize ⌘M, Close ⌘W); an accessory app gets them
  with `App::standard_menus()`, and never a Quit item. Items go to the first responder, so AppKit
  windows (an open panel, an updater) get their editing and close keys. In a Rune window every
  one of these keys still reaches text fields, `on_key` and shortcuts first, once, as before.
  What changes: a ⌘W or ⌘M that nothing takes now closes or minimizes a window with that
  title-bar button, as clicking it would (`on_close`, `keep_running`). A custom `MenuItem` with
  the same equivalent (Close Tab ⌘W) keeps it, and new custom menus go in the Mac order (File
  before Edit, Help last). Apps that set their own menu bar are unchanged. Details under
  "Standard menus" in `src/shortcuts/README.md`.
- **`Tx::minimize()` and `AppHandle::minimize()` minimize a window** as its minimize button
  would (winit's `set_minimized` off macOS). A close or quit in the same change wins. Hidden
  windows, panels, launchers, overlays, borderless windows and windows without a minimize button
  ignore it, saying so in debug builds. On `AppHandle`, `minimize` and `focus` are last-wins, a
  later `open`, `hide` or `close` cancels it, and after a `show` it minimizes the shown window.
- **`KeyEvent::is_command(c)`** matches Cmd (Ctrl off macOS) plus any character, including
  Cmd-Space's `Key::Text(" ")` form. The `Key::Command` docs now say it carries any character
  (⌘, and ⌘1 too), lowercased, not just editing letters.
- **`El::time_scale(k)` runs one subtree's animations at `k` times normal speed.** Springs,
  transitions (enter, exit, layout), implicit animations, hover and press fades, loops and word
  reveals inside it advance by `dt × k`; `0.0` freezes them exactly and requests no frames, and
  changing `k` carries on from the same place. Nested scales multiply, and all of them multiply
  with `Cx::time_scale`, which still scales the whole window. State keeps updating (a hover is
  tracked, only its fade waits), an exiting element drops its own factor and follows its
  ancestors still in the tree (so an exit in a frozen subtree finishes once it resumes), and
  scrolling, growth stacks and `Tween` keep the window's clock. Trees that don't use it are
  unchanged. In `rune-motion`, `AnimationStore::set_node_time_scale` and
  `Scene::set_time_scale` do the same per node, and `AnimationStore::logical_end` reports
  store time for a scaled node (infinite while frozen). `Choreography` keeps scheduling in
  store time: a cue on a frozen step's arrival waits for it to resume, and `PhaseAnimator`
  advances once a frozen phase resumes.
- **A wrapping tooltip child wraps inside the box.** `Tooltip::child(text(..).wrap())` laid out
  at its one-line width and drew past `TooltipStyle::max_width`; it now shrinks to the box and
  wraps there without `.shrink()`, beside a chip that keeps its width. Wrapping text nested
  deeper in a child still needs `.shrink()` on that child. `El::wraps()` reports whether an
  element is wrapping text.
- **`App::tab_visits_controls(true)` lets Tab reach every control.** By default Tab follows
  macOS: with Keyboard navigation off it visits only text fields and lists. With this opt-in
  (also `WindowSpec::tab_visits_controls` and `Runtime::set_tab_visits_controls`) Tab and
  Shift-Tab visit buttons and every other focusable control whatever the system setting.
- **`Cx::titlebar_inset()` says where the traffic lights end.** It is the zoom button's
  trailing edge in window points, for a top bar's leading inset, and 0 when no buttons sit over
  the content: in full screen, in a titled window whose content starts below its titlebar, and
  off macOS. macOS windows update it on resize and when full screen starts or ends, and the
  view re-runs. Leaving full screen also puts `Chrome::traffic_lights` back where the app placed
  them. Headless runs, `run_windows` included, report `Chrome::titlebar_inset()` (67 pt for
  AppKit's default placement; 0 for a borderless window), and `Runtime::set_titlebar_inset`
  sets it in tests.
- **A modal whose field opens focused returns focus to what had it.** Opening a `.modal(..)`
  in the same update that set its `text_input`'s `edit.focused` recorded the field as the place
  focus returns to, so closing the modal left nothing focused. It now records what had focus
  before that update, such as the button that opened it. Focus the closing update moves
  outside the modal (`s.modal = false; s.search.focused = true`) stays where it was sent.
- **`rune` re-exports the motion types its transition and animation API takes.** `Vec2` (for
  `Transition::scale_anchor` and `Edge::direction`), `Effect`, `Reveal`, `CubicBezier`,
  `StepPosition`, `Decay`, `Blend` and `Velocity` are now nameable as `rune::…`, so an app
  needn't depend on `rune-motion`. `rune::Blend` is the animation blend; the GPU blend mode stays
  `rune::render::Blend`. An app that glob-imports `rune::*` and defines one of these names
  itself keeps its own.
- **`Key::PageUp` and `Key::PageDown`, and page keys scroll.** Page Up and Page Down used to
  reach no handler; winit, Wayland layer shells and headless `--keys` (`PageUp`, `PageDown`)
  now send them. A key nobody claims scrolls the focused element's scroll view (else the one
  under the pointer, else the first) by 90% of its height, and Home and End scroll it to the
  top and bottom, as an NSScrollView does. Text fields pass page keys on, so the caret stays
  put while the view scrolls. `Key` is now `#[non_exhaustive]`: a `match` on it needs a `_` arm.
- **A duplicate sibling key no longer closes the app.** Two siblings with one key used to stop
  a debug build at an assertion and panic inside Taffy in a release build, closing the window.
  Every build now keeps the first sibling's key, renames each later one `key#dup1`, `key#dup2`,
  ... in tree order so it still draws, and prints the warning naming both call sites once per
  key. Duplicate row keys in a `virtualize` or `virtual_rows` list, which panicked in every
  build, are renamed the same way. Trees without duplicates are unchanged, and checking them
  allocates nothing.
- **Ogg and WebM audio plays on macOS.** `Player::audio` on an Ogg Vorbis, Ogg Opus or
  WebM/Matroska Opus clip failed at once (AVFoundation can't read them). The new default
  `ogg-audio` feature decodes such a clip in pure Rust (symphonia, opuscule), streaming it to a
  temporary WAV that AVPlayer plays, so state, time, seeking, rate, looping and request headers
  work as for any clip. Clips are picked by extension, or by their bytes when a URL has none.
  The compressed clip is held in memory while it decodes, and the WAV takes about 11 MB a
  minute of 48 kHz stereo. A clip longer than a WAV holds (about 6 hours of that) fails with a
  clear message. Video in those containers still fails at once, and with
  `default-features = false` so does this audio. The feature adds symphonia's Ogg, Matroska
  and Vorbis crates and opuscule to macOS builds.
- **Headless audio follows the virtual clock.** A headless run reported a playing audio clip
  with its time stuck at 0. Its time now advances with the runtime clock of the thread that
  made it, as a video's does: `pause`, `seek`, `set_rate`, looping and the end (`Ended`, and
  the clip's waker hears it) behave as in a window, still without a player or any sound. A
  still or `--frames` capture of a progress bar now shows it moving. Each new `Runtime` starts
  that clock again at 0.
- **`Player::request(url).header(name, value)` opens media with HTTP request headers.** A
  `MediaRequest` builds a player with `.audio()` or `.video()`, and every request for the clip
  (load, poster, playback) sends its headers, so media behind `Authorization` streams instead
  of being downloaded first. On macOS it uses AVURLAsset's HTTP header option. Header values
  never print in `Debug`. `MediaSource` is unchanged.
- **`Transition::allow_small_scale()` lets a small glyph scale from below 0.9.** Transitions
  keep scale within [0.9, 1.1] by default. `Transition::scale(0.6).allow_small_scale()` lowers
  the floor to `SMALL_SCALE_FLOOR` (0.5), for an icon in a button that barely moves at 0.9;
  nothing scales to or from 0. `Effect` gains a pub `small_scale` field: an `Effect { .. }`
  literal without `..Effect::IDENTITY` needs it.
- **A node leaves with the transition of its last render.** `El::transition` was read only when
  a node mounted or revived, so a later render couldn't change how it leaves. Each render now
  updates a present node's transition, as in SwiftUI: removal uses the removal effect and exit
  spring from the last commit before it, and a revive enters with the latest insertion. An
  entrance already running keeps the look it started with. An element whose transition changes
  between renders (or that drops one, falling back to `.opacity`) now exits differently than
  before.
- **`El::virtual_rows(count, estimate, key, build)` builds a virtual list's rows by index.**
  It is `virtualize` with rows the list builds itself: layout calls `build(state, i)` only
  for the rows in the mounted window, after it knows where the window is, so a commit costs a
  key check per row plus the rows it mounts. A 5,000-row list whose counter ticks every frame
  no longer builds or looks up every row in the view. Rows keep their measured heights by key,
  prepending or inserting keeps the reader's row in place, `scroll_into_view` reaches any row,
  and a focused field keeps typing after its row scrolls out. Rows new to the window beyond
  the viewport mount half a viewport per side per frame, and the list asks for frames until
  they fill, so a page loaded above the reader or a jump lays out a little text each frame
  instead of three viewports at once. A list's first showing mounts all of them. Write row
  handlers to find their item by key or id, never by `i`. Elements in rows that aren't built
  don't exist: the inspector, accessibility, Tab order and text selection see built rows
  only. Both kinds of virtual list now show their last rows in the same frame when rows are
  removed below the scroll offset, where the viewport used to stay blank until the next
  commit. `scroll_into_view` on a row that measures taller than its estimate now lands it in
  the same frame.
- **`update_if` for updates that may change nothing.** `AppHandle::update_if`,
  `OverlayHandle::update_if` and `Runtime::update_if` take a closure that returns whether it
  changed state the view reads, as `poll` does. On false the view doesn't re-run, a shown window
  or overlay doesn't redraw, a hidden one doesn't poll, and a hidden launcher or prewarmed overlay
  doesn't refresh, while the closure's `Tx` requests still happen (clipboard, close and quit,
  menus, `wake_after` scheduled from now; a scroll or `fly_text` still takes a frame). A batch
  drained together commits once if any returned true. Returning false after changing state the
  view reads leaves the screen stale until the next change. `update` is unchanged. For a timer
  that posts every 500 ms with nothing new, a 300-row view's frame drops from 0.70 ms to 0.11 ms
  median (0 of 100 builds).
- **Tooltip labels wrap at the style's max width.** `Tooltip::label` and `Tooltip::text` stayed
  on one line past `TooltipStyle::max_width` and drew outside the box; they now wrap inside it,
  so a kit's Standard tooltip wraps at 320 pt. Tooltips without a max width are unchanged.
- **`El::aspect_fit(ratio)` sizes an element to the largest box at a ratio that fits its room.**
  As SwiftUI's `.aspectRatio(ratio, contentMode: .fit)`, the element takes the free space along
  its parent's main axis and the whole cross side (within `max_w` and `max_h`), then lays out at
  the largest box at `ratio` inside it, so siblings such as a caption follow the fitted box and
  the parent's alignment places it. Its own grow and its children never size the room, so equal
  cells share a row equally. Layout runs a second pass only for trees that use it.
- **`El::max_lines(n)` clamps wrapped text to `n` lines.** Like CSS `line-clamp`, it wraps,
  measures at most `n` lines tall and ends the last line with "…" when more would follow. The
  cut falls between graphemes and refits on resize, inside a word too wide for the box; spans
  and links keep their looks, and copy and accessibility keep the whole string. `.truncate()`
  and `.truncate_fade()` turn it off, and `.max_lines` turns them off. A `text_input` ignores it
  and the truncations. Text without it is unchanged.
- **Markdown tables take their own font and cell padding, and can sit flush left.**
  `MarkdownStyle::table_font(TextStyle)` sets the cells' font (the body's by default; a web
  table at `0.93em` is the body at 0.93 of its size), `table_cell_padding(x, y)` the padding (10
  by 6 by default) and `table_flush_left(true)` drops the first column's left padding so it lines
  up with the paragraphs around it. A table without an outline (`table_outline(None)`) no longer
  insets its cells by the half-point border it doesn't draw, so it sits 0.5 pt further left and
  is 1 pt narrower.
- **A Markdown table that ends the text sizes to its last row.** rune-kit read a last table row
  with no newline after it as still streaming in and left it out of the column widths, so a
  table ending a trimmed note could wrap that row. It now does so only under the new
  `Markdown::streaming(true)`, which a stream sets while its text is still arriving; otherwise
  the text is taken as whole. A stream that wants its columns held passes it.
- **`ui-monospace` in `AppSpec::families` names the platform's monospaced font.** The sans and
  mono slots are CSS lists, and CSS's `ui-monospace` in one now stands for `Family::SystemMono`'s
  families (SF Mono on macOS, whose family name is hidden), followed by the rest of the list. With
  `families: ("Inter", "ui-monospace")`, `Family::Mono`, `.mono()` and mono rich spans (rune-kit
  Markdown's inline code among them) shape with the same font as a `Family::SystemMono` code
  block. Where no monospaced font is installed (the web, or Linux without one) a list naming
  no other family falls back to the sans slot. Apps that don't name it are unchanged.
- **Live editing for external app crates and on the desk Mac.** `scripts/rune-dev
  --manifest-path APP/Cargo.toml --patch 'src/ui/**/*.rs'` hot-edits an app outside this
  repo; the app adds only `hot = ["rune/hot"]`. With `--desk` the app compiles on the build
  Mac and runs on this one, patched over an SSH tunnel after a binary hash and platform check.
  `RUNE_HOT=1` now enables `--hot` behavior. `run_windows*`, `WindowSpec::from_app` and
  `App::run` patch every window from one devserver connection, `poll` and `on_key` patch like
  the view, and `--snap-on-patch` renders a `run_windows` or `App::window` app's `--window`
  headless. A panicking patched view, `poll` or `on_key` falls back to the startup one in
  every window, and patches reach overlays and hidden launchers too. Hot builds only;
  feature-off builds are unchanged.

- **Hidden overlays and windows don't run their view.** A hidden overlay used to build its view
  and lay it out on every change (an `OverlayHandle::update`, a `setup` waker, a due
  `wake_after`, a clipboard or pop-up answer), and a hidden window did on every due
  `wake_after`. Now handlers, `poll`, timers, queued input, drag ends, clipboard and window
  commands still run while hidden, but the view waits for the frame that shows the host, which
  builds it once with the latest state. An app that relied on side effects in view code while
  hidden will see them only on show. Hosts that keep a ready frame while hidden (a launcher
  panel's cached pixels, a prewarmed macOS overlay's retained commit) still refresh it, but at
  most every 250 ms: the first change after a quiet spell renders at once, later ones wait. A
  show whose state changed since that frame renders it first, as for an update in the same
  turn. A `scroll_to_bottom`, `scroll_into_view` or `fly_text` asked for while hidden counts
  as a change; a `wake_after` alone doesn't. A window with `fit_content_height` whose content
  changed while hidden builds it as it shows and opens at the new height.
- **Occluded windows and overlays no longer spin or leak; per-surface frame stats.** A surface
  whose drawable acquisition kept failing (covered, or the displays asleep) could retry at once,
  and an overlay's failed re-acquire turned the loop with `ControlFlow::Poll`. Every miss now
  waits for its retry (occluded: 250 ms doubling to 2 s; outdated: 250 ms; timed out: a frame)
  with the display link stopped, so neither input nor display ticks try sooner; timers due
  meanwhile still run. A hidden launcher refresh that gets no drawable retries the same way and
  never reveals the pixels it failed to replace. On macOS Rune reads the window's
  occlusion before asking wgpu, so a covered surface makes no failed `get_current_texture` call
  at all. That matters on wgpu-core 30.0.1, which leaks a texture id per failed acquisition
  (fixed on wgpu's `v30` branch, unreleased); an app that wants the fix now can add
  `[patch.crates-io] wgpu-core = { git = "https://github.com/gfx-rs/wgpu", branch = "v30" }`. A
  display link that failed to start while the displays slept is retried after the next present.
  New `WindowHandle::surface_stats()`, `OverlayHandle::surface_stats()` and
  `Runtime::surface_stats()` return a `SurfaceStats` (redraws, unpaced redraws, presents, failed
  acquisitions by kind, skipped acquisitions, loop turns and wakes, as current totals and
  per-second rates, plus the `Pacing`). The dev panel shows them under Frames, a native `--trace`
  prints them when it closes, and `App::dev_stats(fn() -> String)` adds the app's own line beside
  them. A surface that fails more than 400 acquisitions, or makes more than 400 redraws that
  waited for nothing (no display tick, vsync, timer or event), a second for two seconds prints
  one `rune: surface has spun` line, in release builds too.
- **`memo(id, version, || el)` reuses a subtree whose inputs haven't changed.** The closure
  runs again only when `version` or the window's dark mode, scale or reduced-motion preference
  changes; otherwise the view gets the subtree the last build returned, with the same node ids,
  hover, focus and springs. An offscreen `virtualize` row that is memoized costs a lookup, so a
  long transcript whose counter ticks every frame no longer rebuilds every message.
  `Cx::layout` and `Cx::caret` read inside the closure are tracked: a reuse still re-runs the
  view when they move, and builds again when one returns something else. `TextEdit` now
  implements `Hash` over what a field draws (text, selection, composition, focus), so a
  memoized field passes the edit itself as its version. Hot reload and dev tunables drop the
  cache. Virtualized lists also find their mounted rows by binary search now, rather than
  walking every row each commit.
- **Markdown keeps every document on screen parsed.** `Markdown` and `markdown::parse_cached`
  kept the last 32 sources, so a transcript showing more than 32 messages parsed every one of
  them on every commit. The cache (and the code-colour and table-width caches beside it) now
  keeps whatever the last two commits drew, and evicts the least recently drawn of the rest
  past a memory budget. A streaming message reparses only from its last top-level block, and
  only its newest string stays cached.
  New `rune::commit_generation()` counts commits on the thread, for caches like these.
- **`El::truncate_middle()` and `El::truncate_head()`; `Type::caption()`.** Middle truncation
  keeps a single line's start and end with "…" between, as `NSLineBreakByTruncatingMiddle`
  does for paths; head truncation keeps the end. `.truncate()` now cuts between graphemes
  instead of chars, so a combining accent or emoji sequence is never split (text without them
  draws as before), and a truncated selectable text copies its whole string even across the
  ellipsis. A link or span the cut hides is dropped rather than moved onto "…" (which takes
  the style of the text it replaces, as AppKit draws it). `Type::caption()` is `small` on a 16-point line (11 / 16 in the built-in themes),
  for secondary text under a control; it's a method, so app themes that build `Type` as a
  literal get it without a change.
- **`Switch` draws as macOS 27's `NSSwitch`: a capsule track and a pill knob.** The compact
  (default) switch was a 24 × 16 track with a rounded-square 12-point knob; it is now an
  `NSSwitch` mini, 36 × 16 with a 21 × 14 knob inset 1 point, so it is 12 points wider.
  `Switch::control_size(ControlSize)` picks the other measured sizes: Small 44 × 20 (knob
  26 × 16), Regular 54 × 24 (32 × 20), Large 64 × 28 (38 × 24) and ExtraLarge 80 × 36 (48 × 30).
  `State::Hover` and `State::Pressed` stretch the knob towards the middle instead of rounding
  its corners. The standard appearance keeps its 28 × 16 size with a capsule track and round
  thumb.
- **Breaking (visual): a `Button` with no size set is 24 points tall, as a regular macOS
  push button, not 28.** New `ControlSize { Mini, Small, Regular, Large, ExtraLarge }` (AppKit's
  names, `#[non_exhaustive]`) with `Button::control_size` and `IconButton::control_size` sizes
  controls as macOS 27 measures them: push buttons 16, 20, 24, 28 and 36 points tall with 8, 10,
  12, 14 and 18 points of padding a side, Mini in 9-point type and Small in `ty.small_medium`;
  icon buttons are squares of the same heights. `ButtonSize` and `.size()` are deprecated and
  keep their old metrics, so existing calls draw as before: for a `Button`, `Small` (24) is
  `Regular`, `Normal` (28) is `Large` and `Large` (32) sits between `Large` and `ExtraLarge`;
  for an `IconButton` the names match (20, 24 and 28 are `Small`, `Regular` and `Large`). Only a
  `Button` that never called `.size()` moves: set `.control_size(ControlSize::Large)` to keep
  28. Toast actions now use `Regular` (same height, 12-point padding and the `md` radius).
- **`--keys` caps title-case key names.** A key name without a glyph drew upper case
  ("CAPSLOCK", or "SPACE" from a script's text key under a modifier). It now draws title case
  ("Capslock", "Space"), as the in-app keycaps do; single letters stay capitals and `F12` is
  unchanged.
- **Hiding a window or overlay is a focus loss for its keys.** `on_physical_input` handlers
  used to hear `PhysicalInput::FocusLost` only when the platform reported a blur, which a hide
  on Wayland, with `release_gpu`, or of an overlay never did, and never in `--desktop` or
  headless scripts. They now hear it on every hide and every `Focus(false)`, once until the
  window has focus again, and a hidden window or overlay runs it without waiting to show.
- **Modifier keys on their own reach `on_key`, when an app asks.** `App::modifier_keys()` (also
  `WindowSpec::modifier_keys()` and `ConfiguredOverlay::modifier_keys()`) sends Shift, Control,
  Option/Alt, Command/Super and Fn going down and up as `Key::Modifier { modifier, pressed }`,
  with the side in `Modifier` (`ShiftLeft`, `AltRight`, …, `Modifier::side()`). `mods` hold
  the state after the change. A focused `on_physical_input` handler, such as a shortcut
  recorder, hears the key first, and one it takes is never sent. A blur or hide releases every
  key held. Nothing changes for apps that don't opt in, but `Key` has a new variant, so an
  exhaustive `match` on `Key` with no `_` arm stops compiling: add `Key::Modifier { .. } => …`
  or a `_` arm.
- **`App::try_run` returns startup errors.** `App::try_run`, `try_run_windows_with` and
  `try_run_windows_with_args` return `Err(String)` when the event loop can't start (no Wayland
  or X11 display, as over ssh or in CI) or fails, where `run` panics. On a Mac no one is logged
  in to, an app used to start AppKit and wait forever without a window; it now fails at once
  with "no window server session", and `run` panics with that message. `run` and its siblings
  otherwise behave as before.
- **`Pixels::set` wakes the windows showing it.** A `Pixels` is now a live source like `Live`:
  `set` (from any thread) wakes exactly the windows and overlays whose scene shows it with
  `El::pixels`, and no others. Sets between frames coalesce into one wake and one upload; a
  hidden or covered window sleeps and draws the latest image when it shows again; an element
  keeps waking while its exit (or its parent's) draws it, and stops once it's removed or its
  window closes. `LiveText` on an exiting element now keeps updating the same way, where it
  used to freeze as the exit began. Apps that woke a window after `set` themselves can drop
  that. Finished `ImageCache` decodes also wake the windows showing them.
- **`El::on_screen_layout` follows a window's own moves in the same frame.** An animated
  resize or centre, `place()`, a position change and a resize landing used to report the new
  layout against the previous origin, then correct it a frame later when the move arrived.
  The window now reads its origin as soon as it sets its frame.
- **Prewarmed overlays report where they are.** A prewarmed overlay's `El::on_screen_layout`
  used to report rects from (0, 0) when it warmed, and its warm frame drew at the scale winit
  gave the window rather than the panel's display. A rect that moved while it was hidden
  also wasn't delivered when it was shown. It now warms at its placed origin and display
  scale, and the reveal delivers rects that moved while hidden.
- **A fading-out overlay clicks through on macOS.** `OverlayHandle::fade_out(d)` stopped Rune's
  input at once, but a panel that wasn't prewarmed still took clicks and scrolls for the whole
  fade, so they never reached the window below. It now ignores the mouse from the start of the
  fade.
- **`wake_after(Duration::ZERO)` never spins the loop.** A wake due sooner than a frame asks
  for the next frame. A window or overlay a display link paces (macOS) draws it on the next
  tick: windows now keep their link running for it too, as overlays do. Hidden, covered, faded
  out or unpaced (no display link, or not macOS), it waits a frame interval (60 Hz) from the
  last poll. It used to wake the loop at once, so a poll that kept asking ran as fast as the
  loop could turn while covered, after a fade-out or without a display link.
- **Overlays on macOS draw at the scale of the display they're placed on.** An overlay used to
  take its scale from winit's window when shown, which for a hidden panel needn't match the
  display it's about to appear on, so a panel placed on a display of another scale could draw
  its first frames at the wrong scale. It now reads the panel's `backingScaleFactor` once it's
  on screen, and before that the scale of the display holding most of its frame. `OverlayPlacement::Rect` on
  `DisplayTarget::Primary` documents that it's a global rect that may sit on any display.
- **`El::on_screen_layout`** reports an element's rect in global screen points (from the
  primary display's top-left, y down), the space of an overlay's `OverlayPlacement::Rect` on
  `DisplayTarget::Primary`, so a window can open an overlay exactly over one of its elements.
  It runs when the element lays out somewhere new and when its window moves. Headless runs
  report against the script's virtual desktop: a window opens centred on the primary display,
  or at its `position`, and `place()` moves it.
- **Overlays draw through a fade-out.** `OverlayHandle::fade_out(d)` stops input at once but
  keeps drawing until the fade ends (it used to stop drawing at once), so an exit transition
  started in the same turn plays out as the window fades, and the panel isn't treated as
  occluded while its alpha falls. Showing again mid-fade carries that motion on instead of
  settling it. A shown overlay whose poll calls `tx.wake_after(Duration::ZERO)` now keeps its
  display link running and polls every frame, instead of stopping after each frame and
  restarting on a timer, so a poll stepping its own animation needs no looping element.
- **`El::drag_threshold(points)`** sets how far the pointer travels before that element's
  `drag_out` starts, in place of `DRAG_THRESHOLD` (4 pt, 8 on Linux), which stays the default.
- **Hidden overlays follow theme changes.** A hidden or prewarmed overlay switching between
  light and dark used to keep the old theme until it was shown and the theme changed again; it
  now takes the change while hidden and redraws in it before it's revealed.
- **Overlay window levels and fades on macOS.** `ConfiguredOverlay::macos_level(level)` and
  `OverlayHandle::set_macos_level(level)` set the AppKit window level (e.g. 3 for
  `.floating`), changeable while shown. `OverlayHandle::fade_in(d)` and `fade_out(d)` fade
  the window's alpha with AppKit's ease; elsewhere they show and hide at once. Overlay panels
  now go exactly where they're placed at any level, including over the menu bar, where AppKit
  used to push windows below it at levels under the menu bar's. Overlays read the system
  light or dark theme when their window is created rather than on the first theme change.
- **`Shadow::core_animation(x, y, radius, color)`** builds the shadow a CALayer draws for a
  `shadowRadius`: Core Animation's blur has a standard deviation of about the radius, so
  `blur` is twice it.
- **Continuous corners.** `El::continuous_corners()` rounds an element's fill, border, image,
  outline, `clip_rounded` and system material with Apple's continuous curve (CALayer
  `.continuous`, SwiftUI `.continuous`) instead of circular arcs; shadows stay circular.
  `LayerSpec::clip_radius` takes a negative radius for the same shape. Adds the public field
  `Visual::continuous_corners`, so a `Visual { .. }` literal without `..Default::default()`
  stops building. `system_material(SystemMaterial::Glass)` turns the flag on, since
  `NSGlassEffectView` draws no other curve, so the element's fill, clip and shadow hole match
  the glass; `Popover` follows the flag.
- **System backdrop materials.** `El::system_material(SystemMaterial::Glass | Popover)` backs
  an element with Liquid Glass (`NSGlassEffectView`, macOS 26+; the popover material before)
  or the popover material, as a native view under a transparent window's Metal layer that
  moves in the same Core Animation transaction as each frame. Opaque windows, other platforms
  and headless renders draw the fill or a neutral tint, so goldens don't change. An outer
  shadow on a material element no longer paints under its interior when the native view is
  on. Each native view stays with its element while it's mounted; an element dimmed to zero
  takes none. Once the last material element unmounts, the window stops presenting inside
  transactions. `rune::{SystemMaterial, MAX_NATIVE_MATERIALS}` are new root exports.
  `RUNE_SYSTEM_MATERIAL=legacy|unsynced|off` selects other paths. New example:
  `examples/glass-card`. See the crate README's "System materials".
- **Occluded overlays and windows idle instead of spinning; their timers still fire.** A shown
  overlay or window whose displays sleep or that is covered no longer turns the event loop
  (about a full core before) when its drawable is occluded or times out. It runs updates, `poll`
  and due `wake_after` timers without drawing, so a timer can still hide it; re-checks the surface
  after 250 ms, backing off to 2 s; and redraws once when uncovered. A covered overlay commits
  nothing until then, so motion started while it was covered begins on that frame. A
  `wake_after` from `AppHandle::update` or `OverlayHandle::update` now arms while the window or
  overlay is hidden, covered or minimised, rather than on its next drawn frame. Without a display
  link (made while the displays slept) macOS motion is paced by a ~60 Hz timer rather than a busy
  loop, and an overlay's 120 Hz pointer poll for `OverlayInput::Regions` rests while it is occluded.
- **Help tooltips.** `El::help("…")` shows a tooltip after the system hover delay, as SwiftUI's
  `.help`. A press, scroll, key or the pointer leaving hides it until the pointer comes back. It
  takes no hover or clicks. macOS windows use AppKit's own tooltip, registered per element on
  device pixels and replaced only when it moves, and left out behind a modal or under a popover;
  overlays and other platforms, and headless, have Rune draw one (0.5 s, the double-click time
  on Windows; `Runtime::help_delay`). Headless
  `--snap` shows the drawn tooltip, and `--tree` gains a top-level `tooltip` plus `help` on each
  element. `ElementInfo` gains `help` and `selected`. Elements without `help` are unchanged.
- **Selectable text.** `El::selectable(true)` lets people select and copy read-only text, as
  SwiftUI's `.textSelection(.enabled)`: drag, double-click a word, triple-click a paragraph,
  shift-click to extend, Cmd/Ctrl+C, Cmd/Ctrl+A, and a Copy context menu on macOS. After a
  click on the text, plain Cmd/Ctrl+A selects all of it and plain Cmd/Ctrl+C copies a non-empty
  selection, before the view's key handlers; with Shift, Option or Control added, or nothing
  selected, they fall through as before. Inside a row that handles input the row keeps its
  clicks, hover, press paint and context menu; a drag that selects text doesn't press, and in a
  draggable row dragging drags the row. A view that changes, removes or unselects the text drops
  its selection. A link inside selectable text still opens on click. Scripts gain a
  `triple-click` event. Text without `selectable` is unchanged.
- **`El::selection_color`** sets the selection highlight of a field or selectable text; both
  draw it the same way, and fields draw exactly as before. Kit's `k.selection()` is the colour
  its fields use (unchanged), so selectable text can match them.
- **Radial and angled linear gradients, opt-in sRGB blending, and shadow blur docs.**
  `El::radial_gradient(center, radius, stops)` draws circular (`f32`) or elliptical (`[rx, ry]`)
  radial gradients with up to 8 `(f32, Color)` stops in `0.0..=1.0` in the instanced uber-quad
  pass (`KIND_RADIAL`), with `center` in normalized element coordinates (`[0.5, 0.5]` = centre)
  and `radius` in points; `El::linear_gradient(angle, stops)` draws linear gradients at `angle`
  radians (CSS convention) with up to 8 stops. Both compose with `.fill`, `.border`, `.radius`,
  `.opacity` and `.blur`, and spring stop colours in Oklab. `App::srgb_blend(true)`,
  `WindowSpec::srgb_blend(true)` and `ConfiguredOverlay::srgb_blend(true)` (plus
  `Renderer::set_srgb_blend`, `SurfaceRenderer::set_srgb_blend`, `Runtime::srgb_blend` and
  `Color::premul_srgb`) opt into gamma-space (sRGB) compositing for translucent fills, gradients,
  shadows, blurs, shapes, text, premultiplied and straight-alpha images, native/video surfaces,
  headless `--keys` overlays and `--snap-on-patch` so faint glows over dark backgrounds match CSS
  and Core Animation (`CALayer.shadowRadius ≈ σ`, CSS `blur = 2σ`); `srgb_blend` is ignored with a
  warning when a `Compositor` is set, and default linear-light rendering is unchanged.
  `rune::RadialRadius` is a new root export.
- **macOS drag-out matches an AppKit drag source more closely.** New `El::on_drag_start(|s, tx|)`
  runs once the system takes the drag, before its `on_drag_end`, so the app can style the source
  while it's dragged (the example dims its tile to 0.28). Every start now gets exactly one end, in
  order: a drag-out no system took (headless, or a cancelled press) ends `Cancelled` by the next
  frame. New `DragOperation` (`Copy`, `Move`, `Link`) with `DragItem::operation(op)` to offer just
  that, `allow(op)` to add one and `offers(op)`; `allow_move()` is `allow(DragOperation::Move)`,
  and a drag still offers only Copy by default. Modifier keys pick among the offered operations
  only when there are several. `DragItem::opaque(color)` composites the drag image over `color`,
  and `DragImage::over_element()` starts a supplied image over the element (at its `size`, else
  the element's full unclipped size) instead of centred under the pointer. Behaviour changes: the
  default snapshot is placed at its pixel-aligned crop, so it is no longer resampled to the
  element's fractional rect, and a drag let go over the app's own windows ends `Cancelled` and
  slides back (winit's file-drop handler used to accept it as a Copy and queue a `DroppedFile` for
  the app's own file).
- **Apps can resize and centre their window.** `Tx::resize_window(size)` animates the
  window's content size on the change's spring with the top edge fixed and the window kept on
  its screen; `Tx::resize_window_instant` jumps, and `Tx::center_window` centres as AppKit's
  `center` does. `WindowHandle::{resize, resize_instant, center}` do the same from outside a
  handler. On macOS the content follows the frame every display frame; Windows and Linux
  resize at once; Wayland doesn't keep the window on screen. Headless runs move the viewport
  on the virtual clock. Requested sizes stay inside the window's minimum and maximum.
  `App::fit_content_height()` opts a window into following its content's natural height, with
  no default minimum height. See the crate README's "Resizing and centring a window".
- **Headless recordings of the whole desktop.** `--desktop BACKDROP.png` renders every window a
  multi-window app makes, each at its placed frame with rounded corners and a soft shadow (larger
  for the key window), over the backdrop scaled to fill the output. Showing, hiding, placing and
  focusing follow the script and the app over time, and script pointer coordinates are output
  points. It works with `--video`, `--out`, `--snap`, `--strip`, `--keys` and `--pointer`;
  `--tree` describes the `--window` window and lists every window's frame. `headless_run()` adds
  `desktop()`, `window_frames()` and `windows_at(t)` (`rune::tooling::WindowFrame`). Runs without
  `--desktop` are unchanged. `examples/launcher` records a notes app and its ⌘Space panel.
- **A scroll cancels a held press.** Scrolling a list by wheel or fling while a press inside it
  is held now cancels the press, as a `UIScrollView` cancels its content's touches: the element
  lets go and the release doesn't press. A drag keeps its capture, and programmatic scrolls
  (a chat pinned to its bottom) don't cancel.
- **Nested scales compose.** A scaled element inside a scaled element used to scale about its
  own unscaled centre, dropping its parent's anchor, so it jumped when its own scale (a press, a
  live scale) turned on, and a product of 1 ignored the parent's offset. It now scales about its
  centre where its parent draws it, and hit testing follows. This moves the output of apps
  that nest scales; a single scale draws exactly as before.
- **Text scales with its box, and `El::scale`.** Text inside a scaled element used to keep its
  full size while the box shrank around it. Under `live_scale`, `press_scale`, scale transitions
  and scale beats in a sequence loop, glyphs, span backgrounds, selection and caret now scale
  with the box. Text under a still scale rasterizes at the scaled size; during a spring it
  samples its rest-size glyphs. This moves the output of any app that scales text. Text that
  isn't under a scale is unchanged. `El::scale(k)` is a static scale on the same mechanism:
  about the centre, no layout change, with hit testing on the drawn rect. A `scale` an app
  changes (a zoom slider) samples rest-size glyphs until it holds still for 0.1 s, then
  rasterizes at the scale rounded to 1/32, so the glyph atlas stays bounded.
- **Pictures size from their image.** An element with `pixels`/`pixels_fit` that sets only
  its width or only its height takes the other side from the image's aspect ratio (it used to
  take its content's size, usually 0). Pictures with both sides set, or neither, are
  unchanged. `El::aspect_ratio(r)` sets a ratio on any element. A picture whose image lands
  or changes shape takes its new size at once, even on a frame that re-runs the view.
- **System monospaced font.** `Family::SystemMono` shapes with SF Mono on macOS, Cascadia
  Mono or Consolas on Windows, and fontconfig's `monospace` on Linux, loaded from the system
  on first use and falling back to the mono slot. `Family` gains a variant, so an app's
  exhaustive `match` on it needs a new arm.
- **Double press.** `El::on_double_press` runs on the second of two presses inside the system
  double-click interval (now read on Windows too) and 4 pt of each other; `on_press` still runs
  for both. Only clicks pair: a press that travels more than 4 pt, or is held past the interval,
  is neither half of one. Scripts gain a `double-click` event. Elements without it are unchanged.
- **Press scale.** `El::press_scale(0.97)` shrinks an element about its centre while a press
  on it is held and springs it back on release. Layout and hit testing keep the resting size,
  and a held press keeps its hover. Reduce Motion turns it off. Nothing changes for elements
  without it; rune-kit buttons don't opt in.
- **An explicit `optical_size(o)` now also picks the `trak` tracking:** text is tracked as Core
  Text tracks `o`-point text scaled to the set size, so
  `TextStyle::new(size * k, w).optical_size(size)` matches a SwiftUI view at `size` with
  `scaleEffect(k)`. This is what Core Text does with its optical size attribute
  (`kCTFontOpticalSizeAttribute`): a 28 pt font at optical size 14 lays out exactly as 14 pt
  text scaled 2x. Setting only the `opsz` axis (`variations("'opsz' 14")`, or
  `kCTFontVariationAttribute`) still tracks at the point size. Auto optical size is unchanged.
- **rune-kit `Markdown` colours fenced code** in ts, tsx, js, json, rust, swift, css, html,
  sh/bash, sql and yaml; on by default, so code blocks in those languages render with coloured
  tokens (goldens with such blocks move). Unknown languages and indented blocks stay plain, and
  layout is unchanged (colour-only spans). New: `rune_kit::markdown::Syntax`,
  `MarkdownStyle::syntax(kind, color)` and `syntax_highlight(bool)`. `Block` is unchanged.
- **`MarkdownStyle::diff_text(added, removed)` and `diff_text_tint(amount)`** colour the text
  of a diff's added and removed lines (`diff_text_tint` blends the text colour toward the
  theme's green and red). Off by default: line text keeps the body colour.
- **rune-kit `Markdown` table cells use tabular figures** (`'tnum' 1`), so numbers line up down
  a column; only digits change width. New `MarkdownStyle` builders: `table_outline(Option<Color>)`
  (`None` draws no outline and adds a rule under the last row), `table_rules`,
  `table_header_text` and `table_tabular_figures(bool)`. `table(border, header_fill)` still sets
  the outline and the rules. The default look is otherwise unchanged.
- **rune-kit `Markdown` tables no longer wrap a cell when the table has room.** Columns started
  at zero width and grew by their content width, but each column's padding was reserved
  first, so wide columns came up short and their longest cell wrapped. Columns now start at
  their content's width and shrink in proportion to their text when the table doesn't fit.
  Most tables' column widths move by a few points.
- **Video and audio playback.** `Player` (alias `Video`) plays a file or URL:
  `el().video(&clip, SurfaceFit::Fit)` draws a video (muted by default), and `Player::audio`
  plays audio with no element (unmuted by default). macOS uses AVFoundation; headless runs
  are deterministic and silent; other platforms report `Failed`. Adds the
  `objc2-av-foundation`, `objc2-core-media` and `objc2-core-graphics` macOS dependencies.
  `rune::{Player, Video, MediaSource, PlayerState}` are new root exports, so a glob import
  of `rune::*` can clash with an app type of the same name. See the crate README's "Playing
  video and audio".
- **Headless recordings honour single-window transparency and physical size**, including
  alpha in PNG and ProRes output. Default window pixels are unchanged. `--duration auto --out`
  selects headless PNG recording; `--out` alone uses automatic duration unless `--native` is set.
  Exact mark names such as `step-1` take precedence over time offsets, malformed offsets report
  the available marks, and video filenames starting with `-` work.
  `headless_run().events()` includes pop-up menu picks and dismissals.
- **Global script shortcuts only run their registered handler**, even when a window is key;
  they never reach `on_key` or physical-input handlers. Unregistered chords fail the script.
- **Recording keycaps follow light and dark backgrounds.** `--keys-theme auto|light|dark`
  selects the colours of their fixed screencast style; `auto` follows the app's root fill.
  Overlays blend correctly over transparent frames, and a failed video encoder stops the run
  and reports its exit status.
- **Headless frames can show the script's keys.** `--keys [CORNER]` draws screencast keycaps
  (typing grouped into bursts, ⌃⌥⇧⌘ chords with repeat counts, glyphs for named keys) over
  `--video`, `--out`, `--snap` and strip frames, and `--pointer` a ripple at each press. A
  separate runtime draws them after the app, so `--tree` and `--trace` are unchanged.
- **Scripts can press global chords.** `{"type": "shortcut", "chord": "cmd+space"}`
  runs the app's registered shortcut handler. `{"shortcut": …}` is unchanged.
- **Scripts can mark times.** `{"t": 3.2, "type": "mark", "name": "end"}` has no input effect;
  apps read marks through `headless_run().marks()` and `mark(name)`. `--at` and `--start`
  take `mark:NAME` with an optional offset (`mark:end+0.5`), and `--duration auto` (the
  default for `--video`) runs to the `end` mark, or one second after the last event.
- **Apps can read the headless run.** `rune::tooling::headless_run()` returns the run's script
  path, its expanded events with their times, `--scale`, `--fps`, the viewport, `--start`, the
  frames it writes and its last frame's time, from `init` on. It is None in a window.
- **Headless runs can write a finished video.** `--video out.mp4` (`.mov` for ProRes 4444,
  `.webm` for VP9) streams raw frames into an `ffmpeg` child at `--fps` (default 60), so no
  frames touch disk; ffmpeg must be on `PATH`. Its length is `--frames N`, `--duration S`, or
  through one second after the script's last event. Readback now keeps three frames in flight
  and PNGs (`--out`, `--snap`) encode on worker threads.
- **rune-kit feature `mermaid` draws ```` ```mermaid ```` flowcharts and sequence diagrams
  natively** inside `Markdown` once the fence closes, and through `rune_kit::mermaid::render`.
  Without the feature, or for unsupported diagrams, the fence stays a code block (with a
  "Diagram not supported" hint when the feature is on). New `MarkdownStyle` builders:
  `diagram`, `diagram_surface`, `diagram_note`, `diagram_font`, `diagram_max_width`.
- **rune-kit `Markdown` renders GFM tables and tints ```` ```diff ```` fences.** Pipe tables
  were flattened into run-on text before; they are now tables with a header row, alignment
  and columns sized from their content (narrow tables wrap, then clip). `diff` and `patch`
  fences tint `+`/`-` lines and dim `@@` headers instead of drawing a plain code block.
  Documents with neither render exactly as before. New `MarkdownStyle` builders: `table`,
  `table_min_column`, `diff`, `diff_hunk`.
- **rune-kit `Markdown` works without a `Kit`:** `Markdown::render(&MarkdownStyle)` with
  `MarkdownStyle::neutral(dark)` or `MarkdownStyle::from_seed(seed)`. `build(&kit)` is unchanged.
- **rune-kit `Tooltip` works without a `Kit`:** `Tooltip::label(..).render(&TooltipStyle,
  &mut layer)` with `TooltipStyle::{neutral, from_kit}` and its builders. `build(&kit)` is
  unchanged.
- **`rune::text::measure(text, spans, style, max_width)`** returns a text's laid-out size outside
  a frame, with the fonts the app loaded; `Some(0.0)` gives its min-content width.
- **Text in fonts with a `trak` table (the macOS system font) now has the same width at every
  display scale, matching AppKit.** The font's size-specific tracking used to be looked up at
  the size in device pixels, so at 2x 14 pt text was tracked as 28 pt text. It is now applied
  at the point size, as Core Text does, within 0.003 pt per glyph of Core Text at 8–34 pt. At
  2x, system-font text gets narrower per glyph by 0.18 pt at 13 pt, 0.34 pt at 14 pt, 0.52 pt
  at 16 pt and 0.63 pt at 17–20 pt (a 34-character 14 pt label is 11.6 pt narrower), and wider
  by 0.19 pt at 11 pt and 0.36 pt at 8 pt; 12 pt and 24 pt barely move. Text at 1x moves by
  at most 1/2048 em per glyph. Wrapped text can break at different words. Fonts without
  `trak`, like the bundled Inter and IBM Plex Mono, are unchanged. `TextStyle::tracking` adds
  to the font's tracking; `TextStyle::features("'trak' 0")` turns the font's tracking off.
- **Shortcut labels follow layouts that change under ⌘ (macOS).** `Shortcut::keycaps` (the
  recorder, its error text and native menu key equivalents) now reads the key's character
  under the chord's ⌘ and ⌃, so on "Dvorak – QWERTY ⌘" ⌘ plus the US-K key shows ⌘K, not ⌘T,
  and menu items fire on the key they show. ⇧ and ⌥ still show the base key (⌥⌘K, not ⌥⌘˚),
  as AppKit does. The key keycap is now a capital ("K", was "k"), F-keys read "F5", and on
  macOS named keys use menu glyphs: ↩ ⇥ ⎋ ⌫ ⌦ ↖ ↘ ⇞ ⇟ (Space stays "Space"). `Keybind`
  upper-cases only single characters, so "Space" no longer draws "SPACE". The layout lookup
  also declares `UCKeyTranslate`'s string lengths at their real 8-byte width; it wrote past a
  4-byte stack slot on every call.
- **Native pop-up and context menus.** `tx.popup_menu(PopupMenu, anchor, on_pick)` opens an
  `NSMenu` over an element key (a picker's selected item sits over the element, as SwiftUI's
  menu-style `Picker` does) or at a `PointerPress`. `on_pick` gets the picked id after the menu
  closes, or `None` on dismissal or when its window hides, closes or quits. Frames and updates
  keep running while it's open. Other
  platforms answer `None` on the next frame (`popup_menu_supported()` is false). New:
  `PopupMenu`, `PopupItem`, `PopupAnchor`, `PopupMenuError`. Headless trees gain a `popupMenu`
  object while one is open, and scripts gain `menu-pick` and `menu-cancel` events. Nothing
  changes for apps that don't call it.
- **`El::matched(id)` adds shared-element motion** (SwiftUI `matchedGeometryEffect`, Motion
  `layoutId`). When an element with an id leaves and another with that id mounts elsewhere in
  the same commit, the new one springs from the old one's presented rect, radius, fill and
  opacity to its own layout (`REFLOW` by default), painting above the window with its
  children scaled to cover the moving frame. `rune_motion::Scene::match_geometry_offset`
  starts a match from a rect offset outside the scene (a scroll). Apps that don't call
  `matched` draw the same pixels. In `rune_motion::Scene`, screen travel
  (`LayoutAnim::screen()`) now lands only once its ancestors' presence, scale, opacity and
  blur settle too, so it can't jump onto a parent mid-transition.
  `matched` draw the same pixels.
- **Elements can be dragged out to other apps (macOS).** `El::drag_out(|s| Some(DragItem))`
  starts a system drag once the pointer passes `DRAG_THRESHOLD`, carrying file URLs, PNG data
  or text to Finder, System Settings and other apps; `El::on_drag_end` reports a `DragOutcome`.
  A drag-out element never runs `on_drag`, and once the system drag starts its press is
  released without firing `on_press`. Elements without `drag_out` are unchanged, and other
  platforms ignore it. Headless traces log `drag-out <path>` and `drag-end <outcome> <path>`,
  and `--tree` marks the node `dragOut`. `ElementInfo` has a new pub `drag_out` field, so code
  that builds it as a literal or destructures it without `..` needs updating.
  `rune::{DragItem, DragImage, DragOutcome}` are new root exports, so a glob import of
  `rune::*` can clash with an app type of the same name.
- **On macOS, the first `open()` of a window that doesn't exist yet now activates the app**,
  like every later `open()`. Before, the new window became key while another app stayed
  active, so typing and ⌘W went to that app. The window opened at launch and nonactivating
  panels are unchanged.
- **`App::window` windows still open when the main window closes now run their `on_close`,
  and `WindowSpec::from_app` starts from the app's `window_options`** (before, it used the
  defaults, so headless runs dropped `transparent` and `physical_size`).
- **One app can host windows with different state types.** `App::window(WindowSpec<T>)` adds
  an ordinary window with its own state and returns its `AppHandle<T>` and `WindowHandle`.
  It honours `visible: false`, closes by hiding (its `on_close` runs and reopening keeps the
  state), and never keeps the app running alone. `run_windows*` factories may return
  `AnyWindowSpec`s (`WindowSpec::boxed()` or `.into()`) to mix state types. Factories
  returning `Vec<WindowSpec<S>>` still compile, but a turbofish like `run_windows::<S>(..)`
  no longer does: drop it. Headless `--window KEY` and `--list-windows` include `App::window`
  windows and render them with their own state.
- **Hit testing leaves out an element's right and bottom edges everywhere.** Press, hover,
  secondary press, links, wheel targets and hover and press highlights now use the same
  half-open test as pointer-move, `on_hover_change` and overlays (like `NSPointInRect` and
  `PtInRect`). A pointer exactly on a shared edge belongs to the right or lower sibling
  for every handler. A pointer exactly on a lone element's right or bottom edge no longer
  hovers or presses it.
- **rune-kit `List` rows with an action are keyboard-reachable.** When any row has
  `on_select`, the rows sit in a `listbox` focus group: Tab enters the list once (on the
  selected row, if any), arrows move between rows, and Enter selects. Those rows show
  their highlight on keyboard or pointer focus (`focus_highlight`) instead of on hover. Lists
  without actions are unchanged.
- **A disabled, invalid rune-kit `TextInput` no longer has a red caret.** The caret now follows
  the text colour, as `TextArea`'s does. Only that one state changes.
- **rune-light's `Scene` and `Gi` have an `emission` field.** `Scene::emission` (an optional
  output-resolution texture) lets effect layers light the UI through the GI, scaled by
  `Gi::emission`. Struct literals need `emission: None` and `..Default::default()`. GI media now use
  sub-texel coverage, so a moving emitter no longer pops. New: `LightRenderer::inspect`,
  per-cascade `Timings` and `GlassRenderer` (thick refracting glass).
- **rune-fx effects look different, and some structs gained fields.** Fire rises and narrows with
  a white-yellow core and smoke, electricity strikes and re-strikes as fractal lightning, glows fall
  off within about 2.5 radii, sparkles are a few crisp four-point stars, and one-shots (ripples,
  pings, zaps, bursts) overlap instead of restarting. `FxLayer::opacity`, `ElectricParams::zap`,
  `SparkleParams::burst` and `FireParams::smoke` are new pub fields, so struct literals without
  `..Default::default()` need them. New: `OneShots`, `FxRenderer::blur_backdrop` (frosted glass)
  and `FxRenderer::set_fire_sim`/`simulate` (an opt-in GPU fluid for fire and smoke).
- **Rotating icons turn their artwork, not just their outline.** An icon with
  `.looping(LoopSpec::rotation(..))`, such as rune-kit's `Loader` arc, turned its quad but kept
  sampling the atlas unturned, so the arc stayed put while the rotating square cropped it. The
  artwork now turns with the quad and is bilinear-filtered while turned. Icons at rest
  (angle 0) draw exactly as before.
- **Rune builds for Intel Macs.** `x86_64-apple-darwin` stopped compiling where Rune read
  the Objective-C runtime's `BOOL`, which is `i8` there and `bool` on Apple silicon.
- **Overlay and launcher panels keep their view when something restyles winit's hidden
  window.** On macOS an overlay or nonactivating launcher panel shows winit's view while
  winit's own window stays hidden. That hidden window answered `contentView` with the moved
  view, so AppKit calls on it that rebuild the frame (`setStyleMask:`, `setContentView:`) pulled
  the view out of the panel, which then dropped every click. Starting a ScreenCaptureKit stream
  in the same app does this: ReplayKit restyles the first window in `NSApp.windows`. Now only
  winit gets the moved view; AppKit and other frameworks see the hidden window's real, empty
  content. Likewise only winit's frame, level and mouse-ignoring calls on a launcher's hidden
  window move its panel, so restyling that window no longer resizes the panel, and a title set
  there also stays on the hidden window for `Window::title()`. If the view is taken anyway,
  the panel puts it back on the next event-loop turn or event and releases an overlay press
  it was holding, and an overlay press whose release never reached winit stops holding the
  pointer after 2 s. The hidden window also keeps winit's class: Rune adds its forwards to winit's
  window and view classes instead of subclassing each window, which under KVO had made
  `-[window class]` return nil.
- **Borders can be dashed.** `El::border_dash(dash, gap)` dashes an element's `border` or
  `border_gradient`, in points, with butt ends:
  `el().radius(13.0).border(1.0, amber).border_dash(4.0, 3.0)`. Dashes follow the ring's
  centre line round the corners. The pattern starts with a dash where the top edge leaves the
  top-left corner and runs clockwise, and its period stretches or shrinks a little so a whole
  number of dashes meets itself at that seam. `El::border_dash_phase(points)` shifts it, like
  SwiftUI's `dashPhase`; a phase of `dash + gap` is one period. Border colours spring as before;
  the phase is applied as given. The fill shows through the gaps, so a dashed element draws its
  fill and its ring as two instances, the ring as the new `render::KIND_DASHED` (10), which
  `rune-ref` renders too. Elements without dashes paint the same instances as before.
  `Visual` has new `border_dash` and `border_dash_phase` fields.
- **Text can draw in capitals.** `El::text_case(TextCase::Uppercase)` (or `Lowercase`) draws
  a text element's letters cased without changing its string, like SwiftUI's `textCase`:
  `text("Text channels").text_case(TextCase::Uppercase)` draws "TEXT CHANNELS". Casing is
  `str::to_uppercase`'s, so "ß" draws "SS". The dev tree, `ElementInfo::text`, link labels and
  live text keep the authored string, and `Span` and `Link` ranges stay byte ranges of it even
  when casing changes the length. Text fields ignore it and draw what is typed, so their caret
  and selection are unchanged. `TextStyle` is unchanged.
- **Exiting elements are marked, and gone from lookups.** `ElementInfo` has a new `exiting`
  field, true for an element playing its exit transition and for everything inside it. `--tree`
  (and the hot host's tree) still lists them, now with `"exiting": true`; elements at rest add no
  key, so existing tree goldens don't change. The dev panel's `tree` reply leaves them out, so a
  check that something left passes as soon as it starts leaving; send
  `{"type":"tree","exiting":true}` to keep them, marked. `Runtime::find`, which `--script` key
  positions and the hot host's `tap` use, already skipped an exiting element and now also skips
  its children; a script that tapped a child of a leaving element by key now stops with "no
  element with key". `Runtime::presented` still reads exiting elements.
- **Dev-panel transitions can capture data.** `dev::Config::transition` now takes any
  `impl Fn(&mut S, &mut Tx) + 'static`, so a transition can close over a scene's fixture:
  `.transition("Open", "list", move |s, tx| s.open(&room, tx))`. `dev::Transition::with` builds
  one for `Config::transitions`. `Transition::new` still takes a plain `fn`, so
  `let t = Transition::<Ui>::new;` lists keep building, but it is no longer `const`, `Transition`
  is `Clone` rather than `Copy`, and its `action` field is private.
- **Held keys activate once, as in AppKit.** Auto-repeats no longer press a focused button,
  menu item, row or link (Return, Space), open a popup button (arrows), dismiss another modal
  (Escape), blur a field (Escape) or submit it (Return). Rune consumes those repeats. After a held
  Return, Escape or Space moves focus, its later repeats reach nothing: not the app's `on_key`,
  the control that took focus or its physical input handler. Other held keys keep repeating. A
  rune-kit `Combobox` chooses once on a held Return and closes once on a held Escape. On X11, a
  held global shortcut fires once, as on macOS and Windows. Typing, deletion, caret movement,
  arrows in lists and menus, and Tab still repeat. To act on every repeat, handle the key in `on_key` and
  check `tx.key_repeat()`.
- **`--script` events at t = 0 see the first layout.** A script whose first events are at
  t = 0 (or omit `t`) now lays out a first frame, with the initial state and queued updates,
  before running them. `{"t": 0, "type": "click", "key": "..."}` used to fail with "no element
  with key", and t = 0 clicks or moves at x/y hit nothing. Frame 0 then draws the events'
  effects, so changes they start animate from the mounted state rather than mounting settled.
  Scripts without t = 0 events are unchanged.
- **`ConfiguredOverlay::setup(fn(&mut T, Waker))`**, like `AppSpec::setup`, so an overlay can
  build its own `ImageCache` or channel instead of borrowing the main window's waker. It runs
  once on the event thread when the loop starts, before the overlay's first frame and before
  updates queued on its handle. A wake re-runs the overlay's view: a shown overlay draws a
  frame, which polls, and a hidden one runs a frame without drawing. Headless runs don't
  create overlays, so they don't call it.
- **`Cursor::Crosshair` and `Cursor::Hidden`.** A crosshair for picking a point or dragging out
  a rect, and no OS cursor for views that draw their own. Windows set them, and overlays now
  set an element's `Cursor` too (before, they left the OS cursor alone) while the pointer is
  over a hit area of a shown overlay that takes input, including a non-key macOS overlay.
  `Hidden` is a transparent cursor image, so it is per-window like any shape and never confines
  the pointer. It shows the arrow again when hover leaves the element, the pointer leaves the
  window (including a drag released outside it), the element is removed, the window loses focus
  or hides, or the overlay hides. The cursor now comes from what's under the pointer in the
  current frame, so an element playing its exit transition has no cursor and a sibling that
  replaces it under a still pointer sets its own. On macOS a non-key overlay sets its cursor
  only while its app is frontmost; the opt-in `macos-background-cursor` feature lifts that with
  a private WindowServer property (an App Store review risk), and without it none of that code
  is in the binary. `Cursor` is now `#[non_exhaustive]`: an app that matches it needs a
  wildcard arm.
- **Key handlers can tell a held key's repeats from its first press.** `tx.key_repeat()` is true
  in the window's `on_key`, an overlay's `.on_key`, `on_input_key` and `on_physical_input` while
  the OS auto-repeats a held key, and false for the first press. `KeyEvent` is unchanged, so
  existing literals keep building. Headless drivers queue a repeat with `Runtime::repeat_key`,
  and `--script` keys and `physical` chords take `"repeat": true`. Built-in behaviour is
  unchanged: held keys still type, move the caret, Tab through focus and press a focused button.
- **Unselected segments hover.** A `SegmentedControl` with `on_pick` now lights an unselected
  segment under the pointer: the `hover` overlay fills it and its label turns `text_base`, on the
  first frame in, fading out over the runtime's default 0.1 s. The selected segment and controls
  without `on_pick` are unchanged. A forced `State::Hover` on an unselected segment now also
  fills it, so snapshots match.
- **Native traces record when frames reach the screen.** On macOS, `--native --trace` frames
  now add `target_ms` (the display link's target) and `present_ms` (Metal's `presentedTime`
  for the frame's drawable), plus `present_dt_ms`, the gap from the previous presented frame
  when the two were drawn back to back. A skipped drawable writes `present_ms: null`. Each line
  is written once its present time arrives, so frames land in the file two or three frames
  late, still in order. When the trace ends it prints a summary to stderr: present-interval
  p50/p95/max, intervals under 2 ms (bunched) and over 1.5 refresh periods (missed), and the
  target-to-present lag. Every other field is unchanged, and readers that ignore unknown keys
  read the new traces as before. Overlays, other platforms and headless traces add no present
  fields, and nothing changes without `--trace`.
- **A late frame no longer slows motion down.** Windows, overlays and the hot and dev hosts now
  share one frame-step rule (`frame_clock`). Motion already running advances by the real time
  since the last frame, up to 250 ms, instead of 1/30 s at most. Before, a host marked idle
  mid-motion stepped only 1/120 s, so after a hitch springs fell behind and finished late. Motion
  a frame starts or retargets starts at that frame's time and steps at most 1/120 s, so it no
  longer jumps by a late frame's gap, and a retargeted spring turns from where it is at that
  time. Both happen in one runtime pass, so poll, the motion trace and the settle flag see one
  frame. While other motion runs, a 60 Hz frame now starts new motion with a 1/120 s step rather
  than 1/60 s. The hot host's first frame after idle no longer steps new motion by the idle gap.
  Scroll flings now integrate their decay exactly, so a fling travels about 1% less in total and
  a long frame carries the same distance as the short frames it stands for.
- **Focus follows an app selection.** In a `focus_follows` group, when the app selects a
  different item (a ⌘-digit shortcut, say) while focus is in the group, focus moves to the
  `focus_selected` item and keeps its keyboard ring, instead of staying on the old row.
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
