use super::gpu_tests::{Gpu, OFFSCREEN_FORMAT, Offscreen, draw};
use super::*;

fn bgra_frame(width: u32, height: u32, bgra: [u8; 4]) -> ByteFrame {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&bgra);
    }
    ByteFrame {
        width,
        height,
        format: Format::Bgra,
        color: ColorInfo::default(),
        planes: [pixels, Vec::new()],
    }
}

/// Samples a single `Bgra8Unorm` texture in a custom pipeline and reads back the center pixel.
fn sample_bgra(gpu: &Gpu, texture: &wgpu::Texture, format: wgpu::TextureFormat) -> [u8; 4] {
    let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sample-bgra-test"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(pos[i], 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return textureSample(tex, samp, vec2(0.5, 0.5));
}
"#
            .into(),
        ),
    });
    let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sample-bgra-test"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let view = texture.create_view(&Default::default());
    let sampler = gpu.device.create_sampler(&Default::default());
    let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sample-bgra-test"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let target = Offscreen::new(&gpu.device, 4, 4);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sample-bgra-test"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let px = target.read(&gpu.device);
    [px[0], px[1], px[2], px[3]]
}

#[test]
fn surface_importer_imports_bgra_and_nv12_and_caches_by_frame() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let mut importer = SurfaceImporter::new(&gpu.device);
    let surface = NativeSurface::new();

    assert!(importer.import(&gpu.queue, &surface).unwrap().is_none(), "empty surface returns Ok(None)");

    // BGRA: B=40, G=120, R=220, A=255.
    surface.push_bytes(bgra_frame(8, 6, [40, 120, 220, 255]), 7).unwrap();
    let first = importer.import(&gpu.queue, &surface).unwrap().expect("BGRA frame imported");
    assert_eq!(first.format(), Format::Bgra);
    assert_eq!(first.size(), [8, 6]);
    assert_eq!(first.upright_size(), [8, 6]);
    assert_eq!(first.color(), ColorInfo::default());
    assert_eq!(first.orient(), Orient::UP);
    assert!(!first.mirrored());
    assert_eq!(first.frame_id(), 7);
    assert_eq!(first.revision(), surface.revision());
    assert_eq!(first.planes().len(), 1);
    assert_eq!(first.texture(), &first.planes()[0]);
    assert!(first.chroma().is_none());
    assert_eq!(first.texture().format(), wgpu::TextureFormat::Bgra8Unorm);

    // Sampling the imported texture in a custom pipeline yields the pushed colour
    // (Offscreen uses Rgba8UnormSrgb, so linear values from Bgra8Unorm are sRGB-encoded on write).
    let sampled = sample_bgra(&gpu, first.texture(), OFFSCREEN_FORMAT);
    assert!(sampled[0] > sampled[1] && sampled[1] > sampled[2] && sampled[3] == 255, "unexpected BGRA sample: {sampled:?}");

    // Unchanged frame hits the cache (identical wgpu::Texture handle).
    let cached = importer.import(&gpu.queue, &surface).unwrap().expect("cached BGRA");
    assert_eq!(cached.texture(), first.texture());
    assert_eq!(cached.revision(), first.revision());

    // Toggling mirror bumps revision without re-uploading the underlying texture.
    surface.set_mirrored(true);
    let mirrored = importer.import(&gpu.queue, &surface).unwrap().expect("mirrored BGRA");
    assert_eq!(mirrored.texture(), first.texture(), "mirror toggle must reuse cached texture");
    assert!(mirrored.mirrored());
    assert!(mirrored.revision() > first.revision());

    // Pushing an NV12 frame replaces the cached texture with 2 planes (R8Unorm + Rg8Unorm).
    surface.push_bytes_oriented(synthetic(16, 10, 1), 8, Orient([0, 1, -1, 0])).unwrap();
    let nv12 = importer.import(&gpu.queue, &surface).unwrap().expect("NV12 frame imported");
    assert_ne!(nv12.texture(), first.texture());
    assert_eq!(nv12.format(), Format::Nv12Video);
    assert_eq!(nv12.size(), [16, 10]);
    assert_eq!(nv12.upright_size(), [10, 16], "90-degree orientation swaps upright dimensions");
    assert_eq!(nv12.orient(), Orient([0, 1, -1, 0]));
    assert_eq!(nv12.color(), ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Bt709 });
    assert_eq!(nv12.frame_id(), 8);
    assert_eq!(nv12.planes().len(), 2);
    assert_eq!(nv12.texture().format(), wgpu::TextureFormat::R8Unorm);
    assert_eq!(nv12.texture().size().width, 16);
    assert_eq!(nv12.texture().size().height, 10);
    let chroma = nv12.chroma().expect("NV12 must expose chroma plane");
    assert_eq!(chroma.format(), wgpu::TextureFormat::Rg8Unorm);
    assert_eq!(chroma.size().width, 8);
    assert_eq!(chroma.size().height, 5);

    // Clearing evicts the cached entry and returns Ok(None).
    surface.clear();
    assert!(importer.import(&gpu.queue, &surface).unwrap().is_none());
    assert_eq!(importer.cached_len(), 0);
}

#[test]
fn surface_importer_evicts_unused_surfaces_after_submitted_frame() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let mut importer = SurfaceImporter::new(&gpu.device);
    let surface = NativeSurface::new();
    surface.push_bytes(bgra_frame(4, 4, [10, 20, 30, 255]), 1).unwrap();

    // Frame 0: import and submit -> entry stays cached for Frame 1.
    let a = importer.import(&gpu.queue, &surface).unwrap().unwrap();
    assert_eq!(importer.cached_len(), 1);
    importer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));

    // Frame 1: import again -> cache hit.
    let b = importer.import(&gpu.queue, &surface).unwrap().unwrap();
    assert_eq!(a.texture(), b.texture(), "must stay cached across consecutive frames");
    importer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));

    // Frame 2: compositor stops sampling `surface`, submits frame.
    importer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));

    // Frame 3: prune_stale evicts the unsampled surface.
    importer.prune_stale();
    assert_eq!(importer.cached_len(), 0, "unused surface must be evicted after an unsampled frame");
}

#[test]
fn surface_renderer_shares_cache_between_prepare_and_surface_texture() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let drawn = NativeSurface::new();
    let comp_only = NativeSurface::new();
    drawn.push_bytes(bgra_frame(8, 8, [0, 0, 255, 255]), 1).unwrap();
    comp_only.push_bytes(bgra_frame(8, 8, [0, 255, 0, 255]), 2).unwrap();

    // Frame 0: `drawn` goes through prepare(); `comp_only` goes through surface_texture().
    renderer.prepare(&gpu.device, &gpu.queue, &[draw(&drawn, [0.0, 0.0, 8.0, 8.0])], (8, 8));
    let drawn_from_comp = renderer.surface_texture(&gpu.queue, &drawn).unwrap().unwrap();
    let comp_first = renderer.surface_texture(&gpu.queue, &comp_only).unwrap().unwrap();
    assert_eq!(renderer.cached_import_count(), 2);
    renderer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));

    // Frame 1: prepare() with ONLY `drawn` must NOT evict `comp_only` before compositor runs!
    renderer.prepare(&gpu.device, &gpu.queue, &[draw(&drawn, [0.0, 0.0, 8.0, 8.0])], (8, 8));
    let drawn_again = renderer.surface_texture(&gpu.queue, &drawn).unwrap().unwrap();
    let comp_second = renderer.surface_texture(&gpu.queue, &comp_only).unwrap().unwrap();
    assert_eq!(drawn_from_comp.texture(), drawn_again.texture(), "shared cache hit for drawn surface");
    assert_eq!(comp_first.texture(), comp_second.texture(), "compositor-only surface survives prepare()");
    renderer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));

    // Frame 2: compositor stops sampling `comp_only`; submit advances gen.
    renderer.prepare(&gpu.device, &gpu.queue, &[], (8, 8));
    renderer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));

    // Frame 3: prepare() now evicts both the removed draw and the unsampled compositor surface.
    renderer.prepare(&gpu.device, &gpu.queue, &[], (8, 8));
    assert_eq!(renderer.cached_import_count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn surface_importer_imports_macos_iosurface_pixel_buffer_zero_copy() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let mut importer = SurfaceImporter::new(&gpu.device);
    let surface = NativeSurface::new();
    let buffer = macos::PixelBuffer::from_bytes(&bgra_frame(8, 8, [32, 160, 240, 255])).unwrap();
    surface.push(buffer, 11).unwrap();

    let imported = importer.import(&gpu.queue, &surface).unwrap().expect("IOSurface imported");
    assert_eq!(imported.format(), Format::Bgra);
    assert_eq!(imported.frame_id(), 11);
    assert_eq!(imported.planes().len(), 1);
    assert_eq!(importer.live_planes(), 1);

    let sampled = sample_bgra(&gpu, imported.texture(), OFFSCREEN_FORMAT);
    assert!(sampled[0] > sampled[1] && sampled[1] > sampled[2] && sampled[3] == 255, "unexpected IOSurface sample: {sampled:?}");

    drop(imported);
    surface.clear();
    assert!(importer.import(&gpu.queue, &surface).unwrap().is_none());
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    assert_eq!(importer.live_planes(), 0, "CVMetalTexture ownership leaked");
}

#[cfg(target_os = "windows")]
#[test]
fn surface_importer_imports_windows_shared_texture_and_signals_fence() {
    use ::windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
    use ::windows::Win32::Graphics::Direct3D12::{
        D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
        D3D12_RESOURCE_DESC, D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, D3D12_RESOURCE_STATE_COMMON,
        D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12Fence, ID3D12Resource,
    };
    use ::windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use ::windows::core::HSTRING;

    let Some(gpu) = Gpu::headless_wgpu() else { return };
    #[allow(unsafe_code)]
    let (tex_name, _tex_res, tex_handle, fence_name, fence, fence_handle) = unsafe {
        let hal = gpu.device.as_hal::<wgpu::hal::api::Dx12>().expect("DX12 backend");
        let raw = hal.raw_device();
        let pid = std::process::id();
        let tex_name = format!("Local\\rune-importer-test-tex-{pid}");
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: ::windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: 16,
            Height: 16,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
        };
        let mut resource: Option<ID3D12Resource> = None;
        raw.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut resource)
            .expect("shared texture");
        let resource = resource.expect("resource");
        let tex_handle = raw.CreateSharedHandle(&resource, None, GENERIC_ALL.0, &HSTRING::from(&tex_name)).expect("tex handle");

        let fence_name = format!("Local\\rune-importer-test-fence-{pid}");
        let fence: ID3D12Fence = raw.CreateFence(0, D3D12_FENCE_FLAG_SHARED).expect("shared fence");
        let fence_handle = raw.CreateSharedHandle(&fence, None, GENERIC_ALL.0, &HSTRING::from(&fence_name)).expect("fence handle");
        (tex_name, resource, tex_handle, fence_name, fence, fence_handle)
    };
    struct CloseOnDrop(HANDLE);
    impl Drop for CloseOnDrop {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) }.ok();
        }
    }
    let _guards = (CloseOnDrop(tex_handle), CloseOnDrop(fence_handle));

    let mut importer = SurfaceImporter::new(&gpu.device);
    let surface = NativeSurface::new();
    surface
        .push_shared(windows::SharedTexture::named(&tex_name, 16, 16).release_fence(&fence_name, 5), 5)
        .unwrap();

    let imported = importer.import(&gpu.queue, &surface).unwrap().expect("shared texture imported");
    assert_eq!(imported.format(), Format::Bgra);
    assert_eq!(imported.size(), [16, 16]);
    assert_eq!(imported.frame_id(), 5);
    assert_eq!(imported.planes().len(), 1);
    importer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    #[allow(unsafe_code)]
    let completed = unsafe { fence.GetCompletedValue() };
    assert!(completed >= 5, "release fence should reach 5, got {completed}");

    drop(imported);
    surface.clear();
    assert!(importer.import(&gpu.queue, &surface).unwrap().is_none());
    let win = importer.windows_importer().expect("windows importer");
    assert!(win.cached_texture_names().is_empty(), "cleared surface must prune shared handles");
    assert!(win.copy_pool_counts().is_empty(), "cleared surface must prune copy ring");
}
