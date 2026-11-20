use std::sync::Arc;

use crate::el::{Align, El, el};
use crate::native_surface::place::SurfacePlace;
use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceFit};
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::runtime::{Cx, Env, Runtime};

const TOP_LEFT_1X: SurfacePlace = SurfacePlace { anchor: [Align::Start; 2], unscaled: true };

macro_rules! on_each_backend {
    ($wgpu:ident, $metal:ident, $body:ident) => {
        #[test]
        fn $wgpu() {
            $body(Gpu::headless_wgpu());
        }
        #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
        #[test]
        fn $metal() {
            $body(Gpu::new(crate::gpu::metal::instance(), None));
        }
    };
}

#[test]
fn the_element_carries_its_placement_to_the_draw() {
    type State = (NativeSurface, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let e = el().size(80.0, 60.0).native_surface(&s.0, SurfaceFit::Fill);
        if s.1 { e.surface_anchor(Align::Start, Align::Start).surface_unscaled(true) } else { e }
    }
    let surface = NativeSurface::new();
    surface
        .push_bytes(
            ByteFrame { width: 4, height: 2, format: Format::Bgra, color: ColorInfo::default(), planes: [vec![0; 32], vec![]] },
            1,
        )
        .unwrap();
    for (anchored, want) in [(false, SurfacePlace::CENTRED), (true, TOP_LEFT_1X)] {
        let mut rt = Runtime::new((surface.clone(), anchored), view, (80.0, 60.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.surfaces.len(), 1);
        assert_eq!(rt.surfaces[0].surface.place(), want);
        assert_eq!(rt.surfaces[0].surface.key(), surface.key(), "same frames");
    }
}

on_each_backend!(
    video_interleaves_with_renderer_springs_and_external_timing,
    video_interleaves_with_renderer_springs_and_external_timing_on_metal,
    video_interleaves_with_renderer_springs_and_external_timing_on
);
fn video_interleaves_with_renderer_springs_and_external_timing_on(gpu: Gpu) {
    let surface = NativeSurface::new();
    surface
        .push_bytes(
            ByteFrame {
                width: 16,
                height: 8,
                format: Format::Bgra,
                color: ColorInfo::default(),
                planes: [vec![255; 16 * 8 * 4], vec![]],
            },
            1001,
        )
        .unwrap();

    fn view(s: &NativeSurface, _: &mut Cx<NativeSurface>) -> El {
        el()
            .w(8.0)
            .h(4.0)
            .fill(crate::Color::BLACK)
            .native_surface(s, SurfaceFit::Fit)
            .border(1.0, crate::Color::BLACK)
            // 3x3 pt (6x6 px at 2x) puts the probed pixel (3, 3) clear of both the 2 px border
            // and the child's outer anti-aliasing boundary.
            .child(el().w(3.0).h(3.0).fill(crate::Color::BLACK))
    }
    let mut rt = Runtime::new(surface.clone(), view, (8.0, 4.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.surfaces[0].rect, [0.0, 0.0, 16.0, 8.0]);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (16, 8));
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
    let target = Offscreen::new(&gpu.device, 16, 8);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    let p = target.read(&gpu.device);
    assert_eq!(&p[0..4], &[0, 0, 0, 255], "border must paint over video");
    assert_eq!(&p[(3 * 16 + 3) * 4..(3 * 16 + 3) * 4 + 4], &[0, 0, 0, 255], "child must paint over video");
    assert_eq!(&p[(4 * 16 + 10) * 4..(4 * 16 + 10) * 4 + 4], &[255; 4], "video must paint over background");

    // A video pipeline switch must also restore the retained-springs bind group before
    // painting the border and child. Identity tracks should exactly match ordinary rendering.
    let zero = crate::gpu::springs::Track::constant(0.0);
    let node = crate::gpu::springs::Node { x: zero, y: zero, opacity: crate::gpu::springs::Track::constant(1.0) };
    let mut sprung = Renderer::with_springs(&gpu.device, OFFSCREEN_FORMAT, &[node], &vec![0; rt.instances.len()]);
    sprung.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (16, 8));
    sprung.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    sprung.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    sprung.submit(&gpu.queue, encoder);
    assert_eq!(target.read(&gpu.device), p, "retained-springs binding must survive the video draw");

    // A custom compositor can omit video. Preparing it alone must not report a submission.
    let submitted = surface.counts().0;
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
    renderer.submit(&gpu.queue, gpu.device.create_command_encoder(&Default::default()));
    assert_eq!(surface.counts().0, submitted, "undrawn video must not report submission");

    // External pass timing takes precedence without resolving NativeSurface's unwritten
    // query set. Frame completion still arrives, with no native GPU timestamps.
    if gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        let external = gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("external UI timing"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let (done, completion) = std::sync::mpsc::channel();
        surface.set_observer(Arc::new(move |t| {
            if t.completed_at.is_some() {
                done.send(t).ok();
            }
        }));
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (16, 8));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw_timed(
            &mut encoder,
            &target.view,
            [0.0; 4],
            Some(wgpu::RenderPassTimestampWrites {
                query_set: &external,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            }),
        );
        renderer.submit(&gpu.queue, encoder);
        // The preceding render's callback may still arrive; consume until this submission.
        let submission = surface.timing().unwrap().submitted_at;
        loop {
            let t = completion.recv_timeout(std::time::Duration::from_secs(10)).expect("externally timed frame completion");
            if t.submitted_at != submission {
                continue;
            }
            assert_eq!(t.gpu_start_seconds, None);
            assert_eq!(t.gpu_end_seconds, None);
            break;
        }
    }
}

#[test]
fn video_continuous_corners_match_uber_shader() {
    use crate::render::{Instance, KIND_RECT, NO_CLIP, pack_clip_radii};

    let gpu = Gpu::headless();
    let (w, h) = (96u32, 80u32);
    let surface = NativeSurface::new();
    surface
        .push_bytes(
            ByteFrame {
                width: 80,
                height: 64,
                format: Format::Bgra,
                color: ColorInfo::default(),
                planes: [vec![255; 80 * 64 * 4], vec![]],
            },
            1,
        )
        .unwrap();

    let cases: [(&str, [f32; 4], [f32; 4], [f32; 4], Option<[f32; 4]>); 3] = [
        ("uniform-continuous", [8.0, 8.0, 80.0, 64.0], [-20.0; 4], NO_CLIP, None),
        ("mixed-continuous", [8.0, 8.0, 80.0, 64.0], [-20.0, 12.0, -10.0, 0.0], NO_CLIP, None),
        (
            "continuous-clip",
            [8.0, 8.0, 80.0, 64.0],
            [0.0; 4],
            [12.0, 12.0, 84.0, 68.0],
            Some([-16.0, -10.0, -16.0, 0.0]),
        ),
    ];

    for (name, box_rect, radii, clip, clip_radii) in cases {
        let mut fade = [0u32; 4];
        if let Some(cr) = clip_radii {
            fade[3] = pack_clip_radii(clip, cr);
        }

        // Reference: uber shader (`shader.wgsl`) solid white KIND_RECT with the same geometry.
        let inst = Instance {
            rect: box_rect,
            radii,
            fill: [1.0; 4],
            border: [0.0; 4],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip,
            fade,
        };
        let mut atlas = crate::text::Atlas::new();
        let mut uber = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        uber.prepare(&gpu.device, &gpu.queue, &mut atlas, &[inst], (w, h));
        let uber_target = Offscreen::new(&gpu.device, w, h);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        uber.draw(&mut enc, &uber_target.view, [0.0; 4]);
        uber_target.copy(&mut enc);
        uber.submit(&gpu.queue, enc);
        let uber_px = uber_target.read(&gpu.device);

        // Video shader (`video.wgsl`) with a solid white frame and the exact same rect/radii/clip/fade.
        let draw = crate::native_surface::SurfaceDraw {
            surface: surface.clone(),
            fit: SurfaceFit::Fill,
            before_instance: 0,
            rect: box_rect,
            radii,
            clip,
            fade,
            opacity: 1.0,
        };
        let mut video = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        video.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (w, h));
        video.prepare_surfaces(&gpu.device, &gpu.queue, &[draw], (w, h));
        let video_target = Offscreen::new(&gpu.device, w, h);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        video.draw(&mut enc, &video_target.view, [0.0; 4]);
        video_target.copy(&mut enc);
        video.submit(&gpu.queue, enc);
        let video_px = video_target.read(&gpu.device);

        let mut max_diff = 0u8;
        let mut worst = None;
        let (x0, y0, x1, y1) = (
            box_rect[0] as u32,
            box_rect[1] as u32,
            (box_rect[0] + box_rect[2]) as u32,
            (box_rect[1] + box_rect[3]) as u32,
        );
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let u = [uber_px[i], uber_px[i + 1], uber_px[i + 2], uber_px[i + 3]];
                let v = [video_px[i], video_px[i + 1], video_px[i + 2], video_px[i + 3]];
                if x < x0 || x >= x1 || y < y0 || y >= y1 {
                    assert_eq!(v, [0, 0, 0, 0], "{name}: video drew outside its rect at ({x},{y}): {v:?}");
                    continue;
                }
                let d = (0..4).map(|k| u[k].abs_diff(v[k])).max().unwrap();
                if d > max_diff {
                    max_diff = d;
                    worst = Some((x, y, u, v));
                }
            }
        }
        assert!(
            max_diff <= 1,
            "{name}: video.wgsl drifted from shader.wgsl: max_diff={max_diff} worst={worst:?}"
        );
    }
}
