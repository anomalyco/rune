#![allow(unsafe_code)]

use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use super::{
    create_d3d11_device_for_luid, create_shared_fence, create_shared_render_target,
    d3d12_device_luid, load_system_dll,
};

#[test]
fn load_system_dll_rejects_paths_and_loads_d3d11() {
    assert!(load_system_dll("").is_none());
    assert!(load_system_dll("..\\d3d11.dll").is_none());
    assert!(load_system_dll("C:/Windows/System32/d3d11.dll").is_none());
    assert!(load_system_dll("d3d11.dll").is_some());
}

#[test]
fn shared_fence_and_render_target_round_trip_on_adapter() {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    let instance = wgpu::Instance::new(desc);
    let pick = match crate::pick::pick(&instance, None, false) {
        Ok(pick) => pick,
        Err(err) => {
            if crate::skip_test_without_gpu(&err) {
                return;
            }
            unreachable!();
        }
    };
    let (device, _queue) =
        pollster::block_on(pick.adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("d3d12 device");
    let (d3d12, luid) = d3d12_device_luid(&device).expect("d3d12 device and luid");
    let d3d11 = create_d3d11_device_for_luid(luid, false, false).expect("d3d11 device");
    assert_eq!(d3d11.luid.LowPart, luid.LowPart);
    assert_eq!(d3d11.luid.HighPart, luid.HighPart);

    let fence = create_shared_fence(&d3d12, &d3d11.device5).expect("shared fence");
    // SAFETY: CPU signal on D3D12 fence is visible on the opened D3D11 fence.
    unsafe {
        fence.d3d12.Signal(7).expect("signal d3d12 fence");
        assert_eq!(fence.d3d11.GetCompletedValue(), 7);
    }

    assert!(create_shared_render_target(&d3d12, &d3d11.device1, (0, 64), DXGI_FORMAT_B8G8R8A8_UNORM).is_err());
    let target = create_shared_render_target(
        &d3d12,
        &d3d11.device1,
        (64, 48),
        DXGI_FORMAT_B8G8R8A8_UNORM,
    )
    .expect("shared render target");
    assert_eq!(target.size, (64, 48));
    let mut d11_desc = Default::default();
    // SAFETY: plain query on a live D3D11 texture opened from D3D12.
    unsafe { target.d3d11.GetDesc(&mut d11_desc) };
    assert_eq!((d11_desc.Width, d11_desc.Height), (64, 48));
    assert_eq!(d11_desc.Format, DXGI_FORMAT_B8G8R8A8_UNORM);
}
