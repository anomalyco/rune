//! The floor under `hello-floor`: the same 480 x 320 pt window with no framework, stopped at each
//! step of a Direct3D start, to read what the API and the GPU's driver hold on their own
//! (Windows only).
//!
//!   hello-floor-bare win32              a window, filled by GDI
//!   hello-floor-bare d3d12 device       ... and a Direct3D 12 device on the adapter
//!   hello-floor-bare d3d12 queue        ... and a command queue
//!   hello-floor-bare d3d12 swapchain    ... and a two-buffer flip swap chain
//!   hello-floor-bare d3d12 frame        ... one cleared and presented frame (the default step)
//!   hello-floor-bare d3d11 device | swapchain | frame      the same through Direct3D 11
//!
//! `HELLO_FLOOR_ADAPTER=<part of a name>` picks the adapter (as `WGPU_ADAPTER_NAME` does for
//! Rune); without it, DXGI's first. The window then only runs its message loop: WM_CLOSE ends it.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("hello-floor-bare measures Direct3D: Windows only");
}

#[cfg(windows)]
fn main() {
    bare::run();
}

// A measuring program: a step that fails stops it at once, with the call's own error.
#[cfg(windows)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod bare {
    use std::cell::RefCell;

    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::HiDpi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{Interface, w};

    const BG: [f32; 4] = [0x1e as f32 / 255.0, 0x1e as f32 / 255.0, 0x1e as f32 / 255.0, 1.0];

    /// How far the start goes.
    #[derive(Clone, Copy, PartialEq, PartialOrd)]
    enum Step {
        Device,
        Queue,
        SwapChain,
        Frame,
    }

    /// What the process keeps alive until it ends.
    enum Gpu {
        None,
        D3d11Device { _device: ID3D11Device, _ctx: ID3D11DeviceContext },
        D3d11 { device: ID3D11Device, ctx: ID3D11DeviceContext, swap: IDXGISwapChain1, draws: bool },
        D3d12Device { _device: ID3D12Device },
        D3d12Queue { _device: ID3D12Device, _queue: ID3D12CommandQueue },
        D3d12Swap { _device: ID3D12Device, _queue: ID3D12CommandQueue, _swap: IDXGISwapChain3 },
        D3d12 { device: ID3D12Device, queue: ID3D12CommandQueue, alloc: ID3D12CommandAllocator, list: ID3D12GraphicsCommandList, swap: IDXGISwapChain3, heap: ID3D12DescriptorHeap, fence: ID3D12Fence, value: u64 },
    }

    thread_local! {
        static GPU: RefCell<Gpu> = const { RefCell::new(Gpu::None) };
    }

    /// Clears and presents one frame (after a resize: at the new size), at the `frame` step.
    fn draw(resize: Option<(u32, u32)>) {
        GPU.with_borrow_mut(|g| {
            // SAFETY: every interface below is alive in `g`, on the thread that made it; the
            // D3D12 path waits for the last frame before it reuses the allocator or resizes.
            unsafe {
                match g {
                    Gpu::D3d11 { device, ctx, swap, draws: true } => {
                        if let Some((w, h)) = resize {
                            ctx.ClearState();
                            swap.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)).ok();
                        }
                        let back: ID3D11Texture2D = swap.GetBuffer(0).unwrap();
                        let mut rtv = None;
                        device.CreateRenderTargetView(&back, None, Some(&mut rtv)).unwrap();
                        ctx.ClearRenderTargetView(rtv.as_ref().unwrap(), &BG);
                        swap.Present(0, DXGI_PRESENT(0)).ok().ok();
                    }
                    Gpu::D3d12 { device, queue, alloc, list, swap, heap, fence, value } => {
                        if fence.GetCompletedValue() < *value {
                            let e = CreateEventW(None, false, false, None).unwrap();
                            fence.SetEventOnCompletion(*value, e).unwrap();
                            WaitForSingleObject(e, INFINITE);
                            CloseHandle(e).ok();
                        }
                        if let Some((w, h)) = resize {
                            swap.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)).ok();
                        }
                        let i = swap.GetCurrentBackBufferIndex();
                        let back: ID3D12Resource = swap.GetBuffer(i).unwrap();
                        let rtv = heap.GetCPUDescriptorHandleForHeapStart();
                        device.CreateRenderTargetView(&back, None, rtv);
                        alloc.Reset().unwrap();
                        list.Reset(&*alloc, None).unwrap();
                        let barrier = |before, after| D3D12_RESOURCE_BARRIER {
                            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                                Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER { pResource: std::mem::transmute_copy(&back), Subresource: 0, StateBefore: before, StateAfter: after }),
                            },
                            ..Default::default()
                        };
                        list.ResourceBarrier(&[barrier(D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
                        list.ClearRenderTargetView(rtv, &BG, None);
                        list.ResourceBarrier(&[barrier(D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT)]);
                        list.Close().unwrap();
                        queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
                        swap.Present(0, DXGI_PRESENT(0)).ok().ok();
                        *value += 1;
                        queue.Signal(&*fence, *value).unwrap();
                    }
                    _ => {}
                }
            }
        });
    }

    unsafe extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: the window's own messages, on its thread; the paint struct is this call's.
        unsafe {
            match m {
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(h, &mut ps);
                    // Whatever presents nothing shows the class's fill.
                    if GPU.with_borrow(|g| !matches!(g, Gpu::D3d12 { .. } | Gpu::D3d11 { draws: true, .. })) {
                        FillRect(dc, &ps.rcPaint, CreateSolidBrush(COLORREF(0x001e1e1e)));
                    }
                    EndPaint(h, &ps).ok().ok();
                    LRESULT(0)
                }
                WM_SIZE => {
                    let (w, hh) = ((lp.0 & 0xffff) as u32, ((lp.0 >> 16) & 0xffff) as u32);
                    if w > 0 && hh > 0 {
                        draw(Some((w, hh)));
                    }
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(h, m, wp, lp),
            }
        }
    }

    #[allow(clippy::disallowed_methods)]
    pub(super) fn run() {
        let mut args = std::env::args().skip(1);
        let api = args.next().unwrap_or_else(|| "win32".into());
        let step = match args.next().as_deref() {
            Some("device") => Step::Device,
            Some("queue") => Step::Queue,
            Some("swapchain") => Step::SwapChain,
            _ => Step::Frame,
        };
        let want = std::env::var("HELLO_FLOOR_ADAPTER").ok().map(|s| s.to_lowercase());
        // SAFETY: plain Win32 and Direct3D calls on the process's first thread; every pointer
        // passed is to a local that outlives its call, and the interfaces are kept in `GPU`.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).ok();
            let instance = GetModuleHandleW(None).unwrap();
            let class = w!("hello-floor-bare");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).unwrap(), hbrBackground: CreateSolidBrush(COLORREF(0x001e1e1e)), ..Default::default() };
            RegisterClassW(&wc);
            let scale = GetDpiForSystem() as f32 / 96.0;
            let (cw, ch) = ((480.0 * scale) as i32, (320.0 * scale) as i32);
            let style = WS_OVERLAPPEDWINDOW;
            let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
            AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE(0), GetDpiForSystem()).ok();
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("Hello floor (bare)"), style, CW_USEDEFAULT, CW_USEDEFAULT, r.right - r.left, r.bottom - r.top, None, None, Some(instance.into()), None).unwrap();
            let on = windows::core::BOOL(1);
            windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const on).cast(), 4).ok();
            let factory = || -> IDXGIFactory2 { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap() };
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: cw as u32,
                Height: ch as u32,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                ..Default::default()
            };
            let adapter = || -> IDXGIAdapter1 {
                let factory = factory();
                (0..)
                    .map_while(|i| factory.EnumAdapters1(i).ok())
                    .find(|a| want.as_ref().is_none_or(|w| a.GetDesc1().is_ok_and(|d| String::from_utf16_lossy(&d.Description).to_lowercase().contains(w.as_str()))))
                    .expect("no adapter with that name")
            };
            match api.as_str() {
                "d3d11" => {
                    let (mut device, mut ctx) = (None, None);
                    D3D11CreateDevice(&adapter(), D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx)).unwrap();
                    let (device, ctx): (ID3D11Device, ID3D11DeviceContext) = (device.unwrap(), ctx.unwrap());
                    if step <= Step::Queue {
                        GPU.set(Gpu::D3d11Device { _device: device, _ctx: ctx });
                    } else {
                        let swap = factory().CreateSwapChainForHwnd(&device, hwnd, &desc, None, None).unwrap();
                        GPU.set(Gpu::D3d11 { device, ctx, swap, draws: step == Step::Frame });
                    }
                }
                "d3d12" => {
                    let mut device: Option<ID3D12Device> = None;
                    D3D12CreateDevice(&adapter(), D3D_FEATURE_LEVEL_11_0, &mut device).unwrap();
                    let device = device.unwrap();
                    if step == Step::Device {
                        GPU.set(Gpu::D3d12Device { _device: device });
                    } else {
                        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
                        if step == Step::Queue {
                            GPU.set(Gpu::D3d12Queue { _device: device, _queue: queue });
                            return show(hwnd);
                        }
                        let swap: IDXGISwapChain3 = factory().CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None).unwrap().cast().unwrap();
                        if step == Step::SwapChain {
                            GPU.set(Gpu::D3d12Swap { _device: device, _queue: queue, _swap: swap });
                        } else {
                            let heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() }).unwrap();
                            let alloc: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
                            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None).unwrap();
                            list.Close().unwrap();
                            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
                            GPU.set(Gpu::D3d12 { device, queue, alloc, list, swap, heap, fence, value: 0 });
                        }
                    }
                }
                _ => {}
            }
            show(hwnd);
        }
    }

    /// Draws the first frame (at the `frame` step), shows the window and runs its messages.
    fn show(hwnd: HWND) {
        draw(None);
        // SAFETY: the window made by `run`, on its thread; `msg` is this loop's.
        unsafe {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE).ok().ok();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                TranslateMessage(&msg).ok().ok();
                DispatchMessageW(&msg);
            }
        }
    }
}
