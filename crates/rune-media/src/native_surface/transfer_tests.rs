use super::*;
use super::gpu_tests::{Gpu, draw, pixel};

macro_rules! on_each_backend {
    ($wgpu:ident, $metal:ident, $body:ident) => {
        #[test]
        fn $wgpu() {
            let Some(gpu) = Gpu::headless_wgpu() else { return };
            $body(gpu);
        }
        #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
        #[test]
        fn $metal() {
            let Some(gpu) = Gpu::try_new(rune_gpu::metal::instance()) else { return };
            $body(gpu);
        }
    };
}

fn srgb_u8(x: f32) -> u8 {
    let x = if x <= 0.0031308 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn publish(s: &NativeSurface, b: ByteFrame, id: u64, native: bool) {
    #[cfg(target_os = "macos")]
    if native {
        s.push(macos::PixelBuffer::from_bytes(&b).unwrap(), id).unwrap();
        return;
    }
    let _ = native;
    s.push_bytes(b, id).unwrap();
}

fn render_into(
    gpu: &Gpu,
    renderer: &mut SurfaceRenderer,
    draws: &[SurfaceDraw],
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let (completed, completion) = std::sync::mpsc::channel();
    for d in draws {
        let completed = completed.clone();
        d.surface.set_observer(Arc::new(move |t| {
            if t.completed_at.is_some() {
                completed.send(()).ok();
            }
        }));
    }
    renderer.prepare(&gpu.device, &gpu.queue, draws, size);
    for d in draws {
        assert_eq!(d.surface.last_error(), None);
    }
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("transfer-test-offscreen"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let padded_row = (size.0 * 4).div_ceil(256) * 256;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("transfer-test-readback"),
        size: (padded_row * size.1) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            timestamp_writes: renderer.timestamp_writes(),
            ..Default::default()
        });
        for i in 0..renderer.len() {
            renderer.draw(&mut pass, i);
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(size.1) },
        },
        wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
    );
    renderer.submit(&gpu.queue, encoder);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let row = (size.0 * 4) as usize;
    let stride = padded_row as usize;
    let pixels = {
        let data = slice.get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity(row * size.1 as usize);
        for y in 0..size.1 as usize {
            out.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        out
    };
    buffer.unmap();
    for _ in draws {
        gpu.wait(&completion, std::time::Duration::from_secs(10)).expect("frame completion");
    }
    pixels
}

on_each_backend!(
    video_transfers_match_reference_in_linear_and_srgb_blend,
    video_transfers_match_reference_in_linear_and_srgb_blend_on_metal,
    video_transfers_match_reference_in_linear_and_srgb_blend_on
);
fn video_transfers_match_reference_in_linear_and_srgb_blend_on(gpu: Gpu) {
    let mut linear_renderer = SurfaceRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut srgb_renderer = SurfaceRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    srgb_renderer.set_srgb_blend(true);

    let surface = NativeSurface::new();
    let mut id = 0u64;

    let nv12_codes: &[[u8; 3]] = &[
        [0, 128, 128],
        [3, 128, 128],
        [16, 128, 128],
        [32, 128, 128],
        [51, 128, 128],
        [54, 128, 128],
        [64, 128, 128],
        [128, 128, 128],
        [192, 128, 128],
        [235, 128, 128],
        [255, 128, 128],
        [81, 90, 240],
        [145, 54, 34],
        [41, 240, 110],
    ];

    #[cfg(target_os = "macos")]
    let natives: &[bool] = &[false, true];
    #[cfg(not(target_os = "macos"))]
    let natives: &[bool] = &[false];

    for &native in natives {
        for format in [Format::Nv12Video, Format::Nv12Full] {
            for matrix in [Matrix::Bt601, Matrix::Bt709] {
                for transfer in [Transfer::Srgb, Transfer::Bt709] {
                    let color = ColorInfo { matrix, transfer };
                    for &codes in nv12_codes {
                        id += 1;
                        publish(
                            &surface,
                            ByteFrame {
                                width: 8,
                                height: 8,
                                format,
                                color,
                                planes: [vec![codes[0]; 64], [codes[1], codes[2]].repeat(16)],
                            },
                            id,
                            native,
                        );
                        let draws = [draw(&surface, [0.0, 0.0, 8.0, 8.0])];
                        let lin_px = render_into(&gpu, &mut linear_renderer, &draws, (8, 8), wgpu::TextureFormat::Rgba8UnormSrgb);
                        let srgb_px = render_into(&gpu, &mut srgb_renderer, &draws, (8, 8), wgpu::TextureFormat::Rgba8Unorm);
                        let reference = nv12_reference(codes[0], codes[1], codes[2], format, color).map(srgb_u8);

                        let lin_p = pixel(&lin_px, 8, 4, 4);
                        let srgb_p = pixel(&srgb_px, 8, 4, 4);
                        for c in 0..3 {
                            assert!(
                                lin_p[c].abs_diff(reference[c]) <= 1,
                                "linear blend native={native} {format:?} {matrix:?} {transfer:?} {codes:?}: got {lin_p:?}, ref {reference:?}"
                            );
                            assert!(
                                srgb_p[c].abs_diff(reference[c]) <= 1,
                                "srgb blend native={native} {format:?} {matrix:?} {transfer:?} {codes:?}: got {srgb_p:?}, ref {reference:?}"
                            );
                            assert!(
                                lin_p[c].abs_diff(srgb_p[c]) <= 1,
                                "linear vs srgb blend diverged native={native} {format:?} {matrix:?} {transfer:?} {codes:?}: linear={lin_p:?}, srgb={srgb_p:?}"
                            );
                        }
                        assert_eq!(lin_p[3], 255);
                        assert_eq!(srgb_p[3], 255);

                        // sRGB NV12 full-range neutral chroma passes through exact in sRGB blend mode.
                        if format == Format::Nv12Full && transfer == Transfer::Srgb && codes[1] == 128 && codes[2] == 128 {
                            assert_eq!(
                                [srgb_p[0], srgb_p[1], srgb_p[2]],
                                [codes[0]; 3],
                                "sRGB NV12Full neutral must pass through unchanged in sRGB blend mode"
                            );
                        }
                    }
                }
            }
        }

        // BGRA frames in both transfers and both blend modes, including BT.709 code 54 (-> 70) and partial alpha.
        for transfer in [Transfer::Srgb, Transfer::Bt709] {
            let color = ColorInfo { matrix: Matrix::Bt709, transfer };
            for &v in &[0u8, 3, 16, 32, 51, 54, 64, 128, 192, 235, 255] {
                id += 1;
                publish(
                    &surface,
                    ByteFrame {
                        width: 8,
                        height: 8,
                        format: Format::Bgra,
                        color,
                        planes: [[v, v, v, 255].repeat(64), Vec::new()],
                    },
                    id,
                    native,
                );
                let draws = [draw(&surface, [0.0, 0.0, 8.0, 8.0])];
                let lin_px = render_into(&gpu, &mut linear_renderer, &draws, (8, 8), wgpu::TextureFormat::Rgba8UnormSrgb);
                let srgb_px = render_into(&gpu, &mut srgb_renderer, &draws, (8, 8), wgpu::TextureFormat::Rgba8Unorm);
                let expected = srgb_u8(linear(v as f32 / 255.0, transfer));
                let lin_p = pixel(&lin_px, 8, 4, 4);
                let srgb_p = pixel(&srgb_px, 8, 4, 4);
                for c in 0..3 {
                    assert!(
                        lin_p[c].abs_diff(expected) <= 1,
                        "linear BGRA native={native} {transfer:?} v={v}: got {lin_p:?}, ref {expected}"
                    );
                    assert!(
                        srgb_p[c].abs_diff(expected) <= 1,
                        "srgb BGRA native={native} {transfer:?} v={v}: got {srgb_p:?}, ref {expected}"
                    );
                }
                if transfer == Transfer::Srgb {
                    assert_eq!([srgb_p[0], srgb_p[1], srgb_p[2]], [v; 3], "sRGB BGRA must pass through exact in sRGB blend");
                }
            }
        }

        // Partial-alpha BGRA with BT.709 premultiplies in the blend target's space.
        id += 1;
        publish(
            &surface,
            ByteFrame {
                width: 8,
                height: 8,
                format: Format::Bgra,
                color: ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Bt709 },
                planes: [[54u8, 54, 54, 128].repeat(64), Vec::new()],
            },
            id,
            native,
        );
        let draws = [draw(&surface, [0.0, 0.0, 8.0, 8.0])];
        let lin_px = render_into(&gpu, &mut linear_renderer, &draws, (8, 8), wgpu::TextureFormat::Rgba8UnormSrgb);
        let srgb_px = render_into(&gpu, &mut srgb_renderer, &draws, (8, 8), wgpu::TextureFormat::Rgba8Unorm);
        let a = 128.0 / 255.0;
        let lin_val = linear(54.0 / 255.0, Transfer::Bt709);
        let expected_lin = srgb_u8(lin_val * a);
        let expected_srgb = (rune_core::color::srgb_encode(lin_val) * a * 255.0).round() as u8;
        let lin_p = pixel(&lin_px, 8, 4, 4);
        let srgb_p = pixel(&srgb_px, 8, 4, 4);
        for c in 0..3 {
            assert!(lin_p[c].abs_diff(expected_lin) <= 1, "linear premul BGRA BT.709: got {lin_p:?}, want {expected_lin}");
            assert!(srgb_p[c].abs_diff(expected_srgb) <= 1, "srgb premul BGRA BT.709: got {srgb_p:?}, want {expected_srgb}");
        }
        assert_eq!(lin_p[3], 128);
        assert_eq!(srgb_p[3], 128);
    }
}
