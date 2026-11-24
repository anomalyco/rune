//! The precompiled DX12 shaders are FXC's compile of the HLSL wgpu generates today. Run
//! `cargo run -p rune --example regen_dxbc` on Windows to rewrite all bytecode and hashes after
//! the WGSL, the uber layout or wgpu changes.

#![allow(unsafe_code)]

use std::sync::{Mutex, OnceLock};

/// HLSL wgpu-hal logged at debug level, with the thread that logged it.
static HLSL: Mutex<Vec<(std::thread::ThreadId, String)>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool { metadata.target().starts_with("wgpu_hal") }
    fn log(&self, record: &log::Record) {
        let text = record.args().to_string();
        if text.starts_with("Naga generated shader for") {
            HLSL.lock().unwrap().push((std::thread::current().id(), text));
        }
    }
    fn flush(&self) {}
}

fn capture() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        log::set_logger(&Capture).expect("no other test installs a logger");
        log::set_max_level(log::LevelFilter::Debug);
    });
}

#[link(name = "d3dcompiler")]
unsafe extern "system" {
    fn D3DCompile(source: *const u8, len: usize, name: *const i8, defines: *const u8, include: *mut u8,
        entry: *const i8, target: *const i8, flags1: u32, flags2: u32, code: *mut *mut Blob, errors: *mut *mut Blob) -> i32;
}

#[repr(C)]
struct Blob { vtbl: *const BlobVtbl }

#[repr(C)]
struct BlobVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: unsafe extern "system" fn(*mut Blob) -> u32,
    pointer: unsafe extern "system" fn(*mut Blob) -> *const u8,
    size: unsafe extern "system" fn(*mut Blob) -> usize,
}

/// FXC as wgpu-hal calls it: strictness only, shader model 5.1.
fn fxc(source: &str, entry: &str, target: &str) -> Vec<u8> {
    const D3DCOMPILE_ENABLE_STRICTNESS: u32 = 1 << 11;
    let entry = std::ffi::CString::new(entry).unwrap();
    let target = std::ffi::CString::new(target).unwrap();
    let (mut code, mut errors) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: D3DCompile reads `source` and fills two COM blobs, each read and released once.
    unsafe {
        let hr = D3DCompile(source.as_ptr(), source.len(), std::ptr::null(), std::ptr::null(), std::ptr::null_mut(),
            entry.as_ptr(), target.as_ptr(), D3DCOMPILE_ENABLE_STRICTNESS, 0, &mut code, &mut errors);
        let read = |blob: *mut Blob| {
            let bytes = std::slice::from_raw_parts(((*(*blob).vtbl).pointer)(blob), ((*(*blob).vtbl).size)(blob)).to_vec();
            ((*(*blob).vtbl).release)(blob);
            bytes
        };
        if hr < 0 {
            panic!("FXC failed ({hr:#x}): {}", String::from_utf8_lossy(&read(errors)));
        }
        if !errors.is_null() { read(errors); }
        read(code)
    }
}

fn logged_here() -> Vec<String> {
    let me = std::thread::current().id();
    HLSL.lock().unwrap().iter().filter(|(thread, _)| *thread == me).map(|(_, text)| text.clone()).collect()
}

fn stage(logged: &[String], name: &str) -> String {
    let prefix = format!("Naga generated shader for \"{name}\"");
    let text = logged.iter().rev().find(|text| text.starts_with(&prefix))
        .unwrap_or_else(|| panic!("wgpu logged no HLSL for {name}: is the device DX12 with FXC?"));
    text.split_once(":\n").unwrap().1.to_owned()
}

fn stages(logged: &[String], name: &str) -> Vec<String> {
    let prefix = format!("Naga generated shader for \"{name}\"");
    logged.iter().filter(|text| text.starts_with(&prefix)).map(|text| text.split_once(":\n").unwrap().1.to_owned()).collect()
}

fn hlsl_hash(vertex: &str, fragment: &str) -> String {
    blake3::hash(format!("{vertex}\0{fragment}").as_bytes()).to_hex().to_string()
}

fn wgsl_hash(wgsl: &str) -> String {
    blake3::hash(wgsl.replace("\r\n", "\n").as_bytes()).to_hex().to_string()
}

const STALE: &str = "precompiled DX12 shaders are stale: run `cargo run -p rune --example regen_dxbc` on Windows";

#[test]
fn uber_dxbc_is_fresh() {
    capture();
    let Some(gpu) = crate::render::Gpu::test_headless() else { return };
    let start = logged_here().len();
    let mut renderer = super::with_dxbc_enabled(false, || crate::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm));
    let uber_end = logged_here().len();
    renderer.compile_subpixel_now(&gpu.device).expect("the subpixel pipeline compiles");
    let dual_end = logged_here().len();
    let mut pinned_spans = Vec::new();
    for &kind in &crate::render::PINNED_KINDS {
        let from = logged_here().len();
        renderer.compile_pinned_now(&gpu.device, kind).expect("the pinned pipeline compiles");
        pinned_spans.push((kind, from, logged_here().len()));
    }
    let logged = logged_here();
    let (vertex, fragment) = (stage(&logged[start..uber_end], "vs"), stage(&logged[start..uber_end], "fs"));
    let (dual_vertex, dual_fragment) = (stage(&logged[uber_end..dual_end], "vs"), stage(&logged[uber_end..dual_end], "fs"));
    let (vs, fs) = (fxc(&vertex, "vs", "vs_5_1"), fxc(&fragment, "fs", "ps_5_1"));
    let (vs_dual, fs_dual) = (fxc(&dual_vertex, "vs", "vs_5_1"), fxc(&dual_fragment, "fs", "ps_5_1"));
    let (_, wgsl, abi) = crate::render::uber_shader_identity();
    let hlsl = hlsl_hash(&vertex, &fragment);
    let dual = hlsl_hash(&dual_vertex, &dual_fragment);
    assert_eq!(wgsl, super::UBER_DXBC_WGSL_BLAKE3, "{STALE}");
    assert_eq!(abi, super::UBER_DXBC_ABI_FNV64, "{STALE}");
    assert_eq!(hlsl, super::UBER_DXBC_HLSL_BLAKE3, "{STALE}");
    assert_eq!(dual, super::UBER_DXBC_DUAL_HLSL_BLAKE3, "{STALE}");
    assert!(vs == super::VERTEX && fs == super::FRAGMENT && vs_dual == super::VERTEX_DUAL && fs_dual == super::FRAGMENT_DUAL, "{STALE}");
    assert_eq!(super::PINNED_FRESH, [true; 3], "{STALE}");
    assert_eq!(blake3::hash(&vs).to_hex().as_str(), super::UBER_DXBC_PINNED_VS_BLAKE3, "{STALE}");
    for (at, &(kind, from, to)) in pinned_spans.iter().enumerate() {
        let fs_pinned = fxc(&stage(&logged[from..to], "fs"), "fs", "ps_5_1");
        assert_eq!(crate::render::pinned_wgsl_blake3(kind).as_deref(), Some(super::UBER_DXBC_PINNED_WGSL_BLAKE3[at]), "pinned kind {kind}: {STALE}");
        assert_eq!(blake3::hash(&fs_pinned).to_hex().as_str(), super::UBER_DXBC_PINNED_FS_BLAKE3[at], "pinned kind {kind}: {STALE}");
        assert!(fs_pinned == super::FRAGMENT_PINNED[at], "uber.fs-k{kind}.dxbc: {STALE}");
    }
}

#[test]
fn pass_dxbc_is_fresh() {
    capture();
    let Some(gpu) = crate::render::Gpu::test_headless() else { return };

    assert_eq!(wgsl_hash(crate::render::surface_alpha::WGSL), super::ALPHA_DXBC_WGSL_BLAKE3, "surface_alpha: {STALE}");
    let start = logged_here().len();
    super::with_dxbc_enabled(false, || crate::render::make_straight_alpha(&gpu.device, wgpu::TextureFormat::Bgra8Unorm));
    let logged = logged_here();
    let (alpha_vs, alpha_fs) = (fxc(&stage(&logged[start..], "vs"), "vs", "vs_5_1"), fxc(&stage(&logged[start..], "fs"), "fs", "ps_5_1"));
    assert!(alpha_vs == super::ALPHA_VERTEX && alpha_fs == super::ALPHA_FRAGMENT, "alpha.dxbc: {STALE}");

    assert_eq!(wgsl_hash(crate::render::layer_blur::WGSL), super::LAYER_BLUR_DXBC_WGSL_BLAKE3, "layer_blur: {STALE}");
    let start = logged_here().len();
    super::with_dxbc_enabled(false, || crate::render::make_layer_blur(&gpu.device));
    let logged = logged_here();
    let blur_vs = fxc(&stage(&logged[start..], "vs"), "vs", "vs_5_1");
    let blur_fragments = stages(&logged[start..], "fs");
    assert_eq!(blur_fragments.len(), super::LAYER_BLUR_FRAGMENTS.len(), "layer_blur pipelines: {STALE}");
    assert!(blur_vs == super::LAYER_BLUR_VERTEX, "layer_blur.vs.dxbc: {STALE}");
    for (i, hlsl) in blur_fragments.iter().enumerate() {
        assert!(fxc(hlsl, "fs", "ps_5_1") == super::LAYER_BLUR_FRAGMENTS[i], "layer_blur.fs{i}.dxbc: {STALE}");
    }

    assert_eq!(wgsl_hash(crate::render::backdrop_blur::BLUR_WGSL), super::BACKDROP_BLUR_DXBC_WGSL_BLAKE3, "backdrop_blur: {STALE}");
    assert_eq!(wgsl_hash(&crate::render::backdrop_blur::composite_wgsl()), super::BACKDROP_COMPOSITE_DXBC_WGSL_BLAKE3, "backdrop_composite: {STALE}");
    let renderer = crate::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
    let start = logged_here().len();
    super::with_dxbc_enabled(false, || renderer.make_backdrop_blur(&gpu.device));
    let logged = logged_here();
    let (vertices, down, up, fragments) = (stages(&logged[start..], "vs"), stages(&logged[start..], "fs_down"), stages(&logged[start..], "fs_up"), stages(&logged[start..], "fs"));
    assert!(vertices.len() >= 2 && down.len() == 1 && up.len() == 1 && fragments.len() == 1);
    assert!(fxc(&vertices[0], "vs", "vs_5_1") == super::BACKDROP_BLUR[0], "backdrop_blur.vs.dxbc: {STALE}");
    assert!(fxc(&down[0], "fs_down", "ps_5_1") == super::BACKDROP_BLUR[1], "backdrop_blur.fs_down.dxbc: {STALE}");
    assert!(fxc(&up[0], "fs_up", "ps_5_1") == super::BACKDROP_BLUR[2], "backdrop_blur.fs_up.dxbc: {STALE}");
    assert!(fxc(vertices.last().unwrap(), "vs", "vs_5_1") == super::BACKDROP_COMPOSITE[0], "backdrop_composite.vs.dxbc: {STALE}");
    assert!(fxc(&fragments[0], "fs", "ps_5_1") == super::BACKDROP_COMPOSITE[1], "backdrop_composite.fs.dxbc: {STALE}");
}

/// rune-media's video pass (native surfaces): its two stages and the hash of the WGSL they came
/// from, which `native_surface::dxbc` checks before it uses them.
#[test]
fn video_dxbc_is_fresh() {
    capture();
    let Some(gpu) = crate::render::Gpu::test_headless() else { return };
    let media = concat!(env!("CARGO_MANIFEST_DIR"), "/../rune-media");
    let wgsl = std::fs::read_to_string(format!("{media}/src/native_surface/video.wgsl")).unwrap();
    let recorded = std::fs::read_to_string(format!("{media}/src/native_surface/dxbc_hash.rs")).unwrap();
    assert!(recorded.contains(&format!("\"{}\"", wgsl_hash(&wgsl))), "video.wgsl: {STALE}");
    let start = logged_here().len();
    crate::native_surface::SurfaceRenderer::compile_wgsl_now(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
    let logged = logged_here();
    let (vs, fs) = (fxc(&stage(&logged[start..], "vs"), "vs", "vs_5_1"), fxc(&stage(&logged[start..], "fs"), "fs", "ps_5_1"));
    assert!(vs == std::fs::read(format!("{media}/shaders/dxbc/video.vs.dxbc")).unwrap(), "video.vs.dxbc: {STALE}");
    assert!(fs == std::fs::read(format!("{media}/shaders/dxbc/video.fs.dxbc")).unwrap(), "video.fs.dxbc: {STALE}");
}

/// A DX12 device loads every precompiled shader (general uber, pinned kinds and built-in passes),
/// and without the passthrough path compiles the WGSL.
#[test]
fn renderers_load_the_precompiled_uber_shader() {
    let Some(gpu) = crate::render::Gpu::test_headless() else { return };
    assert!(super::uber(&gpu.device, |_| ()).is_some(), "a DX12 device takes the uber bytecode");
    for &kind in &crate::render::PINNED_KINDS {
        assert!(super::uber_pinned(&gpu.device, kind, |_| ()).is_some(), "pinned kind {kind} loads");
    }
    assert!(super::surface_alpha(&gpu.device, crate::render::surface_alpha::WGSL, |_| ()).is_some(), "straight alpha loads");
    assert!(super::layer_blur(&gpu.device, crate::render::layer_blur::WGSL, |_| ()).is_some(), "layer blur loads");
    assert!(super::backdrop_blur(&gpu.device, crate::render::backdrop_blur::BLUR_WGSL, |_| ()).is_some(), "backdrop blur loads");
    assert!(super::backdrop_composite(&gpu.device, &crate::render::backdrop_blur::composite_wgsl()).is_some(), "backdrop composite loads");
    assert!(super::with_dxbc_enabled(false, || super::uber(&gpu.device, |_| ())).is_none());
    drop(crate::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm));
}
