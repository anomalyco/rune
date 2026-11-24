//! Startup GPU suballocation policy for wgpu devices.
//!
//! Translates Rune's [`GpuMemoryPolicy`] into `wgpu::MemoryHints` for D3D12 (Windows) and Vulkan
//! (Linux) suballocators without exposing `wgpu` types in `rune`'s public API.

use std::ops::Range;
use std::sync::Mutex;

/// Startup GPU memory suballocation policy for wgpu devices.
///
/// Configure before the first window, overlay, or prewarm creates a GPU device via
/// `rune::App::gpu_memory_policy` or `rune::set_gpu_memory_policy`.
///
/// Maps directly to wgpu's `MemoryHints` on backends that suballocate device memory (D3D12 on
/// Windows and Vulkan on Linux). Rune's native Metal backend, wgpu's Metal backend, and browser
/// backends (WebGPU and WebGL2) ignore it.
///
/// The default is [`Self::MemoryUsage`], which favors smaller suballocation blocks over peak
/// throughput.
///
/// In [`Self::Manual`], `block_size` bounds the suballocator's block range in bytes, not the
/// maximum size of a texture or buffer: resources larger than the block range still succeed via
/// dedicated allocations.
///
/// # Example
///
/// ```
/// use rune_gpu::GpuMemoryPolicy;
///
/// let policy = GpuMemoryPolicy::Manual {
///     block_size: (4 * 1024 * 1024)..(8 * 1024 * 1024),
/// };
/// assert_eq!(GpuMemoryPolicy::default(), GpuMemoryPolicy::MemoryUsage);
/// assert_ne!(policy, GpuMemoryPolicy::default());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GpuMemoryPolicy {
    /// Favor lower reserved GPU memory (wgpu's `MemoryHints::MemoryUsage`, Rune's default).
    #[default]
    MemoryUsage,
    /// Favor throughput with larger suballocation blocks (wgpu's `MemoryHints::Performance`).
    Performance,
    /// Explicit suballocation block-size range in bytes (wgpu's `MemoryHints::Manual`).
    ///
    /// wgpu's D3D12 and Vulkan suballocators clamp device block sizes to 4 MiB..=256 MiB and use
    /// half that range (also clamped to 4 MiB..=256 MiB) for host-visible blocks. Passing equal
    /// bounds (for example `4 MiB..4 MiB`) requests fixed-size blocks; zero or inverted bounds
    /// (`end < start`) are clamped safely so they never panic.
    Manual {
        /// Suballocated device-memory block-size range in bytes (`min..max`).
        block_size: Range<u64>,
    },
}

/// Active process-wide startup policy; defaults to `MemoryUsage`.
static POLICY: Mutex<GpuMemoryPolicy> = Mutex::new(GpuMemoryPolicy::MemoryUsage);

/// Clamps `range` so its lower bound is at least 1 byte and `end >= start`, preventing empty
/// zero-byte or inverted bounds from reaching `wgpu` / `gpu-allocator`.
fn normalize_block_size(range: &Range<u64>) -> Range<u64> {
    let start = range.start.max(1);
    let end = range.end.max(start);
    start..end
}

fn normalize(policy: &GpuMemoryPolicy) -> GpuMemoryPolicy {
    match policy {
        GpuMemoryPolicy::MemoryUsage => GpuMemoryPolicy::MemoryUsage,
        GpuMemoryPolicy::Performance => GpuMemoryPolicy::Performance,
        GpuMemoryPolicy::Manual { block_size } => GpuMemoryPolicy::Manual {
            block_size: normalize_block_size(block_size),
        },
    }
}

/// Sets the process-wide startup GPU memory policy for devices created from now on.
///
/// Returns `true` when the effective policy changed, so a caller with a prewarmed or parked GPU
/// built under the previous policy can discard or restart it.
pub fn choose(policy: GpuMemoryPolicy) -> bool {
    let mut current = POLICY.lock().unwrap_or_else(|e| e.into_inner());
    let changed = normalize(&current) != normalize(&policy);
    *current = policy;
    changed
}

/// Returns the currently configured process-wide startup GPU memory policy.
pub fn current() -> GpuMemoryPolicy {
    POLICY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Converts `policy` to `wgpu::MemoryHints`, clamping degenerate `Manual` ranges safely.
pub fn to_wgpu_hints(policy: &GpuMemoryPolicy) -> wgpu::MemoryHints {
    match policy {
        GpuMemoryPolicy::MemoryUsage => wgpu::MemoryHints::MemoryUsage,
        GpuMemoryPolicy::Performance => wgpu::MemoryHints::Performance,
        GpuMemoryPolicy::Manual { block_size } => wgpu::MemoryHints::Manual {
            suballocated_device_memory_block_size: normalize_block_size(block_size),
        },
    }
}

/// Returns `wgpu::MemoryHints` for the currently configured process-wide startup policy.
pub fn wgpu_hints() -> wgpu::MemoryHints {
    to_wgpu_hints(&current())
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
