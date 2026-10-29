- **On Linux, Rune draws on the GPU the Wayland compositor uses.** Rune took the lowest-power
  adapter, so on a desktop with an integrated GPU beside the discrete card driving the display
  (Hyprland on NVIDIA, for one) every window and overlay showed as a black rectangle: the
  compositor couldn't show the integrated GPU's buffers. Rune now asks the compositor for its
  main device (`zwp_linux_dmabuf_v1` feedback) and takes the adapter on the same PCI device,
  falling back to the lowest-power adapter as before, which keeps laptops that drive their panel
  from the integrated GPU on it. `WGPU_ADAPTER_NAME` (part of the adapter's name) and
  `WGPU_POWER_PREF` (`low`, `high`, `none`) override the choice on every platform. A machine with
  more than one GPU prints the choice and its reason to stderr once; `RUNE_GPU_LOG=1` prints it
  anywhere.
