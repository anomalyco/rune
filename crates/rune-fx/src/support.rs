//! Whether a device can run rune-fx. Every effect reads its instances from a storage buffer in the
//! vertex and fragment stages and its per-layer tables from a second one in the fragment stage
//! (`shaders/common.wgsl`), so WebGL2 and downlevel adapters without vertex storage can't draw
//! effects. Check before `FxRenderer::new`, which otherwise fails wgpu validation.

use std::fmt;

/// Storage buffers the fragment stage binds (instances and per-layer tables).
pub(crate) const FRAGMENT_STORAGE_BUFFERS: u32 = 2;

/// Why a device can't run rune-fx.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxUnsupported {
    /// The adapter can't read storage buffers in vertex shaders (`DownlevelFlags::VERTEX_STORAGE`),
    /// as on WebGL2.
    VertexStorage,
    /// The adapter can't read storage buffers in fragment shaders (`DownlevelFlags::FRAGMENT_STORAGE`).
    FragmentStorage,
    /// The device allows fewer storage buffers per shader stage than rune-fx binds.
    StorageBuffers { needed: u32, allowed: u32 },
}

impl fmt::Display for FxUnsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FxUnsupported::VertexStorage => {
                f.write_str("rune-fx needs storage buffers in vertex shaders (DownlevelFlags::VERTEX_STORAGE), which this adapter (e.g. WebGL2) lacks")
            }
            FxUnsupported::FragmentStorage => {
                f.write_str("rune-fx needs storage buffers in fragment shaders (DownlevelFlags::FRAGMENT_STORAGE), which this adapter lacks")
            }
            FxUnsupported::StorageBuffers { needed, allowed } => write!(
                f,
                "rune-fx binds {needed} storage buffers per shader stage, but the device allows {allowed} (max_storage_buffers_per_shader_stage)"
            ),
        }
    }
}

impl std::error::Error for FxUnsupported {}

/// Checks an adapter's downlevel capabilities and a device's limits against what rune-fx binds.
/// `FxRenderer::supported` calls this with a live adapter and device.
pub fn fx_support(downlevel: &wgpu::DownlevelCapabilities, limits: &wgpu::Limits) -> Result<(), FxUnsupported> {
    if !downlevel.flags.contains(wgpu::DownlevelFlags::VERTEX_STORAGE) {
        return Err(FxUnsupported::VertexStorage);
    }
    if !downlevel.flags.contains(wgpu::DownlevelFlags::FRAGMENT_STORAGE) {
        return Err(FxUnsupported::FragmentStorage);
    }
    let allowed = limits.max_storage_buffers_per_shader_stage;
    if allowed < FRAGMENT_STORAGE_BUFFERS {
        return Err(FxUnsupported::StorageBuffers { needed: FRAGMENT_STORAGE_BUFFERS, allowed });
    }
    Ok(())
}
