//! Shared Windows D3D12/D3D11 interop primitives: lazy System32-only D3D11 device creation on
//! Rune's active D3D12 adapter, cross-API shared fences (`ID3D12Fence` <-> `ID3D11Fence`), and
//! cross-API shared 2D render-target textures (`ID3D12Resource` <-> `ID3D11Texture2D`).
//!
//! Used by both `rune-media` (Media Foundation hardware video decoding) and `rune` (Windows
//! synchronized drag resize). DLLs (`d3d11.dll`, `dxgi.dll`) load on demand from System32 only,
//! so launch pays zero import-table or DLL-load cost when neither feature runs.

#![allow(unsafe_code)]

use std::ffi::CStr;
use std::ptr;

use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    ID3D11Device, ID3D11Device1, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4,
    ID3D11Fence, ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_TYPE_DEFAULT, D3D12_MEMORY_POOL_UNKNOWN, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, D3D12_RESOURCE_STATE_COMMON,
    D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12_CPU_PAGE_PROPERTY_UNKNOWN, ID3D12Device, ID3D12Fence,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIFactory2, IDXGIFactory4};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{GUID, HRESULT, HSTRING, Interface, PCSTR};

type CreateDeviceFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    D3D_DRIVER_TYPE,
    HMODULE,
    u32,
    *const D3D_FEATURE_LEVEL,
    u32,
    u32,
    *mut *mut core::ffi::c_void,
    *mut D3D_FEATURE_LEVEL,
    *mut *mut core::ffi::c_void,
) -> HRESULT;

type CreateFactoryFn =
    unsafe extern "system" fn(*const GUID, *mut *mut core::ffi::c_void) -> HRESULT;

/// A D3D11 device created on the exact DXGI adapter of a D3D12 device, with the interfaces needed
/// for cross-API shared textures, shared fences, and HWND swap chains.
pub struct D3d11Device {
    pub device: ID3D11Device,
    pub device1: ID3D11Device1,
    pub device5: ID3D11Device5,
    pub context: ID3D11DeviceContext4,
    pub factory: IDXGIFactory4,
    pub factory2: IDXGIFactory2,
    pub luid: LUID,
}

/// A cross-API fence pair backed by one shared kernel fence object.
pub struct SharedFence {
    pub d3d12: ID3D12Fence,
    pub d3d11: ID3D11Fence,
}

/// A 2D render-target texture allocated on D3D12 with `D3D12_HEAP_FLAG_SHARED` and opened on
/// D3D11 via `ID3D11Device1::OpenSharedResource1`.
pub struct SharedRenderTarget {
    pub d3d12: ID3D12Resource,
    pub d3d11: ID3D11Texture2D,
    pub size: (u32, u32),
    pub format: DXGI_FORMAT,
}

/// Loads a Windows system DLL from System32 only, never from the application directory or CWD.
/// Rejects empty names or names containing path separators or drive prefixes.
pub fn load_system_dll(name: &str) -> Option<HMODULE> {
    if name.is_empty() || name.contains(['\\', '/', ':']) {
        return None;
    }
    // SAFETY: `HSTRING` is NUL-terminated and `LOAD_LIBRARY_SEARCH_SYSTEM32` restricts the search
    // path to System32; the returned handle stays loaded for the process's lifetime.
    unsafe { LoadLibraryExW(&HSTRING::from(name), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()
}

/// Looks up `name` in `module` and casts it to function pointer type `F`.
///
/// # Safety
/// `F` must match the exported symbol's ABI and signature.
unsafe fn symbol<F: Copy>(module: HMODULE, name: &CStr) -> Option<F> {
    // SAFETY: `module` is a live DLL handle and `name` is NUL-terminated.
    let f = unsafe { GetProcAddress(module, PCSTR(name.as_ptr().cast())) }?;
    debug_assert_eq!(size_of::<F>(), size_of_val(&f));
    // SAFETY: caller vouches for `F`.
    Some(unsafe { std::mem::transmute_copy(&f) })
}

/// Returns the raw `ID3D12Device` and its adapter `LUID` when `device` is backed by wgpu's
/// D3D12 backend.
pub fn d3d12_device_luid(device: &wgpu::Device) -> Option<(ID3D12Device, LUID)> {
    // SAFETY: reads only the raw D3D12 device handle and queries its immutable adapter LUID.
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }?;
    let raw = hal.raw_device().clone();
    // SAFETY: plain query on a live `ID3D12Device`.
    let luid = unsafe { raw.GetAdapterLuid() };
    Some((raw, luid))
}

/// Creates a D3D11 device on the DXGI adapter identified by `luid`.
///
/// `video_support` enables `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` (for Media Foundation DXVA
/// decoders); `multithread` enables `ID3D11Multithread::SetMultithreadProtected(true)`.
pub fn create_d3d11_device_for_luid(
    luid: LUID,
    video_support: bool,
    multithread: bool,
) -> Result<D3d11Device, String> {
    let d3d11 = load_system_dll("d3d11.dll").ok_or("d3d11.dll is missing")?;
    let dxgi = load_system_dll("dxgi.dll").ok_or("dxgi.dll is missing")?;
    // SAFETY: documented System32 entry points cast to their documented signatures; out pointers
    // receive owned COM references on success.
    unsafe {
        let create: CreateDeviceFn =
            symbol(d3d11, c"D3D11CreateDevice").ok_or("no D3D11CreateDevice")?;
        let create_factory: CreateFactoryFn =
            symbol(dxgi, c"CreateDXGIFactory1").ok_or("no CreateDXGIFactory1")?;
        let mut raw_factory = ptr::null_mut();
        create_factory(&IDXGIFactory4::IID, &mut raw_factory)
            .ok()
            .map_err(|e| format!("DXGI: {}", e.message()))?;
        let factory = IDXGIFactory4::from_raw(raw_factory);
        let factory2 = factory
            .cast::<IDXGIFactory2>()
            .map_err(|e| format!("IDXGIFactory2: {}", e.message()))?;
        let adapter: IDXGIAdapter = factory
            .EnumAdapterByLuid(luid)
            .map_err(|e| format!("Rune's GPU: {}", e.message()))?;
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let (mut raw_device, mut raw_context) = (ptr::null_mut(), ptr::null_mut());
        let mut flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT.0;
        if video_support {
            flags |= D3D11_CREATE_DEVICE_VIDEO_SUPPORT.0;
        }
        create(
            adapter.as_raw(),
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            flags,
            levels.as_ptr(),
            levels.len() as u32,
            D3D11_SDK_VERSION,
            &mut raw_device,
            ptr::null_mut(),
            &mut raw_context,
        )
        .ok()
        .map_err(|e| format!("no D3D11 device: {}", e.message()))?;
        let device = ID3D11Device::from_raw(raw_device);
        let context = ID3D11DeviceContext::from_raw(raw_context);
        if multithread
            && let Ok(mt) = device.cast::<ID3D11Multithread>()
        {
            let _prev = mt.SetMultithreadProtected(true);
        }
        let device1 = device
            .cast::<ID3D11Device1>()
            .map_err(|_| "D3D11 shared resources need Windows 8+")?;
        let device5 = device
            .cast::<ID3D11Device5>()
            .map_err(|_| "D3D11 fences need Windows 10 1703")?;
        let context = context
            .cast::<ID3D11DeviceContext4>()
            .map_err(|_| "D3D11 fences need Windows 10 1703")?;
        Ok(D3d11Device { device, device1, device5, context, factory, factory2, luid })
    }
}

/// Creates a shared fence on `d3d12` and opens it on `d3d11`, closing the transient NT handle.
pub fn create_shared_fence(
    d3d12: &ID3D12Device,
    d3d11: &ID3D11Device5,
) -> Result<SharedFence, String> {
    // SAFETY: creates a shared D3D12 fence, exports a transient NT handle, opens it on D3D11, and
    // closes the NT handle unconditionally.
    unsafe {
        let fence12: ID3D12Fence = d3d12
            .CreateFence(0, D3D12_FENCE_FLAG_SHARED)
            .map_err(|e| format!("fence: {}", e.message()))?;
        let handle = d3d12
            .CreateSharedHandle(&fence12, None, GENERIC_ALL.0, None)
            .map_err(|e| format!("share fence: {}", e.message()))?;
        let mut fence11: Option<ID3D11Fence> = None;
        let opened = d3d11.OpenSharedFence(handle, &mut fence11);
        CloseHandle(handle).ok();
        opened.map_err(|e| format!("open fence: {}", e.message()))?;
        Ok(SharedFence { d3d12: fence12, d3d11: fence11.ok_or("open fence")? })
    }
}

/// Creates a single-sample, single-mip 2D render-target texture on `d3d12` in
/// `D3D12_RESOURCE_STATE_COMMON` and opens it on `d3d11` through an NT shared handle.
pub fn create_shared_render_target(
    d3d12: &ID3D12Device,
    d3d11: &ID3D11Device1,
    size: (u32, u32),
    format: DXGI_FORMAT,
) -> Result<SharedRenderTarget, String> {
    let (width, height) = size;
    if width == 0 || height == 0 {
        return Err("shared render target size must be non-zero".into());
    }
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(width),
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
            | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    };
    // SAFETY: creates a committed shared 2D texture on `d3d12`, shares it via an NT handle, opens
    // it on `d3d11` via `OpenSharedResource1`, and closes the NT handle unconditionally.
    unsafe {
        let mut raw12: Option<ID3D12Resource> = None;
        d3d12
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_SHARED,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut raw12,
            )
            .map_err(|e| format!("shared render target: {}", e.message()))?;
        let resource12 = raw12.ok_or("shared render target")?;
        let handle = d3d12
            .CreateSharedHandle(&resource12, None, GENERIC_ALL.0, None)
            .map_err(|e| format!("share render target: {}", e.message()))?;
        let opened: windows::core::Result<ID3D11Texture2D> = d3d11.OpenSharedResource1(handle);
        CloseHandle(handle).ok();
        let texture11 = opened.map_err(|e| format!("open shared render target: {}", e.message()))?;
        Ok(SharedRenderTarget { d3d12: resource12, d3d11: texture11, size, format })
    }
}

#[cfg(test)]
#[path = "d3d11_interop_tests.rs"]
mod tests;
