# 24. Reference renderer and differential testing

Reader: the next agent changing Rune's renderer. Run the commands
below before changing a tolerance or a shader formula.

`crates/rune-ref` evaluates Rune's current instance stream on the CPU and checks
the production wgpu renderer against it. The first corpus found a glyph sampling
bug: interpolated screen coordinates selected an adjacent atlas column. Its
single-primitive regression differed by up to 83 channel levels before the fix.
After the fix, measured single-primitive errors are at most 1 level; composited
scenes reach 2. The gate allows 2 per channel and **zero outlier pixels**.

## The oracle evaluates the formulas Rune actually ships

The scalar evaluator implements the five instance kinds present when this work began:

| Kind | Reference behavior |
|---|---|
| 0 | Per-corner rounded-rect SDF, clamped capsule radii, softness, and an inset border (uniform or per-side) over the fill, hiding it by its alpha before opacity (`params[3]`) |
| 1 | Evan Wallace shadow formula, the shader's erf approximation and four y samples |
| 2 | R8 glyph texel loads, scaled bilinear footprints, coverage exponent; `params[0]` turns the quad about its centre and samples bilinearly in the glyph's own frame |
| 3 | Inset shadows with spread, offset and the sharp/blurred branch at sigma .3; square corners take exact pixel areas |
| 4 | Vertical premultiplied-linear gradient with rounded-rect coverage |

Every kind uses the shader's four independent soft clip ramps. Expanded quad
bounds are included, since the GPU cannot shade outside its triangles. Outlines
and capsules already lower to rounded-rect instances. Positions are device pixels.
Translation, positive glyph scale and glyph rotation are the transforms the current
instance ABI supports for these five kinds. The generator keeps glyph rotation at zero:
at arbitrary angles a turned edge can pass within the rasterizer's sub-pixel precision of
a pixel centre. `rotated_glyphs` covers quarter turns, whose edges stay half a pixel away. Arbitrary affine transforms, path rendering, group
opacity and backdrop blur are not covered by this oracle.

During the publication rebase, main added vector segment/triangle kinds 5 and 6
(`d666064`). Those new kinds are rejected by this oracle's validator and are not
generated yet; extending the independent formulas and corpus is follow-up work.

`vello_cpu` was considered, as PLAN §6c requests. Its path-area coverage would
introduce a second antialiasing definition, and its rounded-box blur need not
match Rune's four-sample approximation. A direct evaluator is a closer oracle
for our WGSL. Vello remains a good candidate for the future path renderer.
The CPU implementation is independent code, not generated from the shader.
That catches implementation drift; it cannot prove a shared mathematical model
is visually correct or that the four-sample shadow approximates a true Gaussian.

The CPU lane uses strict scalar Rust f32 operations, fixed loop order, no
`mul_add`, and pinned `libm = 0.2.16` with `force-soft-floats` for
sqrt/exp/pow/round/floor. That feature disables libm's architecture shortcuts even
when another workspace dependency enables its default `arch` feature. Gate scripts
also pass `-C llvm-args=-fp-contract=off`; do not enable fast-math. Each blend
decodes the previous sRGB RGBA8 pixel, applies premultiplied source-over, and
quantizes back to sRGB RGBA8. Alpha stays linear. Clear is quantized too.
Sixteen BLAKE3 hashes in `goldens.json` pin the canonical corpus. Cross-architecture
byte equality still needs the Linux run; it is a design goal, not measured evidence.
The sixteen hashes agree in debug and optimized release builds on arm64; the
release check explicitly disabled FMA contraction. CPU-target Clippy passed with
warnings denied. The CLI regression replay reports node 4, `generated/3`, at
13,34 and a maximum GPU delta of 1 with no outliers.

The CPU-only build has no Rune/wgpu/window/font dependency. `--features gpu`
adds a field-by-field adapter to **Rune's real Renderer**, including its shader,
blend state, atlas upload and sRGB Offscreen readback. `Primitive::from_instance`
accepts real emitted instances with a node/source sidecar. Frozen atlas bytes in
a JSON scene make replay independent of subsequent font/shaper changes.

## One generator drives the checks

`generate::scene` decodes a vector of bounded 32-byte records. Each record controls
kind, geometry, per-corner radii, colours/alpha, border, softness/sigma, clip/fades,
inset offsets and glyph scale. Text cases add real Parley/embedded-Inter runs and
freeze their atlas; those cases check sampling, not shaping correctness. Synthetic
atlas texels have guard space so the tests do not rely on undefined out-of-bounds
GPU texture loads. `generate::seeded` provides a stable SplitMix64 corpus.

The shape follows Vello's `cpu_gpu_differential` target [1]: structured bounded
inputs, two persistent renderer implementations, a per-channel comparison,
and saved replay cases. Rune uses proptest rather than cargo-fuzz. Proptest
shrinks both the vector and its fields, persists failure seeds, and writes the
final minimized GPU failure as `crates/rune-ref/failures/minimized.json`.
`RUNE_REF_FAILURES` overrides that output directory for worker artifact collection.
Move understood cases into `regressions/`; the test replays every JSON there.

CPU metamorphic tests require exact bytes for integer translation, opacity-zero
identity, full-viewport clip identity, and reorder of disjoint clipped groups.
The GPU runs the same four relations. Translation/full clip allow 2 levels;
transparent additions and disjoint order require exact equality. Comparison never
uses an image average that could hide an incorrect edge.

### Measured budget on Apple M4 Pro / Metal

Rust 1.99.0 on an Apple M4 Pro build worker; no windows or screen captures. One successful corpus run contains 128 generated scenes × 8 isolated
primitives, 128 composites, and 16 pinned-font text scenes.

| Primitive | Measured maximum channel delta | Gate |
|---|---:|---:|
| Rounded rect / border / capsule | 1 | 2 |
| Outer shadow | 1 | 2 |
| Glyph, including scaled synthetic atlas | 1 | 2 |
| Inset shadow | 1 | 2 |
| Vertical gradient | 1 | 2 |
| Eight-primitive composite | 2 | 2 |

There are no permitted outliers. One LSB covers rounding around an RGBA8 encoding
boundary; the second covers accumulated blend/encode rounding. This is an
empirical bounded-corpus budget, not a theorem or a cross-vendor certification.
WGSL allows FMA fusion and reassociation [2], and sRGB conversion accuracy adds
another backend-dependent rounding step. Measure Vulkan/D3D12 before claiming
their budget. Never increase the cap to hide a large geometric/texel mismatch.

Also passed: 128 GPU proptest scenes, 128 CPU metamorphic cases, 32 GPU
metamorphic scenes, and the saved glyph regression. The allocation gates passed
for 30 warmed CPU-reference frames and Rune's existing 30-frame streaming runtime
test. These exclude scene construction, shaping/commit and GPU/driver allocations.

## The glyph regression fixes a discontinuity

`regressions/glyph-texel-boundary.json` came from seed 15, primitive 4. Its glyph
starts at x=10.5. At a pixel center x=13.5 the intended local coordinate is 3,
but triangle interpolation can produce a value just below 3. `floor` then loads
column 2. Before the fix, 231 pixels exceeded the 2-level cap, maximum delta 83.

The glyph branch now uses fragment `@builtin(position).xy` for its texel lookup,
which is the rasterizer's pixel center. Both nearest and bilinear glyph sampling
use it. Other primitive formulas and clip interpolation are unchanged. The saved
case and full corpus pass. No existing PNG golden was rewritten. Fractionally
positioned glyphs can intentionally change where the old lookup selected the wrong
texel; existing app-wide snapshot equivalence has not been measured in this pass.

Harness corrections during bring-up: avoid translating the sentinel NO_CLIP
bounds beyond the replay validator's coordinate limit; permit Rune's actual
2048² atlas rather than the README's historical 1024² size. Neither was a
production shader failure. The first compile also caught a mutable-array borrow
in generator setup. All were corrected before the successful runs above.

## A differing pixel names its last contributor

`Frame::new(width, height, true)` enables a primitive-index buffer. A positive
source alpha records the last contributing primitive, so transparent draws do
not erase provenance. `Frame::blame` resolves it to the node ID and source key
(which can be a file:line string supplied by an app). Diff JSON includes the first
outlier's x/y, expected/actual RGBA, node and source. This is top-contributor blame,
not a claim that the top node caused every error under translucent overlays.

```sh
cargo run -p rune-ref --features gpu -- \
  --scene crates/rune-ref/regressions/glyph-texel-boundary.json --diff --blame 13,34
```

`--seed N --save scene.json` creates a replay file. Without `--diff`, the CLI
needs no GPU and prints the exact BLAKE3 hash. Invalid scene geometry/atlas data
is rejected before upload. The optional provenance buffer is allocated once;
repeated rendering allocates nothing.

## Exact gates are scripts, with Linux instruction measurement pending

```sh
sh scripts/check-reference.sh --gpu
```

That runs exact CPU hashes/metamorphic properties, both allocation gates, GPU
differential fuzzing and saved regressions. The normal command without `--gpu`
still runs the production runtime allocator test, but does not initialize wgpu.

`benches/instructions.rs` uses gungraun 0.20.0 [3,4] to count one fixed warmed
reference frame. Scene/frame allocation and destruction sit outside the measured
function. `scripts/check-reference-instructions.sh save` records a named base;
`check` compares the candidate, failing above **+0.5% instructions**. It checks
the Rust/Valgrind/runner version manifest and refuses a missing baseline marker.
Run both revisions on the same Linux worker with the same Cargo target directory.
Install the matching `gungraun-runner --version 0.20.0` and Valgrind first.

No Linux worker was available when this was written, so instruction counts and
the Linux-only benchmark have not been executed. No instruction
number or performance improvement is claimed. These scripts are ready for any Linux runner. The first Linux run must validate the benchmark and save its baseline.

## Sources and remaining verification

1. [Vello fuzz README](https://github.com/linebender/vello/blob/main/fuzz/README.md). Its default permits 2 per channel and 4 outlier pixels;
   Rune intentionally permits zero outliers.
2. [WGSL floating-point evaluation](https://www.w3.org/TR/WGSL/#floating-point-evaluation)
   and research/14 §3 for FMA/reassociation and tolerance policy.
3. [Gungraun regression guide](https://github.com/gungraun/gungraun/blob/main/docs/src/regressions.md). Version 0.20 uses `Callgrind::soft_limits`.
4. [Gungraun baselines](https://github.com/gungraun/gungraun/blob/main/docs/src/cli_and_env/baselines.md).
5. Rune `shader.wgsl` and `render.rs`; research/12 and /14; PLAN tenet 7 and §6c.

Every Renderer caller supplies target dimensions as the viewport, so fragment
position has the required coordinates. Static glyphs are pixel-snapped, which
keeps their texel choices unchanged. Non-Linux workspace benches skip cleanly; the
instruction-gate script still rejects non-Linux execution.

Remaining: Linux instruction execution, x86/arm CPU hash comparison, Vulkan/D3D12
budget measurement, broader app snapshot comparison, and the new vector kinds.
