//! Offscreen render targets with CPU readback and PNG writing.

/// An offscreen render target with CPU readback.
pub struct Offscreen {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    readback: rune_staging::Readback,
    pub width: u32,
    pub height: u32,
}

/// Default offscreen target format (`Rgba8Unorm`), matching Rune's default sRGB-encoded blending
/// on `App` windows, standalone [`Runtime`](crate::Runtime), [`Renderer`](super::Renderer), and
/// [`SurfaceRenderer`](crate::native_surface::SurfaceRenderer).
///
/// Use [`OFFSCREEN_FORMAT_LINEAR`] when opting into linear-light blending (`set_srgb_blend(false)`).
///
/// ```
/// use rune::render::{OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR};
///
/// assert_eq!(OFFSCREEN_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
/// assert_eq!(OFFSCREEN_FORMAT_LINEAR, wgpu::TextureFormat::Rgba8UnormSrgb);
/// ```
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Linear-light offscreen target format (`Rgba8UnormSrgb`), where the GPU encodes linear
/// premultiplied fragments to sRGB on write.
///
/// Pair with [`Offscreen::with_format`], `Renderer::new(device, OFFSCREEN_FORMAT_LINEAR)`, and
/// `rt.set_srgb_blend(false)` when you explicitly want linear-light blending off screen.
///
/// ```no_run
/// use rune::render::{Gpu, OFFSCREEN_FORMAT_LINEAR, Offscreen, Renderer};
/// use rune::{Color, Env, Runtime, el};
///
/// let gpu = Gpu::headless();
/// let mut rt = Runtime::new((), |_: &(), _| el().w_full().h_full().fill(Color::WHITE), (64.0, 64.0), Env::new(true, 1.0));
/// rt.set_srgb_blend(false);
/// let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
/// let target = Offscreen::with_format(&gpu.device, 64, 64, OFFSCREEN_FORMAT_LINEAR);
/// rt.frame(0.0);
/// renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
/// let _rgba = target.read(&gpu.device);
/// ```
pub const OFFSCREEN_FORMAT_LINEAR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Offscreen {
    /// Creates an offscreen target in [`OFFSCREEN_FORMAT`] (`Rgba8Unorm`) for sRGB-encoded blending.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Offscreen {
        Self::with_format(device, width, height, OFFSCREEN_FORMAT)
    }

    /// Creates an offscreen target in `format` (for example [`OFFSCREEN_FORMAT_LINEAR`] for
    /// linear-light blending, or `wgpu::TextureFormat::Bgra8Unorm`).
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

    /// Texture format this offscreen target was created with.
    ///
    /// ```no_run
    /// use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen};
    ///
    /// let gpu = Gpu::headless();
    /// let target = Offscreen::new(&gpu.device, 64, 64);
    /// assert_eq!(target.format(), OFFSCREEN_FORMAT);
    /// ```
    pub fn format(&self) -> wgpu::TextureFormat {
        self.texture.format()
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

impl super::Renderer {
    /// Uploads `rt`'s images, native surfaces, glyph atlas, fade stops and instances, renders the
    /// frame into `target`, copies it into `target`'s readback buffer, and submits the command
    /// buffer on `queue`.
    ///
    /// Call [`Runtime::frame`](crate::Runtime::frame) before rendering and follow with
    /// [`Offscreen::read`] to read back tightly packed RGBA8 pixels. Create `self` and `target`
    /// with the same texture format: [`OFFSCREEN_FORMAT`] (`Rgba8Unorm`) for Rune's default
    /// sRGB-encoded blending (matching `App` windows), or [`OFFSCREEN_FORMAT_LINEAR`]
    /// (`Rgba8UnormSrgb`) together with `rt.set_srgb_blend(false)` and
    /// `renderer.set_srgb_blend(false)` for linear-light blending.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    /// use rune::{Color, Cx, El, Env, Runtime, el};
    ///
    /// fn view(_: &(), _: &mut Cx<()>) -> El {
    ///     el().w_full().h_full().fill(Color::WHITE)
    /// }
    ///
    /// let gpu = Gpu::headless();
    /// let (w, h) = (128, 64);
    /// let mut rt = Runtime::new((), view, (w as f32, h as f32), Env::new(true, 1.0));
    /// let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    /// let target = Offscreen::new(&gpu.device, w, h);
    ///
    /// rt.frame(0.0);
    /// renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    /// let rgba = target.read(&gpu.device);
    /// assert_eq!(rgba.len(), (w * h * 4) as usize);
    /// ```
    #[inline(always)]
    pub fn render_runtime_to_offscreen<S: 'static>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &Offscreen,
        rt: &mut crate::Runtime<S>,
    ) {
        self.render_runtime_core_to_offscreen(device, queue, target, rt.core_mut());
    }

    fn render_runtime_core_to_offscreen(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &Offscreen,
        rt: &mut crate::runtime::RuntimeCore,
    ) {
        let size = (target.width, target.height);
        self.upload_images(device, queue, &mut rt.images);
        self.prepare_surfaces(device, queue, &rt.surfaces, size);
        self.set_fade_stops(rt.fade_stops());
        self.prepare_ui(device, queue, &mut rt.text.atlas, &rt.instances, size);
        let mut encoder = device.create_command_encoder(&Default::default());
        self.draw(&mut encoder, &target.view, rt.clear_color());
        target.copy(&mut encoder);
        self.submit(queue, encoder);
    }
}

pub fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    #[cfg(any(not(target_arch = "wasm32"), feature = "png"))]
    {
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
    #[cfg(all(target_arch = "wasm32", not(feature = "png")))]
    {
        let _ = (path, width, height, rgba);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "write_png is unsupported on wasm32 without the `png` feature",
        ))
    }
}
