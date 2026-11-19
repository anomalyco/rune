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
            .child(el().w(2.0).h(2.0).fill(crate::Color::BLACK))
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
                let _ = done.send(t);
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
