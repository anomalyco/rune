//! Precompiled DX12 bytecode for the video pass (`video.wgsl`), which draws every native surface:
//! a browser pane's pages, a video's frames.
//!
//! wgpu's DX12 backend translates the WGSL through Naga to HLSL and compiles it with FXC each time
//! a pipeline is made: 270 ms of D3DCompile on a laptop (i7-12800H) for the first browser picture
//! of every launch, again for the punch pipeline, and on the UI thread for each other target format
//! and the additive pipeline. Rune ships what FXC makes of that HLSL (`video.vs.dxbc`, `video.fs.dxbc`,
//! written with the WGSL's hash by `cargo run -p rune --example regen_dxbc` on Windows, which
//! `video_dxbc_is_fresh` checks) and loads it with `Device::create_shader_module_passthrough`, as
//! Rune's own passes do (`rune/src/render/dxbc.rs`). Every pipeline of the pass has the one layout,
//! so the two stages serve them all. The bytecode is used only while the WGSL hashes as it did when
//! it was compiled; a pipeline the driver refuses compiles the WGSL instead. `RUNE_DISABLE_DXBC=1`
//! compiles the WGSL, as before.

include!("dxbc_hash.rs");

const VERTEX: &[u8] = include_bytes!("../../shaders/dxbc/video.vs.dxbc");
const FRAGMENT: &[u8] = include_bytes!("../../shaders/dxbc/video.fs.dxbc");

/// `make`'s pipeline from the precompiled stages, or `None` (the caller compiles the WGSL) when this
/// isn't a DX12 device that takes bytecode, the bytecode isn't this WGSL's, or the driver refuses it.
#[allow(unsafe_code)]
pub(super) fn pipeline(device: &wgpu::Device, make: impl FnOnce(&wgpu::ShaderModule, &wgpu::ShaderModule) -> wgpu::RenderPipeline) -> Option<wgpu::RenderPipeline> {
    if device.adapter_info().backend != wgpu::Backend::Dx12 || !device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS) {
        return None;
    }
    if rune_core::env::flag("RUNE_DISABLE_DXBC") || !fresh() {
        return None;
    }
    let module = |blob: &'static [u8], entry: &'static str| {
        // SAFETY: the bytecode is FXC's compile of the HLSL wgpu generates for `video.wgsl` with the
        // pass's one layout (regen_dxbc; `fresh` checked it's this WGSL), so it binds what the layout declares.
        unsafe {
            device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                label: Some("video"),
                entry_points: std::borrow::Cow::Owned(vec![wgpu::PassthroughShaderEntryPoint { name: entry.into(), workgroup_size: (0, 0, 0) }]),
                dxil: Some(std::borrow::Cow::Borrowed(blob)),
                ..Default::default()
            })
        }
    };
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let made = make(&module(VERTEX, "vs"), &module(FRAGMENT, "fs"));
    let refused = pollster::block_on(internal.pop()).or(pollster::block_on(validation.pop()));
    let Some(error) = refused else { return Some(made) };
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| eprintln!("rune: the precompiled video shader was refused ({error}); compiling WGSL"));
    None
}

/// Whether the bytecode was compiled from this build's `video.wgsl` (its `\r\n` read as `\n`); says
/// once when it wasn't.
fn fresh() -> bool {
    static FRESH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FRESH.get_or_init(|| {
        let now = blake3::hash(super::pipeline::WGSL.replace("\r\n", "\n").as_bytes()).to_hex();
        let fresh = !VERTEX.is_empty() && !FRAGMENT.is_empty() && now.as_str() == VIDEO_DXBC_WGSL_BLAKE3;
        if !fresh {
            eprintln!("rune: the precompiled DX12 video shader was built from other WGSL (blake3 {VIDEO_DXBC_WGSL_BLAKE3}, now {now}); compiling WGSL. Rebuild it with `cargo run -p rune --example regen_dxbc` on Windows");
        }
        fresh
    })
}
