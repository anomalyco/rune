//! Which uber shader a renderer draws with, as a state machine ([`Uber`]).
//!
//! `Classic` is shader.wgsl alone, the shader every renderer drew before glyphs could draw from
//! their outlines: its WGSL, layout and precompiled libraries are unchanged, so a Mac that keeps
//! the default text path draws exactly what it drew before. `Windfoil` is shader.wgsl with
//! outline.wgsl, whose glyphs can also draw from the outline store (binding 4). Renderers start
//! with Windfoil except on macOS (and with GPU springs, which keep Classic); a Classic renderer
//! builds Windfoil the first time text asks for outlines. A Windfoil that can't be built leaves
//! the renderer Classic for good (`Unavailable`), and text falls back to its other rasterisers.

use super::instance::shader_consts;
use super::*;
use crate::machine::Effects;

const _: () = {
    assert!(shader_consts::PINNED_KIND < 0.0);
    assert!(!shader_consts::OUTLINE_SUBPIXEL);
};

/// Windfoil's WGSL: shader.wgsl with outline.wgsl's glyph branch, joined when Rune builds
/// (`build.rs`, render/uber_wgsl.rs).
pub(crate) fn windfoil_wgsl() -> &'static str {
    super::uber_wgsl::WINDFOIL
}

/// Windfoil's fragment entry, which the subpixel variant renames for its own entry to call.
const FS_ENTRY: &str = shader_consts::FS_ENTRY_DECL;
/// Mean subpixel coverage floor below which dual-source text emits zero (`DUAL_ENTRY`).
pub(super) const LCD_MEAN_EPSILON: f32 = 1.0e-5;
pub(super) const DUAL_ENTRY: &str = r#"
struct DualOut {
    @location(0) @blend_src(0) color: vec4<f32>,
    @location(0) @blend_src(1) blend: vec4<f32>,
}

// Windfoil's fragment, with each colour channel blended by its own coverage where the glyph
// drawn set one (`lcd_cov`): subpixel text. Everything else blends as premultiplied alpha.
@fragment
fn fs(i: Out) -> DualOut {
    let c = fs_premul(i);
    if (lcd_cov.w == 0.0) {
        return DualOut(c, vec4<f32>(c.a));
    }
    let e = i.params.w;
    let mean = pow((lcd_cov.x + lcd_cov.y + lcd_cov.z) / 3.0, e);
    if (mean < 1.0e-5) {
        return DualOut(vec4<f32>(0.0), vec4<f32>(0.0));
    }
    let k = pow(lcd_cov.xyz, vec3<f32>(e)) / mean;
    return DualOut(vec4<f32>(c.rgb * k, c.a), vec4<f32>(c.a * k, c.a));
}
"#;

/// The kinds the frame pass has a pipeline of its own for (`KIND_RECT`, `KIND_SHADOW`, `KIND_GLYPH`):
/// what most of a frame's fragments draw.
pub(crate) const PINNED: [u32; 3] = [Kind::Rect.as_u32(), Kind::Shadow.as_u32(), Kind::Glyph.as_u32()];

/// shader.wgsl's marker for the kind a fragment draws: -1, every kind.
const PINNED_OFF: &str = shader_consts::PINNED_KIND_OFF_DECL;

/// Windfoil's WGSL with its fragment entry pinned to `kind` (shader.wgsl `fragment_kind`): the
/// compiler drops every other kind's code. The vertex stage is unchanged.
pub(crate) fn pinned_wgsl(kind: u32) -> Option<String> {
    let windfoil = windfoil_wgsl();
    (windfoil.matches(PINNED_OFF).count() == 1).then(|| windfoil.replacen(PINNED_OFF, &format!("const PINNED_KIND: f32 = {kind}.0;"), 1))
}

/// outline.wgsl's switch for subpixel outline glyphs, off but in the subpixel variant.
const SUBPIXEL_OFF: &str = shader_consts::OUTLINE_SUBPIXEL_OFF_DECL;

/// The subpixel variant's WGSL: Windfoil's, with subpixel outline coverage and its fragment
/// entry blending by a second output. Only this variant pays for the three coverages.
pub(super) fn dual_wgsl() -> Option<String> {
    let windfoil = windfoil_wgsl();
    let marked = windfoil.matches(FS_ENTRY).count() == 1 && windfoil.matches(SUBPIXEL_OFF).count() == 1;
    marked.then(|| {
        let body = windfoil.replacen(FS_ENTRY, "fn fs_premul(i: Out) -> vec4<f32> {", 1)
            .replacen(SUBPIXEL_OFF, "const OUTLINE_SUBPIXEL: bool = true;", 1);
        format!("enable dual_source_blending;\n{body}\n{DUAL_ENTRY}")
    })
}

/// Windfoil's subpixel pipeline for the window, built the first frame text draws subpixel masks
/// (`FontSmoothing::WindfoilRgb`, `ClearType`) on a Windfoil renderer whose device blends by a
/// second output. Layers keep the plain pipeline, so text in them draws grayscale.
pub(super) enum Lcd {
    Unbuilt,
    /// No shipped bytecode takes it here, so its WGSL compiles on a thread of its own (FXC takes
    /// about a minute for it on a Windows laptop) and subpixel text draws grayscale meanwhile.
    Building(std::sync::mpsc::Receiver<Result<wgpu::RenderPipeline, String>>),
    Built(wgpu::RenderPipeline),
    /// It can't be built here: subpixel text draws grayscale from now on.
    Refused,
}

/// Classic's bindings and the outline store's.
pub(crate) const fn windfoil_bind_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    let [a, b, c, d] = metallib::uber_bind_entries();
    let outlines = wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Uint,
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    [a, b, c, d, wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: outlines, count: None }]
}

/// Windfoil's pipeline ABI, as `metallib::uber_abi_hash` hashes Classic's.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) const fn windfoil_abi_hash() -> u64 {
    metallib::abi_hash_of(&windfoil_bind_entries())
}

/// Where a Windfoil shader came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The bytecode Rune ships for it (DXBC on Windows, a metallib on macOS).
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    Precompiled,
    /// Its WGSL, compiled at run time.
    Compiled,
}

pub(super) enum Uber {
    /// shader.wgsl alone, and no outline store.
    Classic,
    /// shader.wgsl and outline.wgsl, with the outline store's texture and the atlas fill pass.
    Windfoil { source: Source, outlines: wgpu::Texture, fill: fill::FillPass },
    /// Windfoil was asked for and couldn't be built: Classic from now on.
    Unavailable,
}

pub(super) enum Event {
    /// Text drew glyphs from outlines.
    OutlinesWanted,
    /// Windfoil's pipeline was built.
    Built { source: Source, outlines: wgpu::Texture, fill: fill::FillPass },
    /// Windfoil's pipeline couldn't be built, and why.
    Refused(String),
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Build Windfoil's pipeline, and send `Built` or `Refused`.
    Build,
    /// Say why text falls back from outlines.
    Fallback(String),
}

impl Uber {
    pub(super) fn step(&mut self, event: Event) -> Effects<Effect> {
        match (&*self, event) {
            (Uber::Classic, Event::OutlinesWanted) => Effects::one(Effect::Build),
            (Uber::Classic, Event::Built { source, outlines, fill }) => {
                *self = Uber::Windfoil { source, outlines, fill };
                Effects::none()
            }
            (Uber::Classic, Event::Refused(why)) => {
                *self = Uber::Unavailable;
                Effects::one(Effect::Fallback(why))
            }
            _ => Effects::none(),
        }
    }

    /// The outline store's texture, which Windfoil's bind groups hold.
    pub(super) fn outlines(&self) -> Option<&wgpu::Texture> {
        match self {
            Uber::Windfoil { outlines, .. } => Some(outlines),
            Uber::Classic | Uber::Unavailable => None,
        }
    }

    /// What text can expect of outlines from this renderer.
    pub(super) fn support(&self) -> crate::text::outline::Support {
        match self {
            Uber::Classic => crate::text::outline::Support::Untried,
            Uber::Windfoil { .. } => crate::text::outline::Support::Drawn,
            Uber::Unavailable => crate::text::outline::Support::Unavailable,
        }
    }
}

/// What building Windfoil needs from its renderer.
pub(super) struct BuildContext<'a> {
    pub format: wgpu::TextureFormat,
    pub vertex_entry: &'static str,
    pub globals: &'a wgpu::Buffer,
    pub stand_in: &'a wgpu::Texture,
}

/// A built Windfoil: its layouts, shader and frame pipeline.
pub(super) struct Built {
    pub bind_layout: wgpu::BindGroupLayout,
    pub layout: wgpu::PipelineLayout,
    pub shader: dxbc::Shaders,
    pub pipeline: wgpu::RenderPipeline,
    pub source: Source,
}

impl Renderer {
    /// Which uber shader draws, for diagnostics: `classic`, `windfoil (precompiled)`,
    /// `windfoil (compiled)` or `unavailable` (Windfoil couldn't be built).
    #[doc(hidden)]
    pub fn uber_shader(&self) -> &'static str {
        match self.uber {
            Uber::Classic => "classic",
            Uber::Windfoil { source: Source::Precompiled, .. } => "windfoil (precompiled)",
            Uber::Windfoil { source: Source::Compiled, .. } => "windfoil (compiled)",
            Uber::Unavailable => "unavailable",
        }
    }

    /// Tells `atlas` what this renderer does with outlines (`text::outline::Support`) before it
    /// first draws, so the first frame's glyph masks are queued for the GPU fill rather than
    /// filled on the CPU. `prepare` tells it again every frame.
    pub fn attach_text(&self, atlas: &mut Atlas) {
        atlas.outlines.set_support(self.uber.support());
    }

    /// Steps the shader machine `state` on `event`, carrying out each effect (building Windfoil
    /// feeds `Built` or `Refused` back in) until none is left; with the Windfoil pipeline it
    /// built on the way, if it built one.
    fn run(device: &wgpu::Device, cx: &BuildContext<'_>, state: &mut Uber, event: Event) -> Option<Built> {
        let mut built = None;
        let mut queue: Vec<Effect> = state.step(event).into_iter().collect();
        while let Some(effect) = queue.pop() {
            match effect {
                Effect::Build => {
                    let next = match Self::build_windfoil(device, cx.format, cx.vertex_entry) {
                        Ok(b) => {
                            let outlines = Self::outline_texture(device, outlines::MIN_ROWS);
                            let fill = fill::FillPass::new(device, &b.layout, &b.shader, fill::FillBindings {
                                layout: &b.bind_layout, globals: cx.globals, outlines: &outlines, stand_in: cx.stand_in });
                            let event = Event::Built { source: b.source, outlines, fill };
                            built = Some(b);
                            event
                        }
                        Err(why) => Event::Refused(why),
                    };
                    queue.extend(state.step(next));
                }
                Effect::Fallback(why) => eprintln!("rune: glyphs can't draw from outlines here ({why}); text draws from the atlas alone"),
            }
        }
        built
    }
    /// A new renderer's shader machine, asked for Windfoil at once.
    pub(super) fn start_windfoil(device: &wgpu::Device, cx: &BuildContext<'_>) -> (Uber, Option<Built>) {
        let mut state = Uber::Classic;
        let built = Self::run(device, cx, &mut state, Event::OutlinesWanted);
        (state, built)
    }

    /// Steps this renderer's shader machine on `event`, taking any Windfoil pipeline it builds;
    /// whether it took one (the caller then makes its atlas pages ones the GPU can fill).
    pub(super) fn select(&mut self, device: &wgpu::Device, event: Event) -> bool {
        let cx = BuildContext { format: self.format, vertex_entry: self.vertex_entry, globals: &self.globals, stand_in: &self.stand_in };
        if let Some(built) = Self::run(device, &cx, &mut self.uber, event) {
            self.adopt(device, built);
            self.rebind_all(device);
            return true;
        }
        false
    }

    /// Windfoil's layouts, shader and pipeline for `format`, from the shipped bytecode where it
    /// matches, else from WGSL; an error where neither builds.
    pub(super) fn build_windfoil(device: &wgpu::Device, format: wgpu::TextureFormat, vertex_entry: &'static str) -> Result<Built, String> {
        if vertex_entry != "vs" {
            return Err("GPU springs draw the classic shader".into());
        }
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &windfoil_bind_entries() });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bind_layout)], immediate_size: 0 });
        let make_pipeline = |shader: &dxbc::Shaders| Self::pipeline(device, &layout, shader, format, vertex_entry, Blend::SourceOver);
        #[cfg(target_os = "windows")]
        if let Some((shader, pipeline)) = dxbc::uber(device, make_pipeline) {
            return Ok(Built { bind_layout, layout, shader, pipeline, source: Source::Precompiled });
        }
        #[cfg(target_os = "macos")]
        if let Some((shader, pipeline)) = windfoil_metallib::load(device, make_pipeline) {
            return Ok(Built { bind_layout, layout, shader: dxbc::Shaders::one(shader), pipeline, source: Source::Precompiled });
        }
        let (shader, pipeline) = compile(device, windfoil_wgsl(), make_pipeline)?;
        Ok(Built { bind_layout, layout, shader, pipeline, source: Source::Compiled })
    }

    /// Whether this frame's subpixel text can blend by channel, starting the pipeline the first
    /// time it's asked: from the shipped bytecode at once where it matches, else compiled on a
    /// thread, which `frame_wake` tells when it's done. Until then subpixel text draws grayscale.
    /// `want`: this frame's text draws subpixel masks. A compile already running is collected
    /// either way, since text draws grayscale while it runs.
    pub(super) fn lcd_ready(&mut self, device: &wgpu::Device, want: bool) -> bool {
        if want && matches!(self.lcd, Lcd::Unbuilt) && self.uber.outlines().is_some() {
            self.lcd = match self.start_lcd(device) {
                Ok(lcd) => lcd,
                Err(why) => Self::refuse_lcd(&why),
            };
            if matches!(self.lcd, Lcd::Built(_)) { self.lcd_built(); }
        }
        if let Lcd::Building(done) = &self.lcd {
            match done.try_recv() {
                Ok(Ok(pipeline)) => {
                    self.lcd = Lcd::Built(pipeline);
                    self.lcd_built();
                }
                Ok(Err(why)) => self.lcd = Self::refuse_lcd(&why),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.lcd = Self::refuse_lcd("its compile ended without a pipeline"),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        want && matches!(self.lcd, Lcd::Built(_))
    }

    /// Subpixel text can blend by channel from now on: it switches at its next frame, on the CPU
    /// and the GPU together, which the window is asked for.
    fn lcd_built(&self) {
        crate::text::note_dual_source(true);
        if let Some(wake) = &self.frame_wake { wake.notify(); }
    }

    fn refuse_lcd(why: &str) -> Lcd {
        eprintln!("rune: subpixel text can't blend by channel here ({why}); it draws grayscale");
        crate::text::note_dual_source(false);
        Lcd::Refused
    }

    fn start_lcd(&self, device: &wgpu::Device) -> Result<Lcd, String> {
        if !device.features().contains(wgpu::Features::DUAL_SOURCE_BLENDING) {
            return Err("the device has no dual-source blending".into());
        }
        let wgsl = dual_wgsl().ok_or("shader.wgsl's fragment entry moved")?;
        let (layout, format, entry) = (self.layout.clone(), self.format, self.vertex_entry);
        #[cfg(target_os = "windows")]
        if let Some((_, pipeline)) = dxbc::uber_dual(device, |shader| lcd_pipeline(device, &layout, format, entry, shader)) {
            return Ok(Lcd::Built(pipeline));
        }
        #[cfg(target_arch = "wasm32")]
        return compile(device, &wgsl, |shader| lcd_pipeline(device, &layout, format, entry, shader)).map(|(_, pipeline)| Lcd::Built(pipeline));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (sent, done) = std::sync::mpsc::channel();
            let (device, wake) = (device.clone(), self.frame_wake.clone());
            std::thread::Builder::new()
                .name("rune-subpixel-shader".into())
                .spawn(move || {
                    // A renderer dropped meanwhile has no receiver: nothing to tell.
                    sent.send(compile(&device, &wgsl, |shader| lcd_pipeline(&device, &layout, format, entry, shader)).map(|(_, pipeline)| pipeline)).ok();
                    if let Some(wake) = wake { wake.notify(); }
                })
                .map_err(|e| format!("no thread to compile it on: {e}"))?;
            Ok(Lcd::Building(done))
        }
    }

    /// The window's wake, which a subpixel pipeline compiled off this thread tells when it's
    /// ready, so a window that rests meanwhile still draws its next frame with it.
    pub(crate) fn set_frame_wake(&mut self, wake: &std::sync::Arc<crate::live::Wake>) {
        if !self.frame_wake.as_ref().is_some_and(|w| std::sync::Arc::ptr_eq(w, wake)) {
            self.frame_wake = Some(wake.clone());
        }
    }

    #[cfg(test)]
    pub(crate) fn has_frame_wake(&self) -> bool {
        self.frame_wake.is_some()
    }

    /// The subpixel pipeline compiled from WGSL now, on this thread, for the tools that capture
    /// its HLSL (`regen_dxbc`): it bypasses the shipped bytecode.
    #[doc(hidden)]
    pub fn compile_subpixel_now(&mut self, device: &wgpu::Device) -> Result<(), String> {
        let wgsl = dual_wgsl().ok_or("shader.wgsl's fragment entry moved")?;
        let (layout, format, entry) = (self.layout.clone(), self.format, self.vertex_entry);
        let (_, pipeline) = compile(device, &wgsl, |shader| lcd_pipeline(device, &layout, format, entry, shader))?;
        self.lcd = Lcd::Built(pipeline);
        Ok(())
    }

    /// Compiles the pipeline pinned to `KIND_*` `kind` (`pinned_wgsl`) from WGSL now, so wgpu logs
    /// its HLSL (`regen_dxbc`): it bypasses the shipped bytecode.
    #[doc(hidden)]
    pub fn compile_pinned_now(&mut self, device: &wgpu::Device, kind: u32) -> Result<(), String> {
        let wgsl = pinned_wgsl(kind).ok_or("shader.wgsl's PINNED_KIND moved")?;
        let (layout, format, entry) = (self.layout.clone(), self.format, self.vertex_entry);
        compile(device, &wgsl, |shader| Self::pipeline(device, &layout, shader, format, entry, Blend::SourceOver))?;
        Ok(())
    }

    /// Starts making the frame pass's pipelines pinned to one kind each (`PINNED`) from the shipped bytecode, on a
    /// thread of their own, for Windows renderers drawing Windfoil from its precompiled shader: on a cold driver cache
    /// (the first launch after an install or a driver update) the driver takes about 150 ms for the three, which no
    /// frame should wait for. The general shader draws every kind until they arrive (`take_pinned`), the same picture.
    /// Anywhere else, or when the bytecode doesn't match, none: every kind draws with the one shader, as before.
    /// Never from WGSL, which takes FXC tens of seconds each.
    #[allow(unsafe_code)]
    pub(super) fn start_pinned(&mut self, device: &wgpu::Device) {
        self.pinned = None;
        self.pinned_building = None;
        #[cfg(target_os = "windows")]
        if matches!(self.uber, Uber::Windfoil { source: Source::Precompiled, .. }) && self.vertex_entry == "vs" {
            let (sent, done) = std::sync::mpsc::channel();
            let (device, layout, format) = (device.clone(), self.layout.clone(), self.format);
            let spawned = std::thread::Builder::new()
                .name("rune-pinned-pipelines".into())
                // A renderer dropped meanwhile has no receiver: nothing to tell.
                .spawn(move || {
                    // Below normal, as the CPU frame's helpers are: on a two-core machine a cold driver cache makes this a
                    // 150 ms compile, which mustn't compete with the first input.
                    // SAFETY: this thread's pseudo-handle.
                    unsafe { windows_sys::Win32::System::Threading::SetThreadPriority(windows_sys::Win32::System::Threading::GetCurrentThread(), windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL) };
                    sent.send(make_pinned(&device, &layout, format)).ok();
                });
            if spawned.is_ok() {
                self.pinned_building = Some((format, done));
                return;
            }
        }
        let _ = device;
        self.pinned = Some(Vec::new());
    }

    /// Takes the pinned pipelines once their thread has made them for this format (one made for a format the
    /// renderer has since left is dropped, and that format's next use starts again).
    pub(super) fn take_pinned(&mut self) {
        let Some((format, done)) = &self.pinned_building else { return };
        // Frame-profile builds with `RUNE_PINNED=all` wait for them, so a capture's every frame draws pinned (the
        // screen comparison forces the split on); every other build never waits.
        #[cfg(feature = "frame-profile")]
        let got = if super::pinned_forced() == Some(true) { done.recv().map_err(|_| std::sync::mpsc::TryRecvError::Disconnected) } else { done.try_recv() };
        #[cfg(not(feature = "frame-profile"))]
        let got = done.try_recv();
        match got {
            Ok(pinned) => {
                if *format == self.format { self.pinned = Some(pinned); }
                self.pinned_building = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pinned = Some(Vec::new());
                self.pinned_building = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    /// Takes `built` as the renderer's shader: every pipeline again from it. Bind groups follow
    /// once the outline texture exists (`rebind_all`).
    fn adopt(&mut self, device: &wgpu::Device, built: Built) {
        self.bind_layout = built.bind_layout;
        self.layout = built.layout;
        self.shader = built.shader;
        self.pipeline = built.pipeline;
        for index in 0..self.extra.len() {
            let format = self.extra[index].0;
            self.extra[index].1 = Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver);
        }
        self.shader_changed(device);
        // Built on the old bind layout; made again at the next backdrop blur and subpixel frame.
        self.backdrop_blur = None;
        self.lcd = Lcd::Unbuilt;
        // The pinned pipelines again from the new shader, on their thread: the general shader draws until they're in.
        self.pinned_extra.clear();
        self.start_pinned(device);
    }
}

/// The subpixel pipeline from `shader`'s stages: Windfoil's, blending each channel by the
/// fragment's second output.
fn lcd_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat, vertex_entry: &'static str, shader: &dxbc::Shaders) -> wgpu::RenderPipeline {
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrc1, operation: wgpu::BlendOperation::Add },
        alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("uber subpixel"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader.vertex,
            entry_point: Some(vertex_entry),
            compilation_options: Default::default(),
            buffers: &[Some(metallib::uber_vertex_layout())],
        },
        primitive: metallib::uber_primitive(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader.fragment,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// `wgsl`'s module and pipeline, compiled now, or the error the device reported.
fn compile(device: &wgpu::Device, wgsl: &str, make_pipeline: impl FnOnce(&dxbc::Shaders) -> wgpu::RenderPipeline) -> Result<(dxbc::Shaders, wgpu::RenderPipeline), String> {
    #[cfg(not(target_arch = "wasm32"))]
    let scopes = (device.push_error_scope(wgpu::ErrorFilter::Validation), device.push_error_scope(wgpu::ErrorFilter::Internal));
    let shader = dxbc::Shaders::one(device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("uber"), source: wgpu::ShaderSource::Wgsl(wgsl.into()) }));
    let pipeline = make_pipeline(&shader);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(error) = pollster::block_on(scopes.1.pop()).or(pollster::block_on(scopes.0.pop())) {
        return Err(error.to_string());
    }
    Ok((shader, pipeline))
}

/// The pinned pipelines for `format` (`Renderer::start_pinned`'s thread): none taken (refused, or every fragment
/// stale) is none at all, so nothing is split.
#[cfg(target_os = "windows")]
fn make_pinned(device: &wgpu::Device, layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat) -> Vec<Option<wgpu::RenderPipeline>> {
    #[cfg(feature = "frame-profile")]
    let started = crate::time::now();
    let pinned: Vec<_> = PINNED.iter().map(|&kind| dxbc::uber_pinned(device, kind, |shader| Renderer::pipeline(device, layout, shader, format, "vs", Blend::SourceOver)).map(|(_, p)| p)).collect();
    // Frame-profile builds: what making them costs, and what the run-time freshness check before `PINNED_FRESH`
    // cost (three `pinned_wgsl` and their hashes).
    #[cfg(feature = "frame-profile")]
    {
        let made = started.elapsed();
        let checked = crate::time::now();
        let old: usize = PINNED.iter().filter_map(|&kind| super::pinned_wgsl_blake3(kind)).map(|h| h.len()).sum();
        eprintln!("rune-warm pinned pipelines: {:.0} us to make {} of 3 on their thread (the run-time check they replace: {:.0} us, {old} hex)", made.as_secs_f64() * 1e6, pinned.iter().filter(|p| p.is_some()).count(), checked.elapsed().as_secs_f64() * 1e6);
    }
    if pinned.iter().any(Option::is_some) { pinned } else { Vec::new() }
}