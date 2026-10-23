# Batch C: live labels and optical sizing

`LiveText` changes a label without running its view or reflowing its siblings.
Text styles now expose font features and variations. Optical size defaults to
the run's point size because it matches CoreText more closely in the measured SF
samples. The legacy selection remains available through
`OpticalSize::FontDefault`.

## Live labels keep an authored layout box

```rust,ignore
let clock = LiveText::new("00:00");
text("00:00").live_text(&clock)
    .font(TextStyle::new(14.0, 400.0).features("'tnum' 1"));
```

The authored `00:00` reserves the label's size; `.size(w, h)` can reserve a larger
box. Live updates replace only its shaped layout and clip overflow. Rebuilds
continue measuring the authored placeholder. Wrapping, rich spans, editors,
ellipsis truncation and streaming word reveal are rejected for live labels.

The string is an `Arc<str>` behind a short mutex. A revision counter and the same
subscription/wake machinery as numeric `Live` coalesce publications. Equal values
do not wake; hidden hosts sample the latest value on return. Live layouts are not
retained in the commit cache, so a long-running clock does not retain every tick.
Numeric `Live` keeps its existing lock-free publication path.

## Point-sized optical sizing improves the SF reference match

Measured on mini, macOS 27.0.1 (26A434), with `/System/Library/Fonts/SFNS.ttf`, SHA-256
`44890958f3e0b997028121ed7c7a789f3f5cce227c7a93ce8f86ec0db26da571`.
The existing text-quality harness covers five strings at 11, 12, 13, 15 and 24 pt,
both polarities and 1×/2× density. All rendering was headless.

The CoreText harness's new `--native-opsz` mode keeps CoreText's own optical-size
selection. The previous mode pinned its variation to Rune's selection and could
not answer this question. Both modes still use Rune's glyph IDs and positions,
so SSIM measures outline/raster agreement, not complete native text layout parity.

| Sample | Legacy SSIM | Point-sized SSIM | Absolute ink-weight error, legacy → point |
|---|---:|---:|---:|
| Dark, 1× | 0.948209 | 0.983829 | 6.20% → 4.80% |
| Dark, 2× | 0.924100 | 0.984529 | 9.05% → 6.54% |
| Light, 1× | 0.950077 | 0.986830 | 2.68% → 4.13% |
| Light, 2× | 0.934667 | 0.990541 | 2.76% → 3.37% |

SSIM and normalized ink error improve in all four groups; light-mode ink-weight
error increases slightly. Mean absolute total-advance error against a separately
shaped CoreText line falls from 18.292 to 0.005 px at 1× and from 29.912 to 11.242 px
at 2×. The remaining 2× difference is not claimed fixed by optical sizing.

`OpticalSize::Auto` resolves each run's point size, including rich span sizes.
Explicit `opsz` variations override Auto; `.optical_size(value)` overrides both.
Features and variations use Parley's CSS parser. Layout hashes include every new
setting; atlas keys already include normalized variation coordinates.

The new default was reported before refreshing goldens. Thirty named stills were
regenerated; only the two copies of OpenCode's session-top image changed. The
changed bounds `(844, 532)–(1107, 563)` contain its “What's happening” heading.
The other 28 regenerated files are byte-identical. The manifest also records six
historical images the still scripts do not regenerate. Static Inter glyph quads
and atlas bytes match the legacy policy in a separate 1×/2× regression.

Evidence:

- [Aggregated metrics](../perf/autoresearch/text/batch-c/metrics.json)
- [Golden hashes](../perf/autoresearch/text/batch-c/goldens.json)
- [SF at 2×, Rune and CoreText](../perf/autoresearch/text/batch-c/sf-compare-dark-2x.png)
- [Heading before, after and amplified difference](../perf/autoresearch/text/batch-c/session-top-diff.png)

Reproduce with `RUNE_TEXT_OPSZ=legacy` or `point` when running the `text_quality`
example, followed by `swift scripts/coretext-quality.swift OUT --native-opsz` and
`scripts/text-quality.py OUT`. The phase sweep remains the harness's separate
default-style probe; it was not used to choose the optical-size policy.

## Host preferences and fade truncation

`Cx::system_reduced_motion()` supplies the native accessibility preference to views
and defaults Rune's motion policy. `Cx::reduced_motion` overrides it for the current
commit only; the view must repeat the override on later commits.
Hosts sample at creation and focus changes, rather than monitoring settings while
continuously focused. macOS uses NSWorkspace; Windows uses client-area animation;
Linux uses KDE's animation-duration factor or optional GNOME GSettings. Unknown
platform preferences return false. Headless runtimes use `set_system_reduced_motion`.

Root `on_window_focus` handlers receive ordered, deduplicated native activation
events through the normal transaction path. Tests can call `Runtime::window_focus`.
The same hook is wired for interactive overlays.

`truncate_fade()` keeps the full text and fades the overflowing trailing one-em
edge. End-aligned labels fade on the left. It affects glyphs rather than the
element's background or children, and composes with inherited clips.

## Verification

All jobs ran serially on mini through HiveMind with `HIVEMIND_CAS_SYNC=0` and
source-hash checks before and after. Checks completed:

- Workspace, all-target compile check.
- Six Batch C regressions: live text, font features, optical sizing/cache,
  environment motion, focus delivery and fade truncation.
- Eight existing live-binding regressions.
- Four text-quality/cache tests, two font-fallback tests, two allocation gates,
  two nested-fade GPU tests and two reduced-motion tests.
- Release builds and the headless CoreText/golden comparisons above.

The workspace check also caught one remaining Batch B initializer in the
video-tiles benchmark; it now uses packed integer fade words. Windows/Linux native
preference paths were not executed because the eligible HiveMind workers are Macs.
macOS preference code compiles and links; changing the user's OS settings was not
part of the test. Focus callbacks were tested through runtime event injection.

`Env` keeps its original two public fields, `system_dark` and `scale`, so existing
exhaustive literals still compile. The preference is private Runtime/Cx state.
`TextStyle` literals can inherit new defaults with
`..TextStyle::new(size, weight)`.
