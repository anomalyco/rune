use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{DXC_DLL, from_exe, from_exe_dir, pick};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("rune-gpu-dx12-compiler-{tag}-{pid}-{id}"));
        std::fs::remove_dir_all(&path).ok();
        if let Err(err) = std::fs::create_dir_all(&path) {
            panic!("create temp test dir: {err}");
        }
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn dx12_compiler_is_dxc_only_by_full_path_beside_the_executable() {
    let root = TempDir::new("beside-exe");
    let app_dir = root.path().join("app");
    let other_dir = root.path().join("other");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::create_dir_all(&other_dir).unwrap();

    let app_exe = app_dir.join("app.exe");
    // A dxcompiler.dll in an unrelated directory (such as one on PATH) must never be picked up.
    std::fs::write(other_dir.join(DXC_DLL), b"other").unwrap();
    // Shipping dxil.dll alone without dxcompiler.dll must still fall back to FXC.
    std::fs::write(app_dir.join("dxil.dll"), b"dxil").unwrap();

    assert!(
        matches!(from_exe(Some(&app_exe)), wgpu::Dx12Compiler::Fxc),
        "absent dxcompiler.dll beside the executable must fall back to FXC",
    );

    // A directory named dxcompiler.dll is not a DLL file and must fall back to FXC.
    let dir_named_dll = app_dir.join(DXC_DLL);
    std::fs::create_dir(&dir_named_dll).unwrap();
    assert!(
        matches!(from_exe(Some(&app_exe)), wgpu::Dx12Compiler::Fxc),
        "directory named dxcompiler.dll must fall back to FXC",
    );
    std::fs::remove_dir(&dir_named_dll).unwrap();

    // Remove dxil.dll to confirm modern DXC (1.8.2502+) does not require dxil.dll alongside it.
    std::fs::remove_file(app_dir.join("dxil.dll")).unwrap();

    // Placing dxcompiler.dll directly beside the executable selects DynamicDxc by exact full path.
    let expected = app_dir.join(DXC_DLL);
    std::fs::write(&expected, b"dxc").unwrap();
    match from_exe(Some(&app_exe)) {
        wgpu::Dx12Compiler::DynamicDxc { dxc_path } => {
            let resolved = Path::new(&dxc_path);
            assert_eq!(resolved, expected.as_path());
            assert!(resolved.is_absolute(), "dxc_path must be absolute: {dxc_path}");
            assert_ne!(dxc_path, DXC_DLL, "dxc_path must never be a bare DLL name");
            assert_eq!(resolved.parent(), Some(app_dir.as_path()));
            assert_eq!(resolved.file_name(), Some(OsStr::new(DXC_DLL)));
        }
        other => panic!("expected DynamicDxc with full path, got {other:?}"),
    }

    // Removing dxcompiler.dll restores clean FXC fallback.
    std::fs::remove_file(&expected).unwrap();
    assert!(
        matches!(from_exe(Some(&app_exe)), wgpu::Dx12Compiler::Fxc),
        "removing dxcompiler.dll beside the executable must fall back to FXC",
    );
}

#[test]
fn dx12_compiler_rejects_none_and_relative_paths() {
    assert!(matches!(from_exe(None), wgpu::Dx12Compiler::Fxc));
    assert!(matches!(from_exe(Some(Path::new("app.exe"))), wgpu::Dx12Compiler::Fxc));
    assert!(matches!(from_exe(Some(Path::new("sub/app.exe"))), wgpu::Dx12Compiler::Fxc));
    assert!(matches!(from_exe_dir(Path::new("")), wgpu::Dx12Compiler::Fxc));
    assert!(matches!(from_exe_dir(Path::new(".")), wgpu::Dx12Compiler::Fxc));

    #[cfg(target_os = "windows")]
    {
        // Drive-relative and root-relative paths are not absolute on Windows and must be rejected.
        assert!(matches!(from_exe(Some(Path::new(r"X:app.exe"))), wgpu::Dx12Compiler::Fxc));
        assert!(matches!(from_exe(Some(Path::new(r"\dir\app.exe"))), wgpu::Dx12Compiler::Fxc));
        assert!(matches!(from_exe_dir(Path::new(r"X:relative")), wgpu::Dx12Compiler::Fxc));
        assert!(matches!(from_exe_dir(Path::new(r"\root-relative")), wgpu::Dx12Compiler::Fxc));
    }
}

#[test]
fn pick_is_cached_and_never_returns_bare_dll_name() {
    let first = pick();
    let second = pick();
    match (&first, &second) {
        (wgpu::Dx12Compiler::Fxc, wgpu::Dx12Compiler::Fxc) => {}
        (
            wgpu::Dx12Compiler::DynamicDxc { dxc_path: a },
            wgpu::Dx12Compiler::DynamicDxc { dxc_path: b },
        ) => {
            assert_eq!(a, b);
            assert!(Path::new(a).is_absolute(), "picked dxc_path must be absolute: {a}");
            assert_ne!(a, DXC_DLL, "picked dxc_path must never be a bare DLL name");
        }
        other => panic!("pick() must return a consistent, safe compiler choice: {other:?}"),
    }

    // Unless a test environment placed dxcompiler.dll beside the test binary, Windows defaults
    // cleanly to FXC.
    let exe_has_dxc = cfg!(target_os = "windows")
        && std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|dir| dir.join(DXC_DLL).is_file()))
            .unwrap_or(false);
    if !exe_has_dxc {
        assert!(
            matches!(first, wgpu::Dx12Compiler::Fxc),
            "expected FXC when no dxcompiler.dll sits beside the test binary, got {first:?}",
        );
    }
}
