//! What wgpu's `custom` backend costs per frame on top of the Metal calls it makes, offscreen
//! (research/34, phase 0). Run with `--profile perf --features metal-spike`.

fn main() {
    #[cfg(target_os = "macos")]
    rune::__metal_spike_dispatch_overhead();
}
