#![allow(unsafe_code)]

use std::path::PathBuf;

use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;

use super::load_dll;

struct TempDll {
    path: PathBuf,
    loaded: windows_sys::Win32::Foundation::HMODULE,
}

impl Drop for TempDll {
    fn drop(&mut self) {
        if !self.loaded.is_null() {
            // SAFETY: module handle returned by `load_dll` in this test.
            unsafe { FreeLibrary(self.loaded); }
        }
        std::fs::remove_file(&self.path).ok();
    }
}

fn module_path(module: windows_sys::Win32::Foundation::HMODULE) -> PathBuf {
    let mut buf = [0u16; 260];
    // SAFETY: `module` is a loaded HMODULE and `buf` holds up to 260 UTF-16 code units.
    let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    assert!(len > 0 && len < buf.len());
    PathBuf::from(String::from_utf16_lossy(&buf[..len]))
}

#[test]
fn system_dll_loader_refuses_app_directory_hijack_and_loads_from_system32() {
    let dcomp = load_dll("dcomp.dll");
    assert!(!dcomp.is_null(), "dcomp.dll should load from System32");
    let dcomp_path = module_path(dcomp);
    let system32 = dcomp_path.parent().expect("dcomp parent dir");
    assert!(
        system32.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("system32")),
        "expected dcomp.dll from System32, got {dcomp_path:?}"
    );
    // SAFETY: releases the reference acquired above.
    unsafe { FreeLibrary(dcomp); }

    let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let probe_name = format!("rune_system32_probe_{}.dll", std::process::id());
    let probe_path = exe_dir.join(&probe_name);
    // Copy a small harmless System32 DLL beside the test executable under a unique name.
    std::fs::copy(system32.join("version.dll"), &probe_path).expect("copy probe dll");
    let mut guard = TempDll { path: probe_path, loaded: std::ptr::null_mut() };

    let loaded = load_dll(&probe_name);
    guard.loaded = loaded;
    assert!(loaded.is_null(), "system DLL loader must not load a DLL from the application directory");
    assert!(load_dll("..\\dcomp.dll").is_null(), "relative paths are rejected");
    assert!(load_dll(&dcomp_path.to_string_lossy()).is_null(), "full paths are rejected");
}
