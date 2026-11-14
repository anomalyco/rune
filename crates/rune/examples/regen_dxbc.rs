//! Rewrites the precompiled DX12 uber shader (`uber.vs.dxbc`, `uber.fs.dxbc`, the subpixel
//! variant's `uber.vs-dual.dxbc` and `uber.fs-dual.dxbc`, and
//! `src/render/uber/dxbc_hash.rs`), and the straight-alpha pass's `alpha.vs.dxbc`, `alpha.fs.dxbc` and
//! `src/render/uber/pass_dxbc_hash.rs`, after `shader.wgsl`, `clip.wgsl`, `outline.wgsl`, the uber layout or wgpu
//! changes, without building Rune's unit tests: `cargo run -p rune --example regen_dxbc` on
//! Windows. It does what `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`
//! does: makes the uber pipeline from WGSL on a DX12 device, takes the HLSL wgpu-hal logs, and
//! compiles it with FXC as wgpu-hal does. `uber_dxbc_is_fresh` checks the result.

#[cfg(target_os = "windows")]
fn main() {
    // The bytecode in the tree may match nothing now; make the pipeline from WGSL either way.
    // SAFETY: set before any other thread starts.
    unsafe { std::env::set_var("RUNE_DISABLE_DXBC", "1") };
    log::set_logger(&Capture).expect("the only logger");
    log::set_max_level(log::LevelFilter::Debug);
    let gpu = rune::render::Gpu::headless();
    let mut renderer = rune::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
    let uber_logged = HLSL.lock().unwrap().len();
    // The subpixel variant (render/variant.rs `dual_wgsl`): FXC takes about a minute for it.
    renderer.compile_subpixel_now(&gpu.device).expect("the subpixel pipeline compiles");
    drop(renderer);
    let logged = HLSL.lock().unwrap();
    let stage = |logged: &[String], name: &str| {
        let text = logged.iter().rev().find(|text| text.starts_with(&format!("Naga generated shader for \"{name}\"")))
            .unwrap_or_else(|| panic!("wgpu logged no HLSL for {name}: is the device DX12 with FXC?"));
        text.split_once(":\n").unwrap().1.to_owned()
    };
    let (vertex, fragment) = (stage(&logged[..uber_logged], "vs"), stage(&logged[..uber_logged], "fs"));
    let (dual_vertex, dual_fragment) = (stage(&logged[uber_logged..], "vs"), stage(&logged[uber_logged..], "fs"));
    let (vs, fs) = (fxc(&vertex, "vs", "vs_5_1"), fxc(&fragment, "fs", "ps_5_1"));
    let started = std::time::Instant::now();
    let (vs_dual, fs_dual) = (fxc(&dual_vertex, "vs", "vs_5_1"), fxc(&dual_fragment, "fs", "ps_5_1"));
    println!("FXC took {:.1} s for the subpixel variant", started.elapsed().as_secs_f64());
    // The hash the build took of the WGSL it joined, CRLF read as LF (crates/rune/build.rs).
    let (_, wgsl, abi) = rune::render::uber_shader_identity();
    let hlsl = blake3::hash(format!("{vertex}\0{fragment}").as_bytes()).to_hex();
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(format!("{root}/uber.vs.dxbc"), &vs).unwrap();
    std::fs::write(format!("{root}/uber.fs.dxbc"), &fs).unwrap();
    std::fs::write(format!("{root}/uber.vs-dual.dxbc"), &vs_dual).unwrap();
    std::fs::write(format!("{root}/uber.fs-dual.dxbc"), &fs_dual).unwrap();
    let dual = blake3::hash(format!("{dual_vertex}\0{dual_fragment}").as_bytes()).to_hex();
    std::fs::write(format!("{root}/src/render/uber/dxbc_hash.rs"), format!(
        "// Written by `cargo run -p rune --example regen_dxbc` (or `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`) on Windows.\n\
         pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = \"{wgsl}\";\n\
         pub(super) const UBER_DXBC_ABI_FNV64: u64 = {abi:#018x};\n\
         #[cfg(test)]\npub(super) const UBER_DXBC_HLSL_BLAKE3: &str = \"{hlsl}\";\n\
         #[cfg(test)]\npub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = \"{dual}\";\n")).unwrap();
    println!("uber.vs.dxbc {} bytes, uber.fs.dxbc {} bytes, uber.fs-dual.dxbc {} bytes, WGSL {wgsl}", vs.len(), fs.len(), fs_dual.len());
    drop(logged);

    // The other built-in passes, each made once from its WGSL (render/dxbc.rs loads them).
    let start = HLSL.lock().unwrap().len();
    rune::render::make_straight_alpha(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
    let logged = HLSL.lock().unwrap();
    let (alpha_vs, alpha_fs) = (fxc(&stage(&logged[start..], "vs"), "vs", "vs_5_1"), fxc(&stage(&logged[start..], "fs"), "fs", "ps_5_1"));
    std::fs::write(format!("{root}/alpha.vs.dxbc"), &alpha_vs).unwrap();
    std::fs::write(format!("{root}/alpha.fs.dxbc"), &alpha_fs).unwrap();
    let alpha_wgsl = blake3::hash(std::fs::read_to_string(format!("{root}/src/surface_alpha.wgsl")).unwrap().replace("\r\n", "\n").as_bytes()).to_hex();
    println!("alpha.vs.dxbc {} bytes, alpha.fs.dxbc {} bytes, WGSL {alpha_wgsl}", alpha_vs.len(), alpha_fs.len());
    drop(logged);

    // The layer blur: one vertex stage, and a fragment stage per `layer_blur::MODES` entry, in its order.
    let start = HLSL.lock().unwrap().len();
    rune::render::make_layer_blur(&gpu.device);
    let logged = HLSL.lock().unwrap();
    let fragments: Vec<&String> = logged[start..].iter().filter(|text| text.starts_with("Naga generated shader for \"fs\"")).collect();
    assert_eq!(fragments.len(), 5, "the layer blur makes five pipelines");
    let blur_vs = fxc(&stage(&logged[start..], "vs"), "vs", "vs_5_1");
    std::fs::write(format!("{root}/layer_blur.vs.dxbc"), &blur_vs).unwrap();
    for (i, text) in fragments.iter().enumerate() {
        let fs = fxc(text.split_once(":\n").unwrap().1, "fs", "ps_5_1");
        std::fs::write(format!("{root}/layer_blur.fs{i}.dxbc"), &fs).unwrap();
    }
    let blur_wgsl = blake3::hash(std::fs::read_to_string(format!("{root}/src/layer_blur.wgsl")).unwrap().replace("\r\n", "\n").as_bytes()).to_hex();
    println!("layer_blur.vs.dxbc {} bytes and 5 fragment variants, WGSL {blur_wgsl}", blur_vs.len());

    drop(logged);

    // The backdrop blur: its down and up passes (one vertex stage, `fs_down`, `fs_up`), then its
    // composite (`vs`, `fs`), made after a renderer whose own uber logs come first.
    let renderer = rune::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
    let start = HLSL.lock().unwrap().len();
    renderer.make_backdrop_blur(&gpu.device);
    let logged = HLSL.lock().unwrap();
    let named = |name: &str| logged[start..].iter().filter(|text| text.starts_with(&format!("Naga generated shader for \"{name}\""))).map(|text| text.split_once(":\n").unwrap().1.to_owned()).collect::<Vec<_>>();
    let (vertices, down, up, fragments) = (named("vs"), named("fs_down"), named("fs_up"), named("fs"));
    assert!(vertices.len() >= 2 && down.len() == 1 && up.len() == 1 && fragments.len() == 1, "the backdrop blur logged {} vs, {} fs_down, {} fs_up, {} fs", vertices.len(), down.len(), up.len(), fragments.len());
    let backdrop = [("backdrop_blur.vs", fxc(&vertices[0], "vs", "vs_5_1")), ("backdrop_blur.fs_down", fxc(&down[0], "fs_down", "ps_5_1")),
        ("backdrop_blur.fs_up", fxc(&up[0], "fs_up", "ps_5_1")), ("backdrop_composite.vs", fxc(vertices.last().unwrap(), "vs", "vs_5_1")),
        ("backdrop_composite.fs", fxc(&fragments[0], "fs", "ps_5_1"))];
    for (name, blob) in &backdrop {
        std::fs::write(format!("{root}/{name}.dxbc"), blob).unwrap();
    }
    let read = |path: &str| std::fs::read_to_string(format!("{root}/src/{path}")).unwrap().replace("\r\n", "\n");
    let blur_wgsl_hash = blake3::hash(read("backdrop_blur.wgsl").as_bytes()).to_hex();
    let composite_wgsl_hash = blake3::hash(format!("{}\n{}", read("clip.wgsl"), read("backdrop_composite.wgsl")).as_bytes()).to_hex();
    println!("backdrop blur: {} bytes in 5 stages, WGSL {blur_wgsl_hash} and {composite_wgsl_hash}", backdrop.iter().map(|(_, b)| b.len()).sum::<usize>());

    std::fs::write(format!("{root}/src/render/uber/pass_dxbc_hash.rs"), format!(
        "// Written by `cargo run -p rune --example regen_dxbc` on Windows.\n\
         pub(super) const ALPHA_DXBC_WGSL_BLAKE3: &str = \"{alpha_wgsl}\";\n\
         pub(super) const LAYER_BLUR_DXBC_WGSL_BLAKE3: &str = \"{blur_wgsl}\";\n\
         pub(super) const BACKDROP_BLUR_DXBC_WGSL_BLAKE3: &str = \"{blur_wgsl_hash}\";\n\
         pub(super) const BACKDROP_COMPOSITE_DXBC_WGSL_BLAKE3: &str = \"{composite_wgsl_hash}\";\n")).unwrap();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("regen_dxbc runs on Windows, where FXC is");
}

/// HLSL wgpu-hal logged at debug level.
#[cfg(target_os = "windows")]
static HLSL: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(target_os = "windows")]
struct Capture;

#[cfg(target_os = "windows")]
impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool { metadata.target().starts_with("wgpu_hal") }
    fn log(&self, record: &log::Record) {
        let text = record.args().to_string();
        if text.starts_with("Naga generated shader for") {
            HLSL.lock().unwrap().push(text);
        }
    }
    fn flush(&self) {}
}

#[cfg(target_os = "windows")]
#[link(name = "d3dcompiler")]
unsafe extern "system" {
    fn D3DCompile(source: *const u8, len: usize, name: *const i8, defines: *const u8, include: *mut u8,
        entry: *const i8, target: *const i8, flags1: u32, flags2: u32, code: *mut *mut Blob, errors: *mut *mut Blob) -> i32;
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct Blob { vtbl: *const BlobVtbl }

#[cfg(target_os = "windows")]
#[repr(C)]
struct BlobVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: unsafe extern "system" fn(*mut Blob) -> u32,
    pointer: unsafe extern "system" fn(*mut Blob) -> *const u8,
    size: unsafe extern "system" fn(*mut Blob) -> usize,
}

/// FXC as wgpu-hal calls it: strictness only, shader model 5.1.
#[cfg(target_os = "windows")]
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
