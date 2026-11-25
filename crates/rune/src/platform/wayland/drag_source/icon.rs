//! Wayland drag-icon surface layout, premultiplied ARGB8888 conversion, and `wl_shm` builder.
//!
//! Keeps pixel conversion, integer `buffer_scale` divisibility checks, and `wl_surface` v5+
//! `offset()` vs v1..v4 `attach(x, y)` placement math pure and cross-platform.

use crate::drag_out::DragImage;

/// Hard pixel dimension cap for a Wayland drag icon surface.
pub(crate) const MAX_ICON_DIM: u32 = 4096;

/// Hard decoded byte cap for a Wayland drag icon surface (64 MiB).
pub(crate) const MAX_ICON_BYTES: usize = 64 * 1024 * 1024;

/// How to apply the hotspot offset when attaching a buffer to a `wl_surface`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachPlacement {
    /// `wl_surface` v1..=4: pass `(dx, dy)` directly to `wl_surface.attach(buf, dx, dy)`.
    AttachOffset { dx: i32, dy: i32 },
    /// `wl_surface` v5+: pass `(0, 0)` to `wl_surface.attach` and `(dx, dy)` to `wl_surface.offset`
    /// (non-zero `attach` offsets are a fatal `invalid_offset` protocol error on v5+).
    SurfaceOffset { dx: i32, dy: i32 },
}

/// Prepared premultiplied ARGB8888 drag icon ready for upload to `wl_shm`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedIcon {
    pub width: u32,
    pub height: u32,
    pub argb8888: Vec<u8>,
    pub buffer_scale: i32,
    pub offset: (i32, i32),
}

/// Maps a logical `(dx, dy)` hotspot offset to `wl_surface.offset` on v5+ or
/// `wl_surface.attach(buf, dx, dy)` on v1..=4.
pub(crate) const fn attach_placement((dx, dy): (i32, i32), surface_version: u32) -> AttachPlacement {
    if surface_version >= 5 {
        AttachPlacement::SurfaceOffset { dx, dy }
    } else {
        AttachPlacement::AttachOffset { dx, dy }
    }
}

/// Integer `buffer_scale` supported by `width x height` at `window_scale`.
pub(crate) fn integer_buffer_scale(width: u32, height: u32, window_scale: f32) -> i32 {
    if !window_scale.is_finite() || window_scale < 1.5 {
        return 1;
    }
    // Only apply integer buffer_scale when window_scale is effectively an integer >= 2 and
    // divides both buffer dimensions evenly.
    let rounded = window_scale.round();
    if (window_scale - rounded).abs() > 0.01 {
        return 1;
    }
    let s = rounded.clamp(1.0, 8.0) as u32;
    if s > 1 && width % s == 0 && height % s == 0 {
        s as i32
    } else {
        1
    }
}

/// Converts straight RGBA8 pixels to native-endian premultiplied Wayland `ARGB8888`.
pub(crate) fn rgba_to_premultiplied_argb8888(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let a = u16::from(px[3]);
        let r = ((u16::from(px[0]) * a + 127) / 255) as u32;
        let g = ((u16::from(px[1]) * a + 127) / 255) as u32;
        let b = ((u16::from(px[2]) * a + 127) / 255) as u32;
        let word = (u32::from(px[3]) << 24) | (r << 16) | (g << 8) | b;
        out.extend_from_slice(&word.to_ne_bytes());
    }
    out
}

/// Decodes `image` and computes its premultiplied ARGB8888 buffer, scale, and hotspot offset.
pub(crate) fn prepare_icon(
    image: &DragImage,
    rect: [f32; 4],
    press: (f32, f32),
    window_scale: f32,
) -> Option<PreparedIcon> {
    let decoded = image::load_from_memory(image.png_data()).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 || width > MAX_ICON_DIM || height > MAX_ICON_DIM {
        return None;
    }
    let byte_len = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if byte_len > MAX_ICON_BYTES {
        return None;
    }
    let buffer_scale = integer_buffer_scale(width, height, window_scale);
    let (hx, hy) = match image.hotspot_point() {
        Some((x, y)) => (x.round() as i32, y.round() as i32),
        None => {
            let natural_points = (width as f32 / buffer_scale as f32, height as f32 / buffer_scale as f32);
            let frame = image.frame(rect, press, natural_points);
            ((press.0 - frame[0]).round() as i32, (press.1 - frame[1]).round() as i32)
        }
    };
    let argb8888 = rgba_to_premultiplied_argb8888(decoded.as_raw());
    Some(PreparedIcon { width, height, argb8888, buffer_scale, offset: (-hx, -hy) })
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod shm {
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

    use wayland_client::QueueHandle;
    use wayland_client::protocol::wl_buffer::WlBuffer;
    use wayland_client::protocol::wl_compositor::WlCompositor;
    use wayland_client::protocol::wl_shm::{Format, WlShm};
    use wayland_client::protocol::wl_shm_pool::WlShmPool;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{Dispatch, Proxy};

    use super::PreparedIcon;

    /// Live `wl_surface` + `wl_buffer` + `wl_shm_pool` owned by an active Wayland drag session.
    /// All three stay alive until the drag ends so the compositor can read the buffer.
    pub(crate) struct DragIconSurface {
        pub surface: WlSurface,
        buffer: WlBuffer,
        pool: WlShmPool,
    }

    impl Drop for DragIconSurface {
        fn drop(&mut self) {
            self.surface.destroy();
            self.buffer.destroy();
            self.pool.destroy();
        }
    }

    #[allow(unsafe_code)]
    fn create_shm_fd(bytes: &[u8]) -> Option<OwnedFd> {
        let len = i32::try_from(bytes.len()).ok()?;
        if len <= 0 {
            return None;
        }
        let name = c"rune-drag-icon";
        // SAFETY: `memfd_create` creates a fresh anonymous close-on-exec memory fd.
        let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if raw < 0 {
            return None;
        }
        // SAFETY: `raw` is a newly created descriptor owned by `fd`.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: extends anonymous memfd to `len` bytes.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return None;
        }
        // SAFETY: maps `len` bytes read/write for copying pixels, then immediately unmaps.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return None;
        }
        // SAFETY: `ptr` points to a valid `bytes.len()`-byte writable mapping.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
            libc::munmap(ptr, bytes.len());
        }
        Some(fd)
    }

    /// Uploads `icon` into a `wl_shm` buffer and commits a `wl_surface` ready for
    /// `wl_data_device.start_drag`.
    pub(crate) fn build_icon_surface<D>(
        compositor: &WlCompositor,
        shm: &WlShm,
        qh: &QueueHandle<D>,
        icon: &PreparedIcon,
    ) -> Option<DragIconSurface>
    where
        D: Dispatch<WlSurface, ()> + Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let fd = create_shm_fd(&icon.argb8888)?;
        let byte_len = i32::try_from(icon.argb8888.len()).ok()?;
        let width = i32::try_from(icon.width).ok()?;
        let height = i32::try_from(icon.height).ok()?;
        let stride = width.checked_mul(4)?;
        let pool = shm.create_pool(fd.as_fd(), byte_len, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, Format::Argb8888, qh, ());
        let surface = compositor.create_surface(qh, ());
        if icon.buffer_scale > 1 && surface.version() >= 3 {
            surface.set_buffer_scale(icon.buffer_scale);
        }
        match super::attach_placement(icon.offset, surface.version()) {
            super::AttachPlacement::SurfaceOffset { dx, dy } => {
                surface.attach(Some(&buffer), 0, 0);
                if dx != 0 || dy != 0 {
                    surface.offset(dx, dy);
                }
            }
            super::AttachPlacement::AttachOffset { dx, dy } => {
                surface.attach(Some(&buffer), dx, dy);
            }
        }
        if surface.version() >= 4 {
            surface.damage_buffer(0, 0, width, height);
        } else {
            surface.damage(0, 0, width, height);
        }
        surface.commit();
        Some(DragIconSurface { surface, buffer, pool })
    }
}
