//! Non-generic growable GPU buffers and 2D texture readback staging buffers.

/// A GPU buffer holding fixed-stride items that grows to the next power of two when `ensure`
/// sees a count past its current capacity.
///
/// On the steady path `ensure` is a single comparison and allocates nothing. When it returns
/// `true`, the underlying [`wgpu::Buffer`] was replaced and callers must rebind or rewrite.
pub struct GrowBuffer {
    buffer: wgpu::Buffer,
    label: &'static str,
    usage: wgpu::BufferUsages,
    stride: usize,
    min_bytes: u64,
    capacity: usize,
}

impl GrowBuffer {
    /// Allocates a buffer for `initial_capacity` items of `stride` bytes, floored to `min_bytes`.
    pub fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        stride: usize,
        initial_capacity: usize,
        min_bytes: u64,
    ) -> Self {
        let buffer = Self::alloc(device, label, usage, stride, initial_capacity, min_bytes);
        Self { buffer, label, usage, stride, min_bytes, capacity: initial_capacity }
    }

    /// Ensures room for at least `count` items. Returns `true` if a new buffer was allocated
    /// (capacity grows to `count.next_power_of_two()`).
    #[inline]
    pub fn ensure(&mut self, device: &wgpu::Device, count: usize) -> bool {
        if count <= self.capacity {
            return false;
        }
        self.grow(device, count);
        true
    }

    #[cold]
    fn grow(&mut self, device: &wgpu::Device, count: usize) {
        self.capacity = count.next_power_of_two();
        self.buffer = Self::alloc(device, self.label, self.usage, self.stride, self.capacity, self.min_bytes);
    }

    fn alloc(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        stride: usize,
        capacity: usize,
        min_bytes: u64,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: ((capacity * stride) as u64).max(min_bytes),
            usage,
            mapped_at_creation: false,
        })
    }

    #[inline]
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn as_entire_binding(&self) -> wgpu::BindingResource<'_> {
        self.buffer.as_entire_binding()
    }

    #[inline]
    pub fn slice<S: std::ops::RangeBounds<wgpu::BufferAddress>>(&self, bounds: S) -> wgpu::BufferSlice<'_> {
        self.buffer.slice(bounds)
    }
}

/// Row stride in bytes for a texture copy of `width` pixels at `bytes_per_pixel` bytes each,
/// rounded up to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
#[inline]
pub const fn padded_row_bytes(width: u32, bytes_per_pixel: u32) -> u32 {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    (width * bytes_per_pixel).div_ceil(align) * align
}

/// Copies `height` tightly packed rows of `row_bytes` bytes each out of a buffer whose rows are
/// `padded_row` bytes apart.
pub fn unpad_rows(data: &[u8], row_bytes: usize, height: u32, padded_row: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for y in 0..height as usize {
        let start = y * padded_row as usize;
        out.extend_from_slice(&data[start..start + row_bytes]);
    }
    out
}

/// A reusable `COPY_DST | MAP_READ` buffer sized to read back a 2D texture.
pub struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    bytes_per_pixel: u32,
    padded_row: u32,
}

impl Readback {
    pub fn new(device: &wgpu::Device, label: &'static str, width: u32, height: u32, bytes_per_pixel: u32) -> Self {
        let padded_row = padded_row_bytes(width, bytes_per_pixel);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { buffer, width, height, bytes_per_pixel, padded_row }
    }

    #[inline]
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub fn padded_row(&self) -> u32 {
        self.padded_row
    }

    #[inline]
    pub fn row_bytes(&self) -> usize {
        (self.width * self.bytes_per_pixel) as usize
    }

    /// Records a copy of `texture`'s mip 0 from `(0, 0, 0)` into this readback buffer.
    pub fn copy_from(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        self.copy_from_origin(encoder, texture, wgpu::Origin3d::ZERO);
    }

    /// Records a copy of `texture`'s mip 0 starting at `origin` into this readback buffer.
    pub fn copy_from_origin(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, origin: wgpu::Origin3d) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    /// Unpads pixels out of an already-mapped buffer and unmaps it, returning `None` if the
    /// buffer is not mapped.
    pub fn try_take_mapped(&self) -> Option<Vec<u8>> {
        let out = {
            let data = self.buffer.slice(..).get_mapped_range().ok()?;
            unpad_rows(&data, self.row_bytes(), self.height, self.padded_row)
        };
        self.buffer.unmap();
        Some(out)
    }

    /// Unpads pixels out of an already-mapped buffer and unmaps it.
    pub fn take_mapped(&self) -> Vec<u8> {
        self.try_take_mapped().expect("mapped range")
    }

    /// Blocks until `copy_from` finishes, maps the buffer, unpads rows, and unmaps.
    /// Returns `None` if polling or mapping fails.
    pub fn try_read(&self, device: &wgpu::Device) -> Option<Vec<u8>> {
        // A callback never panics: one that did while a failed wait unwinds (wgpu aborts the map
        // then) would abort the process. A failed map shows as `try_take_mapped`'s `None`.
        self.buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        wait(device).then_some(())?;
        self.try_take_mapped()
    }

    /// Blocks until `copy_from` finishes, maps the buffer, unpads rows, and unmaps.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(&self, device: &wgpu::Device) -> Vec<u8> {
        self.buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        assert!(wait(device), "poll: the GPU device is lost");
        self.take_mapped()
    }
}

/// Waits for the device's work; false on a lost or removed device (`crate::health::poll`).
fn wait(device: &wgpu::Device) -> bool {
    crate::health::poll(device, wgpu::PollType::wait_indefinitely()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_row_bytes_aligns_to_wgpu_copy_alignment() {
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        assert_eq!(padded_row_bytes(0, 4), 0);
        assert_eq!(padded_row_bytes(1, 4), align);
        assert_eq!(padded_row_bytes(64, 4), 256);
        assert_eq!(padded_row_bytes(65, 4), 512);
        assert_eq!(padded_row_bytes(37, 4), align);
        assert_eq!(padded_row_bytes(2048, 1), 2048);
    }

    #[test]
    fn unpad_rows_strips_trailing_padding() {
        let data = [1, 2, 3, 4, 9, 9, 9, 9, 5, 6, 7, 8, 9, 9, 9, 9];
        assert_eq!(unpad_rows(&data, 4, 2, 8), [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(unpad_rows(&data[..8], 4, 1, 8), [1, 2, 3, 4]);
        assert!(unpad_rows(&[], 4, 0, 256).is_empty());
    }
}
