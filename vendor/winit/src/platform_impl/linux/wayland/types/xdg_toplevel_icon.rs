//! Handling of xdg toplevel icons, the window's own icon (backport of rust-windowing/winit#4288).

use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_shm::Format;
use sctk::reexports::client::{delegate_dispatch, Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::xdg::toplevel_icon::v1::client::xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1;
use sctk::reexports::protocols::xdg::toplevel_icon::v1::client::xdg_toplevel_icon_v1::XdgToplevelIconV1;
use sctk::shm::slot::{Buffer, CreateBufferError, SlotPool};

use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::PlatformIcon;

#[derive(Debug)]
pub struct XdgToplevelIconManagerState {
    xdg_toplevel_icon_manager: XdgToplevelIconManagerV1,
}

impl XdgToplevelIconManagerState {
    pub fn bind(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        let xdg_toplevel_icon_manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { xdg_toplevel_icon_manager })
    }

    pub fn global(&self) -> &XdgToplevelIconManagerV1 {
        &self.xdg_toplevel_icon_manager
    }
}

/// The window's icon in an shm buffer, kept alive while the toplevel shows it.
#[derive(Debug)]
pub struct ToplevelIcon {
    buffer: Buffer,
}

impl ToplevelIcon {
    pub(crate) fn new(icon: &PlatformIcon, pool: &mut SlotPool) -> Result<Self, CreateBufferError> {
        let (width, height) = (icon.width as i32, icon.height as i32);
        let (buffer, canvas) = pool.create_buffer(width, height, 4 * width, Format::Argb8888)?;
        argb_premultiplied(&icon.rgba, canvas);
        Ok(Self { buffer })
    }

    pub fn add_buffer(&self, xdg_toplevel_icon: &XdgToplevelIconV1) {
        xdg_toplevel_icon.add_buffer(self.buffer.wl_buffer(), 1);
    }
}

/// Writes straight RGBA pixels as premultiplied little-endian ARGB8888.
fn argb_premultiplied(rgba: &[u8], canvas: &mut [u8]) {
    for (canvas_chunk, rgba) in canvas.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let alpha = rgba[3] as f32 / 255.;
        let r = (rgba[0] as f32 * alpha) as u32;
        let g = (rgba[1] as f32 * alpha) as u32;
        let b = (rgba[2] as f32 * alpha) as u32;
        let color = ((rgba[3] as u32) << 24) + (r << 16) + (g << 8) + b;
        let array: &mut [u8; 4] = canvas_chunk.try_into().unwrap();
        *array = color.to_le_bytes();
    }
}

impl Dispatch<XdgToplevelIconManagerV1, GlobalData, WinitState> for XdgToplevelIconManagerState {
    fn event(
        _state: &mut WinitState,
        _proxy: &XdgToplevelIconManagerV1,
        _event: <XdgToplevelIconManagerV1 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        // Preferred icon sizes: one buffer is enough, the compositor scales it.
    }
}

impl Dispatch<XdgToplevelIconV1, GlobalData, WinitState> for XdgToplevelIconManagerState {
    fn event(
        _state: &mut WinitState,
        _proxy: &XdgToplevelIconV1,
        _event: <XdgToplevelIconV1 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        // No events.
    }
}

delegate_dispatch!(WinitState: [XdgToplevelIconManagerV1: GlobalData] => XdgToplevelIconManagerState);
delegate_dispatch!(WinitState: [XdgToplevelIconV1: GlobalData] => XdgToplevelIconManagerState);
