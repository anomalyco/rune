//! What a page logs of its GPU device's uncaptured errors. A frame that fails validation fails
//! the same way every frame after it, and a browser stops logging a device's errors after a few
//! hundred, so the cause scrolls away under copies of it. Rune logs the first error with the
//! adapter it happened on, each different error once after it, and stops after a few.

/// How many different errors are logged before the rest are muted.
const DIFFERENT: usize = 4;

/// What to do with an uncaptured error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// The device's first error: log it with the adapter's description.
    First,
    /// An error not seen before: log it.
    New,
    /// The error that reached the limit: say that the rest won't be logged.
    Mute,
    /// Seen before, or past the limit: log nothing.
    Quiet,
}

/// The errors a device has reported.
#[derive(Default)]
pub struct ErrorLog {
    seen: Vec<String>,
}

impl ErrorLog {
    pub fn note(&mut self, message: &str) -> Note {
        if self.seen.len() > DIFFERENT || self.seen.iter().any(|m| m == message) { return Note::Quiet; }
        self.seen.push(message.to_owned());
        match self.seen.len() {
            1 => Note::First,
            n if n > DIFFERENT => Note::Mute,
            _ => Note::New,
        }
    }
}

pub use rune_staging::health::{PollFailed, poll, removed};

/// `removed` for one device, cheap enough to ask every frame: `removed` costs about 200 ns a
/// call on D3D12 (`GetDeviceRemovedReason` is most of it), while this reads a fence of its own,
/// never signalled, which D3D12 sets to `u64::MAX` when it removes the device.
pub struct Removal {
    #[cfg(target_os = "windows")]
    fence: Option<RemovalFence>,
}

/// D3D12 fences are free-threaded.
#[cfg(target_os = "windows")]
struct RemovalFence(windows::Win32::Graphics::Direct3D12::ID3D12Fence);
#[cfg(target_os = "windows")]
// SAFETY: ID3D12Fence is free-threaded; it is only read.
#[allow(unsafe_code)]
unsafe impl Send for RemovalFence {}
#[cfg(target_os = "windows")]
// SAFETY: as for Send.
#[allow(unsafe_code)]
unsafe impl Sync for RemovalFence {}

impl Removal {
    /// Watches `device`; on other backends, and if the fence can't be made, it never reads removed.
    #[allow(unsafe_code)]
    pub fn watch(device: &wgpu::Device) -> Removal {
        #[cfg(target_os = "windows")]
        {
            // SAFETY: a fence made on the device's D3D12 interface, only ever read.
            let fence = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
                .and_then(|d| unsafe { d.raw_device().CreateFence(0, windows::Win32::Graphics::Direct3D12::D3D12_FENCE_FLAG_NONE) }.ok())
                .map(RemovalFence);
            Removal { fence }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = device;
            Removal {}
        }
    }

    #[allow(unsafe_code)]
    pub fn removed(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            // SAFETY: a plain read of a live fence.
            self.fence.as_ref().is_some_and(|f| unsafe { f.0.GetCompletedValue() } == u64::MAX)
        }
        #[cfg(not(target_os = "windows"))]
        false
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
