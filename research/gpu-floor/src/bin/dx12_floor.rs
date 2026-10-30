//! research/30's HUD frame on raw D3D12 with the fewest calls the API allows: one command list,
//! two transition barriers (PRESENT -> RENDER_TARGET -> PRESENT, which D3D12 requires for a
//! swapchain buffer), one `ExecuteCommandLists`, one `Present`, one `Signal`. Views are made once.
//!
//! `WRITES=n` writes a 256-byte uniform `n` times a frame. `UPLOAD=mapped` (default) copies into a
//! persistently mapped upload-heap ring with no API call. `UPLOAD=copy` also records a
//! `CopyBufferRegion` per write into a default-heap buffer and one COPY_DEST -> constant-buffer
//! barrier; the buffer decays back to COMMON when the list finishes, so it needs no second one.

#[path = "../count.rs"]
mod count;
#[path = "../harness.rs"]
mod harness;

#[cfg(not(windows))]
fn main() {
    eprintln!("dx12_floor runs on Windows (or under Wine with vkd3d)");
}

#[cfg(windows)]
fn main() {
    harness::run("dx12_floor", |window| Box::new(unsafe { imp::Floor::new(window) }));
}

#[cfg(windows)]
mod imp {
    use std::mem::ManuallyDrop;
    use std::sync::Arc;

    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
    use windows::core::Interface;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use crate::harness::{Draw, env};

    const IN_FLIGHT: usize = 2;
    const BUFFERS: usize = 3;
    const SLOT: u64 = 256;

    pub struct Floor {
        _window: Arc<Window>,
        queue: ID3D12CommandQueue,
        swapchain: IDXGISwapChain3,
        targets: Vec<(ID3D12Resource, D3D12_CPU_DESCRIPTOR_HANDLE)>,
        _heap: ID3D12DescriptorHeap,
        allocators: [ID3D12CommandAllocator; IN_FLIGHT],
        list: ID3D12GraphicsCommandList,
        fence: ID3D12Fence,
        event: HANDLE,
        values: [u64; IN_FLIGHT],
        next: u64,
        mapped: *mut u8,
        staging: ID3D12Resource,
        uniforms: ID3D12Resource,
        writes: usize,
        copy: bool,
        frame: usize,
    }

    unsafe fn buffer(device: &ID3D12Device, size: u64, heap: D3D12_HEAP_TYPE, state: D3D12_RESOURCE_STATES) -> ID3D12Resource {
        let props = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: size,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut resource = None;
        unsafe { device.CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut resource).unwrap() };
        resource.unwrap()
    }

    fn transition(resource: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
        D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    // Borrowed for the call, not AddRef'd.
                    pResource: unsafe { std::mem::transmute_copy(resource) },
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: before,
                    StateAfter: after,
                }),
            },
        }
    }

    impl Floor {
        pub unsafe fn new(window: Arc<Window>) -> Floor {
            unsafe {
                let RawWindowHandle::Win32(handle) = window.window_handle().unwrap().as_raw() else { panic!("not Win32") };
                let hwnd = HWND(handle.hwnd.get() as *mut _);
                let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
                let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0).unwrap();
                let mut device: Option<ID3D12Device> = None;
                D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap();
                let device = device.unwrap();
                let queue: ID3D12CommandQueue = device
                    .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })
                    .unwrap();
                let desc = DXGI_SWAP_CHAIN_DESC1 {
                    Width: 360,
                    Height: 80,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: BUFFERS as u32,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    ..Default::default()
                };
                let swapchain: IDXGISwapChain3 = factory.CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None).unwrap().cast().unwrap();
                let heap: ID3D12DescriptorHeap = device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                        NumDescriptors: BUFFERS as u32,
                        ..Default::default()
                    })
                    .unwrap();
                let start = heap.GetCPUDescriptorHandleForHeapStart();
                let step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
                let view = D3D12_RENDER_TARGET_VIEW_DESC {
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                    ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                };
                let targets = (0..BUFFERS)
                    .map(|i| {
                        let resource: ID3D12Resource = swapchain.GetBuffer(i as u32).unwrap();
                        let handle = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + i * step };
                        device.CreateRenderTargetView(&resource, Some(&view), handle);
                        (resource, handle)
                    })
                    .collect();
                let allocators = [(); IN_FLIGHT].map(|_| device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap());
                let list: ID3D12GraphicsCommandList =
                    device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocators[0], None).unwrap();
                list.Close().unwrap();
                let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
                let event = CreateEventW(None, false, false, None).unwrap();

                let writes = env("WRITES", 0usize);
                let ring = SLOT * writes.max(1) as u64 * IN_FLIGHT as u64;
                let staging = buffer(&device, ring, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
                let mut mapped = std::ptr::null_mut();
                staging.Map(0, None, Some(&mut mapped)).unwrap();
                let uniforms = buffer(&device, SLOT, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COMMON);
                let copy = std::env::var("UPLOAD").as_deref() == Ok("copy");
                Floor {
                    _window: window,
                    queue,
                    swapchain,
                    targets,
                    _heap: heap,
                    allocators,
                    list,
                    fence,
                    event,
                    values: [0; IN_FLIGHT],
                    next: 0,
                    mapped: mapped.cast(),
                    staging,
                    uniforms,
                    writes,
                    copy,
                    frame: 0,
                }
            }
        }
    }

    impl Draw for Floor {
        fn frame(&mut self) {
            let f = self.frame % IN_FLIGHT;
            self.frame += 1;
            unsafe {
                if self.fence.GetCompletedValue() < self.values[f] {
                    self.fence.SetEventOnCompletion(self.values[f], self.event).unwrap();
                    WaitForSingleObject(self.event, INFINITE);
                }
                self.allocators[f].Reset().unwrap();
                self.list.Reset(&self.allocators[f], None).unwrap();
                let (target, rtv) = &self.targets[self.swapchain.GetCurrentBackBufferIndex() as usize];
                let phase = (self.frame % 120) as f32 / 120.0;
                let data = [phase.to_bits(); 64];
                for w in 0..self.writes {
                    let offset = (f * self.writes + w) as u64 * SLOT;
                    std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), self.mapped.add(offset as usize), SLOT as usize);
                    if self.copy {
                        self.list.CopyBufferRegion(&self.uniforms, 0, &self.staging, offset, SLOT);
                    }
                }
                let mut barriers = Vec::with_capacity(2);
                if self.copy && self.writes > 0 {
                    barriers.push(transition(&self.uniforms, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER));
                }
                barriers.push(transition(target, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET));
                self.list.ResourceBarrier(&barriers);
                self.list.ClearRenderTargetView(*rtv, &[0.0, 0.0, phase * 0.4, 0.4], None);
                self.list.ResourceBarrier(&[transition(target, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT)]);
                self.list.Close().unwrap();
                self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
                self.swapchain.Present(1, DXGI_PRESENT(0)).ok().unwrap();
                self.next += 1;
                self.queue.Signal(&self.fence, self.next).unwrap();
                self.values[f] = self.next;
            }
        }
    }
}
