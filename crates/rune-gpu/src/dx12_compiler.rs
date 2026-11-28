//! Windows DX12 shader compiler selection: dynamic DXC by explicit full path when the app ships
//! `dxcompiler.dll` beside its executable, and FXC (`d3dcompiler_47.dll`) otherwise.
//!
//! Passing a bare DLL name lets Windows' DLL search order (including `PATH` and the current
//! working directory) load an unrelated app's incompatible `dxcompiler.dll` and crash the first
//! shader compile. Resolving only an absolute path inside the running executable's directory
//! prevents `PATH` lookups, leaves apps that do not ship DXC on FXC unchanged, and probes the
//! filesystem at most once per process during GPU initialization (never in the frame loop).

/// File name of the redistributable DirectX Shader Compiler DLL looked up beside the executable.
pub const DXC_DLL: &str = "dxcompiler.dll";

/// Picks the DX12 shader compiler once for the current process.
///
/// On Windows, returns `wgpu::Dx12Compiler::DynamicDxc` with the explicit full path to
/// `dxcompiler.dll` when that file sits directly in the running executable's directory, and
/// `wgpu::Dx12Compiler::Fxc` otherwise. The filesystem check is cached per process via
/// `OnceLock` so GPU re-creation never re-stats disk and the frame loop performs no file I/O.
///
/// Outside Windows, always returns `wgpu::Dx12Compiler::Fxc` with no filesystem access.
pub fn pick() -> wgpu::Dx12Compiler {
    #[cfg(target_os = "windows")]
    {
        static COMPILER: std::sync::OnceLock<wgpu::Dx12Compiler> = std::sync::OnceLock::new();
        COMPILER
            .get_or_init(|| from_exe(std::env::current_exe().ok().as_deref()))
            .clone()
    }
    #[cfg(not(target_os = "windows"))]
    {
        wgpu::Dx12Compiler::Fxc
    }
}

/// Resolves the DX12 shader compiler for a given executable path without consulting `PATH` or the
/// current working directory.
///
/// Returns `wgpu::Dx12Compiler::DynamicDxc` only when `exe` has an absolute parent directory
/// containing a regular file named `dxcompiler.dll` whose full path is valid UTF-8. In every
/// other case (missing `exe`, relative path, missing DLL, or directory named `dxcompiler.dll`)
/// returns `wgpu::Dx12Compiler::Fxc`.
#[cfg(not(target_arch = "wasm32"))]
pub fn from_exe(exe: Option<&std::path::Path>) -> wgpu::Dx12Compiler {
    let Some(exe_dir) = exe.and_then(std::path::Path::parent) else {
        return wgpu::Dx12Compiler::Fxc;
    };
    from_exe_dir(exe_dir)
}

/// Resolves the DX12 shader compiler for a given executable directory.
///
/// Rejects any non-absolute directory so `wgpu-hal` is never handed a bare DLL name or relative
/// path that Windows' loader could satisfy from `PATH` or the current working directory.
#[cfg(not(target_arch = "wasm32"))]
pub fn from_exe_dir(exe_dir: &std::path::Path) -> wgpu::Dx12Compiler {
    if !exe_dir.is_absolute() {
        return wgpu::Dx12Compiler::Fxc;
    }
    let candidate = exe_dir.join(DXC_DLL);
    if !candidate.is_absolute() || !candidate.is_file() {
        return wgpu::Dx12Compiler::Fxc;
    }
    match candidate.to_str() {
        Some(dxc_path) => wgpu::Dx12Compiler::DynamicDxc {
            dxc_path: dxc_path.to_owned(),
        },
        None => wgpu::Dx12Compiler::Fxc,
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "dx12_compiler_tests.rs"]
mod tests;
