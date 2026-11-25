//! Per-frame backdrop blur uniform slot pool in `rune-fx`.

use rune::render::{Gpu, Offscreen};
use rune_fx::*;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn make_striped_backdrop(gpu: &Gpu, w: u32, h: u32, cell: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blur-slots backdrop"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let at = ((y * w + x) * 4) as usize;
            let bright = (x % cell) < (cell / 2);
            let px = if bright { [0u8, 229, 255, 255] } else { [20, 20, 20, 255] };
            pixels[at..at + 4].copy_from_slice(&px);
        }
    }
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let view = tex.create_view(&Default::default());
    (tex, view)
}

fn lens(x: f32, y: f32, w: f32, h: f32, frost: f32) -> FxLayer {
    Fx::lens(LensParams {
        frost,
        depth: 0.0,
        magnification: 1.0,
        shadow: 0.0,
        rim: 0.0,
        ..Default::default()
    })
    .on(Shape::rounded(x, y, w, h, 8.0))
    .at(1.0)
}

fn render_lenses(gpu: &Gpu, fx: &mut FxRenderer, w: u32, h: u32, bd: &wgpu::TextureView, layers: &[FxLayer]) -> (Vec<bool>, Vec<u8>) {
    let target = Offscreen::with_format(&gpu.device, w, h, FORMAT);
    let stats = fx.prepare(
        &gpu.device,
        &gpu.queue,
        &FxFrame { viewport: [w, h], scale: 1.0, layers, backdrop: None, sources: &[] },
    );
    assert_eq!(stats.drawn as usize, layers.len());
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    let mut blurred = Vec::with_capacity(layers.len());
    for i in 0..layers.len() {
        blurred.push(fx.blur_backdrop(&gpu.device, &gpu.queue, &mut enc, bd, i));
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if i == 0 { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        fx.draw_layer(&mut pass, i);
    }
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    (blurred, target.read(&gpu.device))
}

fn px(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let at = ((y * width + x) * 4) as usize;
    [rgba[at], rgba[at + 1], rgba[at + 2]]
}

#[test]
fn twelve_frosted_lenses_all_blur_their_backdrop() {
    let gpu = Gpu::headless();
    let (cols, rows, cell) = (4u32, 3u32, 64u32);
    // Keep a 1-cell (64 px > 3*sigma+8 = 62 px) striped margin around the grid so every lens
    // sees identical surroundings without viewport-edge clamping differences.
    let (w, h) = ((cols + 2) * cell, (rows + 2) * cell);
    let (_tex, bd) = make_striped_backdrop(&gpu, w, h, cell);
    let mut fx = FxRenderer::new(&gpu.device, FORMAT, FxOptions::default());

    let mut forward = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows {
        for col in 0..cols {
            let x = ((col + 1) * cell + 4) as f32;
            let y = ((row + 1) * cell + 4) as f32;
            forward.push(lens(x, y, (cell - 8) as f32, (cell - 8) as f32, 1.0));
        }
    }
    let (blurred_fwd, img_fwd) = render_lenses(&gpu, &mut fx, w, h, &bd, &forward);
    assert!(
        blurred_fwd.iter().all(|&ok| ok),
        "all 12 frosted lenses must build their blur pyramid: {blurred_fwd:?}"
    );

    // 8 px right of each cell's cyan/dark step edge: the pyramid frosts bright cyan deeply into
    // the dark half, evenly across all 12 lenses.
    let first = px(&img_fwd, w, cell + 40, cell + 32);
    assert!(
        first[1] > 80 && first[2] > 90,
        "lens 0 blurs cyan into the dark half: {first:?}"
    );
    for idx in 1..forward.len() {
        let col = idx as u32 % cols + 1;
        let row = idx as u32 / cols + 1;
        let sample = px(&img_fwd, w, col * cell + 40, row * cell + 32);
        assert!(
            sample.iter().zip(first).all(|(a, b)| a.abs_diff(b) <= 4),
            "lens {idx} must frost evenly with lens 0: got {sample:?}, lens 0 was {first:?}"
        );
    }

    // Reversing draw order yields matching frosting across all 12 lenses (independent of order).
    let mut reverse = forward;
    reverse.reverse();
    let mut fx_rev = FxRenderer::new(&gpu.device, FORMAT, FxOptions::default());
    let (blurred_rev, img_rev) = render_lenses(&gpu, &mut fx_rev, w, h, &bd, &reverse);
    assert!(blurred_rev.iter().all(|&ok| ok));
    let worst = img_fwd.iter().zip(&img_rev).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    assert!(worst <= 2, "frosting must not depend on draw order (max channel diff {worst})");
}

#[test]
fn twenty_four_mixed_sigma_lenses_and_fluted_glass_all_blur() {
    let gpu = Gpu::headless();
    let (cols, rows, cell) = (6u32, 4u32, 64u32);
    let (w, h) = (cols * cell, rows * cell);
    let (_tex, bd) = make_striped_backdrop(&gpu, w, h, cell);
    let mut fx = FxRenderer::new(&gpu.device, FORMAT, FxOptions::default());

    // 24 distinct frost values across Lens and FlutedGlass, including exact mip levels
    // (blur_px = 4, 8, 16 -> frac = 1.0) and a tiny frost (frac = 0.0).
    let frosts: [f32; 24] = [
        0.05, 4.0 / 18.0, 0.28, 0.33, 0.39, 8.0 / 18.0,
        0.48, 0.52, 0.56, 0.60, 0.64, 0.68,
        0.72, 0.76, 0.80, 0.84, 16.0 / 18.0, 0.91,
        0.93, 0.95, 0.97, 0.98, 0.99, 1.0,
    ];
    let layers: Vec<FxLayer> = frosts
        .iter()
        .enumerate()
        .map(|(i, &frost)| {
            let col = (i as u32 % cols) * cell;
            let row = (i as u32 / cols) * cell;
            let shape = Shape::rounded((col + 4) as f32, (row + 4) as f32, (cell - 8) as f32, (cell - 8) as f32, 8.0);
            if i % 3 == 2 {
                Fx::fill(FlutedGlassParams {
                    frost,
                    refraction: 0.0,
                    dispersion: 0.0,
                    highlight: 0.0,
                    ..FlutedGlassParams::frosted()
                })
                .on(shape)
                .at(1.0)
            } else {
                lens((col + 4) as f32, (row + 4) as f32, (cell - 8) as f32, (cell - 8) as f32, frost)
            }
        })
        .collect();

    let (blurred, img) = render_lenses(&gpu, &mut fx, w, h, &bd, &layers);
    assert!(
        blurred.iter().all(|&ok| ok),
        "all 24 mixed-sigma layers must blur: {blurred:?}"
    );
    // The last layer (index 23, frost = 1.0) blurs cyan deeply into its dark half.
    let last = px(&img, w, 5 * cell + 40, 3 * cell + 32);
    assert!(
        last[1] > 80 && last[2] > 90,
        "24th mixed-sigma lens blurs its backdrop: {last:?}"
    );
}

#[test]
fn repeated_and_exact_mip_weights_exceed_256_lenses_without_fallback() {
    let gpu = Gpu::headless();
    let (w, h) = (128u32, 128u32);
    let (_tex, bd) = make_striped_backdrop(&gpu, w, h, 64);
    let mut fx = FxRenderer::new(&gpu.device, FORMAT, FxOptions::default());

    let palette = [1.0, 0.6, 0.85, 8.0 / 18.0, 16.0 / 18.0];
    let layers: Vec<FxLayer> = (0..300)
        .map(|i| lens(8.0, 8.0, 48.0, 48.0, palette[i % palette.len()]))
        .collect();
    fx.prepare(
        &gpu.device,
        &gpu.queue,
        &FxFrame { viewport: [w, h], scale: 1.0, layers: &layers, backdrop: None, sources: &[] },
    );
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    for i in 0..layers.len() {
        assert!(
            fx.blur_backdrop(&gpu.device, &gpu.queue, &mut enc, &bd, i),
            "lens {i} sharing repeated/exact-mip weights must not exhaust blur slots"
        );
    }
    gpu.queue.submit([enc.finish()]);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
}

#[test]
fn distinct_weights_past_blur_slots_fall_back_gracefully_and_reset_next_frame() {
    let gpu = Gpu::headless();
    let (w, h) = (128u32, 128u32);
    let (_tex, bd) = make_striped_backdrop(&gpu, w, h, 64);
    let mut fx = FxRenderer::new(&gpu.device, FORMAT, FxOptions::default());

    // 260 strictly distinct frost values inside (4/18, 8/18), so every layer gets n = 2 and a
    // distinct fractional weight strictly in (0, 1).
    let count = 260usize;
    let lo = 4.5f32 / 18.0;
    let hi = 7.5f32 / 18.0;
    let layers: Vec<FxLayer> = (0..count)
        .map(|i| {
            let t = i as f32 / (count - 1) as f32;
            let frost = lo + (hi - lo) * t;
            lens(8.0, 8.0, 48.0, 48.0, frost)
        })
        .collect();

    for frame_idx in 0..2 {
        let (blurred, img) = render_lenses(&gpu, &mut fx, w, h, &bd, &layers);
        let ok_count = blurred.iter().filter(|&&b| b).count();
        assert_eq!(
            ok_count, 255,
            "frame {frame_idx}: first 255 distinct fractional weights fit slots 1..=255"
        );
        assert!(blurred[..255].iter().all(|&b| b));
        assert!(
            blurred[255..].iter().all(|&b| !b),
            "frame {frame_idx}: layers past BLUR_SLOTS fall back cleanly"
        );
        // The last layer still rendered via the 8-tap fallback without panicking or blanking.
        let center = px(&img, w, 32, 32);
        assert!(
            center.iter().any(|&c| c > 20),
            "frame {frame_idx}: fallback layer still renders backdrop: {center:?}"
        );
    }
}

#[test]
fn twelve_frosted_lenses_through_draw_with_backdrop() {
    use std::sync::{Arc, Mutex};
    use rune::color::hex;
    use rune::compose::{Compositor, Frame};
    use rune::render::{EffectPhase, EffectSite, KIND_EFFECT};
    use rune::{App, AppSpec, Cx, El, effect, el};

    struct TwelveLenses {
        fx: Option<FxRenderer>,
        blurred: Arc<Mutex<Vec<bool>>>,
    }

    impl Compositor<()> for TwelveLenses {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            let (device, queue, size) = (f.device, f.queue, f.size);
            let slots: Vec<(u32, rune::render::Instance)> = f
                .rt
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.params[1] == KIND_EFFECT)
                .map(|(n, i)| (n as u32, *i))
                .collect();
            let layers: Vec<FxLayer> = slots
                .iter()
                .map(|(_, i)| {
                    let k = i.params[3];
                    let [x, y, w, h] = i.rect.map(|v| v / k);
                    lens(x, y, w, h, 1.0)
                })
                .collect();
            let fx = self.fx.get_or_insert_with(|| FxRenderer::new(device, f.format, FxOptions::default()));
            fx.set_srgb_blend(f.srgb_blend());
            let placements: Vec<_> = slots.iter().map(|(_, i)| FxSlot::default().scale(i.params[3]).clip(i.clip, i.fade)).collect();
            fx.prepare_with_slots(
                device,
                queue,
                &FxFrame { viewport: [size.0, size.1], scale: 1.0, layers: &layers, backdrop: None, sources: &[] },
                &placements,
            );
            let clear = f.rt.clear_color();
            f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
            let blurred = &self.blurred;
            f.ui.draw_with_backdrop(f.encoder, f.target, clear, &mut |phase, s: &EffectSite<'_>| match phase {
                EffectPhase::Backdrop(enc) => {
                    if let (Some(bd), Some(k)) = (s.backdrop, slots.iter().position(|(at, _)| *at == s.index)) {
                        blurred.lock().unwrap().push(fx.blur_backdrop(device, queue, enc, bd, k));
                    }
                }
                EffectPhase::Draw(pass) => {
                    if let Some(k) = slots.iter().position(|(at, _)| *at == s.index) {
                        fx.draw_layer_in(pass, s.format, s.origin, k);
                    }
                }
            });
        }
    }

    const COLS: u32 = 4;
    const ROWS: u32 = 3;
    const CELL: f32 = 64.0;
    const W: f32 = (COLS + 2) as f32 * CELL;
    const H: f32 = (ROWS + 2) as f32 * CELL;
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut root = el().size(W, H).fill(hex("#141414").unwrap());
        for col in 0..(COLS + 2) {
            root = root.child(
                el().absolute(col as f32 * CELL, 0.0)
                    .size(CELL * 0.5, H)
                    .fill(hex("#00e5ff").unwrap()),
            );
        }
        let mut mark = 1u32;
        for row in 0..ROWS {
            for col in 0..COLS {
                root = root.child(
                    effect(mark)
                        .effect_backdrop(true)
                        .absolute((col + 1) as f32 * CELL + 4.0, (row + 1) as f32 * CELL + 4.0)
                        .size(CELL - 8.0, CELL - 8.0),
                );
                mark += 1;
            }
        }
        root
    }

    let dir = std::env::temp_dir().join(format!("rune-fx-twelve-lenses-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("snap.png");
    let blurred = Arc::new(Mutex::new(Vec::new()));
    App::new(AppSpec::new("twelve-lenses", (W, H), || (), view))
        .compositor(TwelveLenses { fx: None, blurred: Arc::clone(&blurred) })
        .args(["--snap".to_string(), path.to_str().unwrap().to_string(), "--scale".to_string(), "1".to_string()])
        .run();
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap())).read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    std::fs::remove_dir_all(&dir).unwrap();

    let ok = blurred.lock().unwrap();
    assert_eq!(ok.len(), 12);
    assert!(ok.iter().all(|&b| b), "all 12 draw_with_backdrop lenses must build blur pyramids: {ok:?}");
    let first = px(&buf, info.width, 64 + 40, 64 + 32);
    assert!(first[1] > 80 && first[2] > 90, "first lens blurs cyan into dark half: {first:?}");
    for idx in 1..12u32 {
        let col = idx % COLS + 1;
        let row = idx / COLS + 1;
        let sample = px(&buf, info.width, col * 64 + 40, row * 64 + 32);
        assert!(
            sample.iter().zip(first).all(|(a, b)| a.abs_diff(b) <= 4),
            "draw_with_backdrop lens {idx} must frost evenly with lens 0: got {sample:?}, lens 0 was {first:?}"
        );
    }
}
