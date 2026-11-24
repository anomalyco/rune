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

/// Errors captured by pushing `Validation`, `OutOfMemory`, and `Internal` error scopes around
/// a synchronous `wgpu` operation and popping all three in reverse (LIFO) order.
#[derive(Debug)]
pub struct ScopeErrors<E = wgpu::Error> {
    pub memory: Option<E>,
    pub internal: Option<E>,
    pub validation: Option<E>,
}

/// Runs `op` inside `Validation`, `OutOfMemory`, and `Internal` error scopes on `device`,
/// unconditionally popping all three in strict reverse (LIFO) order on both native and `wasm32`
/// (`wgpu-core` / WebGL2).
///
/// Only the future resolver differs between targets (`pollster::block_on` on native, immediate
/// poll on `wasm32`); push and pop order are shared in one function so native tests cover the
/// exact order executed on WebGL2.
pub fn with_error_scopes<T>(device: &wgpu::Device, op: impl FnOnce() -> T) -> (T, ScopeErrors) {
    with_error_scopes_using(
        |filter| device.push_error_scope(filter),
        op,
        |guard| resolve_scope(guard.pop()),
    )
}

/// Generic core of [`with_error_scopes`]: pushes `Validation`, `OutOfMemory`, then `Internal`,
/// runs `op`, and unconditionally pops `Internal`, `OutOfMemory`, then `Validation` in reverse
/// order before returning.
pub(crate) fn with_error_scopes_using<G, E, T>(
    mut push: impl FnMut(wgpu::ErrorFilter) -> G,
    op: impl FnOnce() -> T,
    mut pop: impl FnMut(G) -> Option<E>,
) -> (T, ScopeErrors<E>) {
    let validation = push(wgpu::ErrorFilter::Validation);
    let memory = push(wgpu::ErrorFilter::OutOfMemory);
    let internal = push(wgpu::ErrorFilter::Internal);
    let out = op();
    // Pop strictly in reverse push order into locals so argument evaluation order or
    // short-circuiting combinators can never pop out of order or leave a scope open.
    let internal = pop(internal);
    let memory = pop(memory);
    let validation = pop(validation);
    (out, ScopeErrors { memory, internal, validation })
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_scope<F: std::future::Future<Output = Option<wgpu::Error>>>(fut: F) -> Option<wgpu::Error> {
    pollster::block_on(fut)
}

#[cfg(target_arch = "wasm32")]
fn resolve_scope<F: std::future::Future<Output = Option<wgpu::Error>>>(fut: F) -> Option<wgpu::Error> {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match std::pin::pin!(fut).poll(&mut cx) {
        std::task::Poll::Ready(err) => err,
        std::task::Poll::Pending => None,
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
