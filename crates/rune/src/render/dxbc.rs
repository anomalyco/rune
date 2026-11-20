//! Precompiled DX12 bytecode for Rune's uber shader on Windows.
//!
//! wgpu 30's DX12 backend translates `shader.wgsl` through Naga to HLSL and compiles it with
//! FXC each time a device makes the uber pipeline: about 250 ms on a desktop CPU, the largest
//! part of a first surface's cost, and paid again by every new device. Rune ships the bytecode
//! FXC makes from that HLSL for the Windfoil variant (render/variant.rs), the one Windows renderers
//! draw with (`uber.vs.dxbc`, `uber.fs.dxbc`), and loads it with
//! `Device::create_shader_module_passthrough`; the pipeline then takes about 2 ms, as the
//! driver caches its own compile across launches. D3D12 takes FXC's DXBC as it takes DXIL.
//!
//! The bytecode must be what wgpu itself would compile: the same HLSL, whose register
//! assignments wgpu's root signature for the uber layout expects. `uber_dxbc_is_fresh`
//! (Windows) compares the HLSL wgpu generates now with the HLSL the general and subpixel bytecode
//! came from, and `cargo run -p rune --example regen_dxbc` rewrites everything, the pinned fragments
//! (`uber.fs-k0/k1/k2.dxbc`) and the hash of the WGSL each came from included: no other path writes
//! those, and a pinned fragment is used only while the build finds that WGSL and those files (`PINNED_FRESH`). The
//! bytecode is used only while the WGSL hashes as it did when it was compiled, which the build
//! knows (`build.rs` hashes the WGSL with `\r\n` read as `\n`, so a CRLF checkout matches too, and
//! warns when it doesn't match), and while the layout hashes the same. A launch that can't use it
//! says why once; a pipeline the driver refuses falls back to WGSL.

#[cfg(target_os = "windows")]
include!("uber/dxbc_hash.rs");
#[cfg(target_os = "windows")]
include!("uber/dxbc_pinned_hash.rs");

#[cfg(target_os = "windows")]
const VERTEX: &[u8] = include_bytes!("../../shaders/dxbc/uber.vs.dxbc");
#[cfg(target_os = "windows")]
const FRAGMENT: &[u8] = include_bytes!("../../shaders/dxbc/uber.fs.dxbc");
/// The subpixel variant's stages (render/variant.rs `dual_wgsl`), whose fragment blends by a
/// second output. FXC takes about a minute for it, so a window drawing subpixel text froze that
/// long compiling it.
#[cfg(target_os = "windows")]
const VERTEX_DUAL: &[u8] = include_bytes!("../../shaders/dxbc/uber.vs-dual.dxbc");
#[cfg(target_os = "windows")]
const FRAGMENT_DUAL: &[u8] = include_bytes!("../../shaders/dxbc/uber.fs-dual.dxbc");
/// The fragment stage pinned to one kind each (render/variant.rs `pinned_wgsl`, `PINNED`), beside the
/// shared vertex stage: the frame pass's pipelines for rects, shadows and glyphs.
#[cfg(target_os = "windows")]
const FRAGMENT_PINNED: [&[u8]; 3] = [
    include_bytes!("../../shaders/dxbc/uber.fs-k0.dxbc"),
    include_bytes!("../../shaders/dxbc/uber.fs-k1.dxbc"),
    include_bytes!("../../shaders/dxbc/uber.fs-k2.dxbc"),
];
// `FRAGMENT_PINNED` is in `PINNED`'s order and its files are named by kind: the two must agree.
#[cfg(target_os = "windows")]
const _: () = assert!(super::variant::PINNED[0] == 0 && super::variant::PINNED[1] == 1 && super::variant::PINNED[2] == 2,
    "render/variant.rs PINNED changed: name FRAGMENT_PINNED's files after it and run regen_dxbc");

// The built-in passes besides the uber shader, compiled by regen_dxbc from their WGSL as wgpu
// would, and the BLAKE3 of the WGSL each came from (`uber/pass_dxbc_hash.rs`).
#[cfg(target_os = "windows")]
include!("uber/pass_dxbc_hash.rs");
#[cfg(target_os = "windows")]
const ALPHA_VERTEX: &[u8] = include_bytes!("../../shaders/dxbc/alpha.vs.dxbc");
#[cfg(target_os = "windows")]
const ALPHA_FRAGMENT: &[u8] = include_bytes!("../../shaders/dxbc/alpha.fs.dxbc");
#[cfg(target_os = "windows")]
const LAYER_BLUR_VERTEX: &[u8] = include_bytes!("../../shaders/dxbc/layer_blur.vs.dxbc");
#[cfg(target_os = "windows")]
const BACKDROP_BLUR: [&[u8]; 3] = [
    include_bytes!("../../shaders/dxbc/backdrop_blur.vs.dxbc"),
    include_bytes!("../../shaders/dxbc/backdrop_blur.fs_down.dxbc"),
    include_bytes!("../../shaders/dxbc/backdrop_blur.fs_up.dxbc"),
];
#[cfg(target_os = "windows")]
const BACKDROP_COMPOSITE: [&[u8]; 2] = [
    include_bytes!("../../shaders/dxbc/backdrop_composite.vs.dxbc"),
    include_bytes!("../../shaders/dxbc/backdrop_composite.fs.dxbc"),
];
#[cfg(target_os = "windows")]
const LAYER_BLUR_FRAGMENTS: [&[u8]; 5] = [
    include_bytes!("../../shaders/dxbc/layer_blur.fs0.dxbc"),
    include_bytes!("../../shaders/dxbc/layer_blur.fs1.dxbc"),
    include_bytes!("../../shaders/dxbc/layer_blur.fs2.dxbc"),
    include_bytes!("../../shaders/dxbc/layer_blur.fs3.dxbc"),
    include_bytes!("../../shaders/dxbc/layer_blur.fs4.dxbc"),
];

/// The uber shader's modules: one WGSL (or metallib) module for both stages, or one
/// precompiled module per stage.
#[derive(Clone)]
pub(crate) struct Shaders {
    pub vertex: wgpu::ShaderModule,
    pub fragment: wgpu::ShaderModule,
}

impl Shaders {
    pub(crate) fn one(module: wgpu::ShaderModule) -> Self {
        Self { vertex: module.clone(), fragment: module }
    }
}

/// What a caller does when its precompiled shader is refused: makes it from WGSL.
#[cfg(target_os = "windows")]
const WGSL: &str = "compiling WGSL";

/// Whether the bytecode was compiled from the WGSL this build joined (render/uber_wgsl.rs).
#[cfg(target_os = "windows")]
const WGSL_MATCHES: bool = super::uber_wgsl::same(super::uber_wgsl::WINDFOIL_BLAKE3, UBER_DXBC_WGSL_BLAKE3);

/// Whether it was compiled for Windfoil's pipeline layout as this build has it.
#[cfg(target_os = "windows")]
const ABI_MATCHES: bool = super::variant::windfoil_abi_hash() == UBER_DXBC_ABI_FNV64;

// A build whose layout isn't the bytecode's warns, as build.rs does for the WGSL.
#[cfg(target_os = "windows")]
const _: () = Layout::<ABI_MATCHES>::check();

#[cfg(target_os = "windows")]
struct Layout<const MATCHES: bool>;

#[cfg(target_os = "windows")]
#[allow(dead_code)]
impl Layout<true> {
    const fn check() {}
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
impl Layout<false> {
    #[deprecated(note = "the precompiled DX12 uber shader was compiled for another pipeline layout, so it isn't used: \
        run `cargo run -p rune --example regen_dxbc` on Windows")]
    const fn check() {}
}

/// The precompiled Windfoil uber shader and its pipeline on a DX12 device, when the bytecode
/// matches Windfoil's WGSL and layout and the driver takes it.
#[cfg(target_os = "windows")]
pub(super) fn uber<T>(device: &wgpu::Device, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    load(device, (VERTEX, FRAGMENT), "uber", uber_fresh, WGSL, make_pipeline)
}

/// [`uber`] for the subpixel variant.
#[cfg(target_os = "windows")]
pub(super) fn uber_dual<T>(device: &wgpu::Device, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    load(device, (VERTEX_DUAL, FRAGMENT_DUAL), "subpixel uber", uber_fresh, WGSL, make_pipeline)
}

/// [`uber`] pinned to `KIND_*` `kind` (render/variant.rs `PINNED`): the shared vertex stage and that kind's fragment,
/// only while that fragment was compiled from the WGSL `pinned_wgsl(kind)` gives now (`pinned_fresh`). A stale one
/// isn't used: the general shader draws the kind.
#[cfg(target_os = "windows")]
pub(super) fn uber_pinned<T>(device: &wgpu::Device, kind: u32, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    let at = super::variant::PINNED.iter().position(|&k| k == kind)?;
    let fragment = Some(FRAGMENT_PINNED[at]).filter(|f| !f.is_empty())?;
    load(device, (VERTEX, fragment), "pinned uber", || uber_fresh() && pinned_fresh(at, kind), "the general shader draws that kind", make_pipeline)
}

/// Whether each pinned fragment (`PINNED` order) is the one `regen_dxbc` compiled from this build's WGSL beside this
/// build's vertex stage: the WGSL hash and both files' BLAKE3 that build.rs takes (render/uber_wgsl.rs) against what
/// `regen_dxbc` recorded with them (`uber/dxbc_pinned_hash.rs`). Settled at compile time, so nothing is hashed at run
/// time. A fragment left by another shader or beside a regenerated vertex stage (which is how a wgpu or Naga upgrade
/// shows), a hand-edited hash or a swapped file all fail it, and the general shader draws that kind.
#[cfg(target_os = "windows")]
const PINNED_FRESH: [bool; 3] = {
    use super::uber_wgsl::same;
    let mut fresh = [false; 3];
    let mut at = 0;
    while at < 3 {
        fresh[at] = same(super::uber_wgsl::PINNED_WGSL_BLAKE3[at], UBER_DXBC_PINNED_WGSL_BLAKE3[at])
            && same(super::uber_wgsl::PINNED_FS_BLAKE3[at], UBER_DXBC_PINNED_FS_BLAKE3[at])
            && same(super::uber_wgsl::PINNED_VS_BLAKE3, UBER_DXBC_PINNED_VS_BLAKE3);
        at += 1;
    }
    fresh
};

/// [`PINNED_FRESH`] for the fragment at `at`; says once for each stale kind (build.rs warns too).
#[cfg(target_os = "windows")]
fn pinned_fresh(at: usize, kind: u32) -> bool {
    if !PINNED_FRESH[at] {
        static STALE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let mut said = STALE.lock().unwrap_or_else(|e| e.into_inner());
        if !said.contains(&kind) {
            said.push(kind);
            eprintln!("rune: the precompiled DX12 fragment pinned to kind {kind} (uber.fs-k{kind}.dxbc) wasn't compiled from this shader beside this vertex stage, so the general shader draws that kind (scrolling on the slow path); rebuild it with `cargo run -p rune --example regen_dxbc` on Windows");
        }
    }
    PINNED_FRESH[at]
}
/// The straight-alpha pass (render/surface_alpha.rs) from its precompiled stages, when they were
/// compiled from `wgsl` and the driver takes them.
#[cfg(target_os = "windows")]
pub(super) fn surface_alpha<T>(device: &wgpu::Device, wgsl: &str, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    load(device, (ALPHA_VERTEX, ALPHA_FRAGMENT), "straight alpha", || compiled_from(wgsl, ALPHA_DXBC_WGSL_BLAKE3, "straight alpha"), WGSL, make_pipeline)
}

/// Whether a pass's bytecode came from `wgsl` (its `\r\n` read as `\n`, as build.rs reads the
/// uber shader's); says so when it didn't.
#[cfg(target_os = "windows")]
fn compiled_from(wgsl: &str, recorded: &str, what: &str) -> bool {
    let now = blake3::hash(wgsl.replace("\r\n", "\n").as_bytes()).to_hex();
    if now.as_str() == recorded { return true; }
    eprintln!("rune: the precompiled DX12 {what} shader was built from other WGSL (blake3 {recorded}, now {now}); compiling WGSL. Rebuild it with `cargo run -p rune --example regen_dxbc` on Windows");
    false
}

#[cfg(target_os = "windows")]
fn load<T>(device: &wgpu::Device, (vertex, fragment): (&'static [u8], &'static [u8]), what: &str, fresh: impl FnOnce() -> bool, otherwise: &str,
    make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    load_modules(device, &[(vertex, "vs"), (fragment, "fs")], what, fresh, otherwise, |modules| {
        let shaders = Shaders { vertex: modules[0].clone(), fragment: modules[1].clone() };
        let pipeline = make_pipeline(&shaders);
        (shaders, pipeline)
    })
}

/// Makes a passthrough module from each `(bytecode, entry point)` and hands them to
/// `make_pipelines`, when this is a DX12 device that takes bytecode and `fresh` says it's this
/// build's; `None` (so the caller does `otherwise`, which a refusal names) when not, or when the driver refuses it.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn load_modules<T>(device: &wgpu::Device, blobs: &[(&'static [u8], &'static str)], what: &str, fresh: impl FnOnce() -> bool, otherwise: &str,
    make_pipelines: impl FnOnce(&[wgpu::ShaderModule]) -> T) -> Option<T> {
    if device.adapter_info().backend != wgpu::Backend::Dx12 || !device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS) || !enabled() {
        return None;
    }
    if !fresh() { return None; }
    let module = |blob: &'static [u8], entry: &'static str| {
        // SAFETY: the bytecode is FXC's compile of the HLSL wgpu generates for this pass's WGSL with
        // its layout (regen_dxbc; `fresh` checked it's that WGSL), so it binds what the layout declares.
        unsafe {
            device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                label: Some(what),
                entry_points: std::borrow::Cow::Owned(vec![wgpu::PassthroughShaderEntryPoint { name: entry.into(), workgroup_size: (0, 0, 0) }]),
                dxil: Some(std::borrow::Cow::Borrowed(blob)),
                ..Default::default()
            })
        }
    };
    let modules: Vec<wgpu::ShaderModule> = blobs.iter().map(|&(blob, entry)| module(blob, entry)).collect();
    checked(device, what, otherwise, || make_pipelines(&modules))
}

/// `make`'s pipelines from precompiled modules, or `None` (said once on stderr, with what the
/// caller does `otherwise`) when the driver refuses them.
#[cfg(target_os = "windows")]
pub(super) fn checked<T>(device: &wgpu::Device, what: &str, otherwise: &str, make: impl FnOnce() -> T) -> Option<T> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let made = make();
    let refused = pollster::block_on(internal.pop()).or(pollster::block_on(validation.pop()));
    match refused {
        None => Some(made),
        Some(error) => {
            eprintln!("rune: the precompiled {what} shader was refused ({error}); {otherwise}");
            None
        }
    }
}

/// The backdrop blur's down and up passes (render/backdrop_blur.rs): its vertex stage and its
/// `fs_down` and `fs_up` fragment stages, when they were compiled from `wgsl`.
#[cfg(target_os = "windows")]
pub(super) fn backdrop_blur<T>(device: &wgpu::Device, wgsl: &str, make_pipelines: impl FnOnce(&[wgpu::ShaderModule]) -> T) -> Option<T> {
    let blobs = [(BACKDROP_BLUR[0], "vs"), (BACKDROP_BLUR[1], "fs_down"), (BACKDROP_BLUR[2], "fs_up")];
    load_modules(device, &blobs, "backdrop blur", || compiled_from(wgsl, BACKDROP_BLUR_DXBC_WGSL_BLAKE3, "backdrop blur"), WGSL, make_pipelines)
}

/// The backdrop blur's composite stages, which bind the uber shader's group 0: when they were
/// compiled from `wgsl` and for this build's uber layout. Its pipelines are made per target format
/// later, each through [`checked`].
#[cfg(target_os = "windows")]
pub(super) fn backdrop_composite(device: &wgpu::Device, wgsl: &str) -> Option<Shaders> {
    let fresh = || ABI_MATCHES && compiled_from(wgsl, BACKDROP_COMPOSITE_DXBC_WGSL_BLAKE3, "backdrop blur composite");
    load_modules(device, &[(BACKDROP_COMPOSITE[0], "vs"), (BACKDROP_COMPOSITE[1], "fs")], "backdrop blur composite", fresh, WGSL,
        |m| Shaders { vertex: m[0].clone(), fragment: m[1].clone() })
}

/// The layer blur's vertex stage and its five fragment variants (render/layer_blur.rs `MODES`, the
/// WGSL's `MODE` and `ADDITIVE` overrides each compiled in), when they were compiled from `wgsl`.
#[cfg(target_os = "windows")]
pub(super) fn layer_blur<T>(device: &wgpu::Device, wgsl: &str, make_pipelines: impl FnOnce(&[wgpu::ShaderModule]) -> T) -> Option<T> {
    let blobs = [(LAYER_BLUR_VERTEX, "vs"), (LAYER_BLUR_FRAGMENTS[0], "fs"), (LAYER_BLUR_FRAGMENTS[1], "fs"), (LAYER_BLUR_FRAGMENTS[2], "fs"),
        (LAYER_BLUR_FRAGMENTS[3], "fs"), (LAYER_BLUR_FRAGMENTS[4], "fs")];
    load_modules(device, &blobs, "layer gaussian", || compiled_from(wgsl, LAYER_BLUR_DXBC_WGSL_BLAKE3, "layer gaussian"), WGSL, make_pipelines)
}
/// Whether the uber bytecode matches this build's Windfoil WGSL and layout; says why once when not.
#[cfg(target_os = "windows")]
fn uber_fresh() -> bool {
    if !WGSL_MATCHES || !ABI_MATCHES { stale(); }
    WGSL_MATCHES && ABI_MATCHES
}

/// Says once why the shipped bytecode can't be used, and how to rebuild it.
#[cfg(target_os = "windows")]
fn stale() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let why = if WGSL_MATCHES {
            format!("the uber pipeline layout changed (it hashes {:#018x}; the bytecode's was {UBER_DXBC_ABI_FNV64:#018x})", super::variant::windfoil_abi_hash())
        } else {
            format!("the WGSL changed (blake3 {}; the bytecode's was {UBER_DXBC_WGSL_BLAKE3})", super::uber_wgsl::WINDFOIL_BLAKE3)
        };
        eprintln!("rune: the precompiled DX12 uber shader doesn't match this build: {why}. FXC compiles the WGSL instead, \
            which takes seconds on every launch; rebuild the bytecode with `cargo run -p rune --example regen_dxbc` on Windows");
    });
}

#[cfg(target_os = "windows")]
std::thread_local! {
    static FORCE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// `RUNE_DISABLE_DXBC=1` compiles the WGSL instead, as before.
#[cfg(target_os = "windows")]
fn enabled() -> bool {
    FORCE.with(std::cell::Cell::get).unwrap_or_else(|| !rune_core::env::flag("RUNE_DISABLE_DXBC"))
}

#[cfg(all(test, target_os = "windows"))]
pub(super) fn with_dxbc_enabled<R>(enabled: bool, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) { FORCE.with(|c| c.set(self.0)); }
    }
    let _restore = Restore(FORCE.with(|c| c.replace(Some(enabled))));
    f()
}

#[cfg(all(test, target_os = "windows"))]
#[path = "dxbc_tests.rs"]
mod tests;
