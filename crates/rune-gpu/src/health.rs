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

/// Whether `device`'s GPU was removed (a reset after a timeout, a driver update, a GPU
/// unplugged). D3D12 says so before wgpu does: Rune's wgpu-hal keeps a removed device's work
/// pending, never finished (`vendor/wgpu-hal`, `Device::reached`), and wgpu hears of the
/// removal only at the device's next submission. False on other backends, whose loss reaches
/// wgpu's lost callback.
pub fn removed(device: &wgpu::Device) -> bool {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: the device's D3D12 interface, asked whether it was removed.
        unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.is_some_and(|d| unsafe { d.raw_device().GetDeviceRemovedReason() }.is_err())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = device;
        false
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
