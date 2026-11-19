//! Offscreen render targets with CPU readback and PNG writing.

/// An offscreen sRGB target with CPU readback.
pub struct Offscreen {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    readback: rune_staging::Readback,
    pub width: u32,
    pub height: u32,
}

pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Offscreen {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Offscreen {
        Self::with_format(device, width, height, OFFSCREEN_FORMAT)
    }

    pub fn with_format(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> Offscreen {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let readback = rune_staging::Readback::new(device, "readback", width, height, 4);
        Offscreen { texture, view, readback, width, height }
    }

    pub fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        self.readback.copy_from(encoder, &self.texture);
    }

    /// Underlying offscreen texture, for readback rings (`tooling::record`).
    pub(crate) fn texture(&self) -> &wgpu::Texture { &self.texture }

    /// Tightly packed sRGB RGBA8 after `copy` was submitted.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(&self, device: &wgpu::Device) -> Vec<u8> {
        // A callback never panics: one that did while a failed wait unwinds (wgpu aborts the map
        // then) would abort the process. A failed map shows as `take_mapped`'s panic.
        self.readback.buffer().slice(..).map_async(wgpu::MapMode::Read, |_| {});
        rune_gpu::health::poll(device, wgpu::PollType::wait_indefinitely()).expect("poll");
        self.readback.take_mapped()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn read(&self, _: &wgpu::Device) -> Vec<u8> {
        panic!("blocking GPU readback is unsupported on web")
    }
}

pub fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(rgba).map_err(std::io::Error::other)?;
    Ok(())
}
