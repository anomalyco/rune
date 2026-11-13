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
//! (Windows) compares the HLSL wgpu generates now with the HLSL the bytecode came from, and
//! `cargo run -p rune --example regen_dxbc` rewrites both without building the tests. The
//! bytecode is used only while the WGSL hashes as it did when it was compiled, which the build
//! knows (`build.rs` hashes the WGSL with `\r\n` read as `\n`, so a CRLF checkout matches too, and
//! warns when it doesn't match), and while the layout hashes the same. A launch that can't use it
//! says why once; a pipeline the driver refuses falls back to WGSL.

#[cfg(target_os = "windows")]
include!("uber/dxbc_hash.rs");

#[cfg(target_os = "windows")]
const VERTEX: &[u8] = include_bytes!("../../uber.vs.dxbc");
#[cfg(target_os = "windows")]
const FRAGMENT: &[u8] = include_bytes!("../../uber.fs.dxbc");
/// The subpixel variant's stages (render/variant.rs `dual_wgsl`), whose fragment blends by a
/// second output. FXC takes about a minute for it, so a window drawing subpixel text froze that
/// long compiling it.
#[cfg(target_os = "windows")]
const VERTEX_DUAL: &[u8] = include_bytes!("../../uber.vs-dual.dxbc");
#[cfg(target_os = "windows")]
const FRAGMENT_DUAL: &[u8] = include_bytes!("../../uber.fs-dual.dxbc");

// The built-in passes besides the uber shader, compiled by regen_dxbc from their WGSL as wgpu
// would, and the BLAKE3 of the WGSL each came from (`uber/pass_dxbc_hash.rs`).
#[cfg(target_os = "windows")]
include!("uber/pass_dxbc_hash.rs");
#[cfg(target_os = "windows")]
const ALPHA_VERTEX: &[u8] = include_bytes!("../../alpha.vs.dxbc");
#[cfg(target_os = "windows")]
const ALPHA_FRAGMENT: &[u8] = include_bytes!("../../alpha.fs.dxbc");

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
    load(device, (VERTEX, FRAGMENT), "uber", uber_fresh, make_pipeline)
}

/// [`uber`] for the subpixel variant.
#[cfg(target_os = "windows")]
pub(super) fn uber_dual<T>(device: &wgpu::Device, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    load(device, (VERTEX_DUAL, FRAGMENT_DUAL), "subpixel uber", uber_fresh, make_pipeline)
}

/// The straight-alpha pass (render/surface_alpha.rs) from its precompiled stages, when they were
/// compiled from `wgsl` and the driver takes them.
#[cfg(target_os = "windows")]
pub(super) fn surface_alpha<T>(device: &wgpu::Device, wgsl: &str, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    load(device, (ALPHA_VERTEX, ALPHA_FRAGMENT), "straight alpha", || compiled_from(wgsl, ALPHA_DXBC_WGSL_BLAKE3, "straight alpha"), make_pipeline)
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
fn load<T>(device: &wgpu::Device, (vertex, fragment): (&'static [u8], &'static [u8]), what: &str, fresh: impl FnOnce() -> bool, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
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
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let shaders = Shaders { vertex: module(vertex, "vs"), fragment: module(fragment, "fs") };
    let pipeline = make_pipeline(&shaders);
    let refused = pollster::block_on(internal.pop()).or(pollster::block_on(validation.pop()));
    match refused {
        None => Some((shaders, pipeline)),
        Some(error) => {
            eprintln!("rune: the precompiled {what} shader was refused ({error}); compiling WGSL");
            None
        }
    }
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
    FORCE.with(std::cell::Cell::get).unwrap_or_else(|| !matches!(std::env::var("RUNE_DISABLE_DXBC").as_deref(), Ok("1" | "true" | "TRUE" | "yes")))
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
