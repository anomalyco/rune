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
unsafe impl Send for RemovalFence {}
#[cfg(target_os = "windows")]
unsafe impl Sync for RemovalFence {}

impl Removal {
    /// Watches `device`; on other backends, and if the fence can't be made, it never reads removed.
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

/// Why `poll` gave no status.
#[derive(Debug)]
pub enum PollFailed {
    /// The device is gone: nothing it was given will finish.
    Lost,
    /// The wait timed out, or named a submission the queue never had.
    Wait(wgpu::PollError),
}

impl std::fmt::Display for PollFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PollFailed::Lost => f.write_str("the GPU device was lost"),
            PollFailed::Wait(error) => write!(f, "{error}"),
        }
    }
}

/// `Device::poll`, every Rune wait on the GPU goes through: a lost or removed device is an
/// error, never a wait that can't end and never a panic. wgpu 30 panics in `poll` once a fence
/// read fails ("Error in Device::poll: Parent device is lost"): on Vulkan that is how a lost
/// device shows. Until wgpu returns that as an error, the panic is caught here, and only that
/// one (wgpu holds no lock when it panics there; a callback's panic during the poll goes on).
/// On D3D12 the removal is asked first, so it never reaches wgpu's panic.
pub fn poll(device: &wgpu::Device, kind: wgpu::PollType) -> Result<wgpu::PollStatus, PollFailed> {
    if removed(device) {
        return Err(PollFailed::Lost);
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.poll(kind))) {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(_)) if removed(device) => Err(PollFailed::Lost),
        Ok(Err(error)) => Err(PollFailed::Wait(error)),
        Err(panic) => {
            let message = panic.downcast_ref::<String>().map(String::as_str).or_else(|| panic.downcast_ref::<&str>().copied()).unwrap_or("");
            if message.starts_with("Error in Device::poll") {
                return Err(PollFailed::Lost);
            }
            std::panic::resume_unwind(panic)
        }
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
