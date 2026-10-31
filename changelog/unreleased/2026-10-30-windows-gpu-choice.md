- **Windows takes the GPU Windows chooses for the app, on DX12.** Rune took the lowest-power
  adapter, so on a desktop with an integrated GPU and a discrete card driving the monitor it drew
  on the integrated one and every frame crossed to the card (a 240 Hz display's resize steps took
  about 34 ms, against 3 ms on the card). Rune now takes the card DXGI lists first, which is the
  one set for the app in Settings > Display > Graphics, else the system default; a laptop whose
  panel hangs off its integrated GPU keeps it. Whatever picks the card, Windows takes it on DX12
  before Vulkan, so translucent windows composite through DirectComposition on every
  GPU and resizes keep their swapchain. `WGPU_BACKEND` (`dx12`, `vulkan`, comma-separated)
  now limits the backends on desktop; `WGPU_ADAPTER_NAME` and `WGPU_POWER_PREF` still
  choose the card, and `RUNE_GPU_LOG=1` prints the choice and, on Windows, how long after the
  process started the device was ready. The card order comes from DXGI directly (`windows`
  0.62, which wgpu-hal already builds), so the choice adds no launch time. With a hardware
  card and no `WGPU_BACKEND`, Windows loads DX12 alone, which saves loading Vulkan's driver
  at launch (about 145 ms on a two-GPU desktop); `WGPU_BACKEND=vulkan` still runs on Vulkan.
