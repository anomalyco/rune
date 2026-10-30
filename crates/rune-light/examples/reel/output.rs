//! Output: PNGs, the ffmpeg pipes, the render loop and the bench.

use super::*;

pub(super) fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rune::render::write_png(path, w, h, rgba).unwrap();
}

fn downscale(src: &[u8], w: u32, h: u32, k: u32) -> Vec<u8> {
    let (ow, oh) = (w / k, h / k);
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    for y in 0..oh {
        for x in 0..ow {
            let mut acc = [0f32; 3];
            for dy in 0..k {
                for dx in 0..k {
                    let i = (((y * k + dy) * w + x * k + dx) * 4) as usize;
                    for c in 0..3 {
                        acc[c] += lin(src[i + c]);
                    }
                }
            }
            let o = ((y * ow + x) * 4) as usize;
            for c in 0..3 {
                out[o + c] = (rune::color::srgb_encode(acc[c] / (k * k) as f32) * 255.0).round() as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

fn ffmpeg(args: &[&str], out: &Path) -> Child {
    Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-s", &format!("{W}x{H}"), "-framerate", &FPS.to_string()])
        .args(args)
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg")
}

pub(super) fn render(reel: &Reel, ctx: &mut Ctx, out: &Path, from: f32, to: f32) {
    let tag = ["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv", "-movflags", "+faststart"];
    let mut h264_args = vec!["-pix_fmt", "yuv420p", "-i", "-", "-c:v", "libx264", "-preset", "slow", "-crf", "12", "-tune", "film", "-pix_fmt", "yuv420p"];
    h264_args.extend(tag);
    let mut hevc_args = vec!["-pix_fmt", "yuv420p10le", "-i", "-", "-c:v", "libx265", "-preset", "medium", "-crf", "10", "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1", "-x265-params", "log-level=error:aq-mode=3"];
    hevc_args.extend(tag);
    let mut h264 = ffmpeg(&h264_args, &out.join("rc-reel.mp4"));
    let mut hevc = ffmpeg(&hevc_args, &out.join("rc-reel-master-10bit.mp4"));
    let n0 = (from * FPS as f32).round() as u32;
    let n1 = (to * FPS as f32).round() as u32;
    let poster_t = T_END + 1.8;
    let sheet_t = [T_TILT + 2.2, T_INPUTS + 1.0, T_CASCADES + 2.2, T_MERGE + 1.4, T_FLUENCE + 0.7, T_UNTILT + 0.9];
    let mut sheet: Vec<Vec<u8>> = Vec::new();
    let t0 = std::time::Instant::now();
    for f in n0..n1 {
        let t = f as f32 / FPS as f32;
        let (view, _) = reel.frame(ctx, t, false);
        let want_png = (t - poster_t).abs() < 0.5 / FPS as f32 || sheet_t.iter().any(|s| (t - s).abs() < 0.5 / FPS as f32);
        let (y8, y10, rgba) = ctx.encode(&view, want_png, true);
        h264.stdin.as_mut().unwrap().write_all(&y8.unwrap()).expect("h264 write");
        hevc.stdin.as_mut().unwrap().write_all(&y10.unwrap()).expect("hevc write");
        if let Some(px) = rgba {
            if (t - poster_t).abs() < 0.5 / FPS as f32 {
                write_png(&out.join("poster.png"), W, H, &px);
            } else {
                sheet.push(downscale(&px, W, H, 4));
            }
        }
        if f % 60 == 0 {
            let line = format!("frame {f}/{n1} ({:.1}s)\n", t0.elapsed().as_secs_f32());
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(out.join("progress.log")).and_then(|mut fh| fh.write_all(line.as_bytes()));
        }
    }
    drop(h264.stdin.take());
    drop(hevc.stdin.take());
    h264.wait().expect("h264");
    hevc.wait().expect("hevc");
    if !sheet.is_empty() {
        let (sw, sh) = (W / 4, H / 4);
        let cols = 3u32;
        let rows = (sheet.len() as u32).div_ceil(cols);
        let gap = 8;
        let (tw, th) = (sw * cols + gap * (cols - 1), sh * rows + gap * (rows - 1));
        let mut img = vec![10u8; (tw * th * 4) as usize];
        for (k, fr) in sheet.iter().enumerate() {
            let (cx, cy) = (k as u32 % cols, k as u32 / cols);
            for y in 0..sh {
                let src = &fr[(y * sw * 4) as usize..((y + 1) * sw * 4) as usize];
                let d = (((cy * (sh + gap) + y) * tw + cx * (sw + gap)) * 4) as usize;
                img[d..d + src.len()].copy_from_slice(src);
            }
        }
        for i in (3..img.len()).step_by(4) {
            img[i] = 255;
        }
        write_png(&out.join("breakdown-sheet.png"), tw, th, &img);
    }
    eprintln!("rendered {} frames in {:.1}s", n1 - n0, t0.elapsed().as_secs_f32());
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

pub(super) fn bench(reel: &Reel, ctx: &mut Ctx, out: &Path, gpu: &str) {
    if !ctx.timestamps {
        eprintln!("no TIMESTAMP_QUERY on this adapter");
        return;
    }
    let mut runs: Vec<(Timings, f64)> = Vec::new();
    for f in 0..150 {
        // The hero at its fullest: every emitter on, progress flooding.
        let t = 5.0 + (f % 60) as f32 / 60.0;
        let (_, b) = reel.frame(ctx, t, true);
        if f >= 30 {
            if let Some(tm) = b.timings {
                runs.push((tm, b.glass_ms));
            }
        }
    }
    // Whole frame, wall clock of back-to-back light + glass passes (GPU-bound), no readback.
    let wall = {
        let st = state(5.5);
        let built = build_ui(&mut ctx.ts, &st);
        let g = glass_at(T_GLASS + 1.5).unwrap();
        let lights: Vec<GlassLight> = built.prims.iter().filter_map(|p| GlassLight::from_emitter(p, 18.0 * S, 0.9)).collect();
        ctx.light.inspect = false;
        let one = |ctx: &mut Ctx| {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            let scene = Scene { prims: &built.prims, lights: &[], settings: glow_settings(5.5), emission: None };
            ctx.light.render(&ctx.device, &ctx.queue, &mut enc, &scene, &ctx.base.view, &ctx.text.view, &ctx.lit.view);
            ctx.glass.render(&ctx.device, &ctx.queue, &mut enc, &[g], &lights, GlassSettings::default(), &ctx.lit.view, ctx.light.inspect().fluence, &ctx.glassed.view);
            ctx.queue.submit([enc.finish()]);
        };
        one(ctx);
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let t0 = std::time::Instant::now();
        for _ in 0..120 {
            one(ctx);
        }
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        t0.elapsed().as_secs_f64() * 1000.0 / 120.0
    };
    let col = |f: &dyn Fn(&(Timings, f64)) -> f64| median(&mut runs.iter().map(f).collect::<Vec<_>>());
    let field = col(&|r| r.0.field);
    let gi = col(&|r| r.0.gi);
    let scene = col(&|r| r.0.gi_scene);
    let fluence = col(&|r| r.0.fluence);
    let glass = col(&|r| r.1);
    let cs: Vec<f64> = (0..CASCADES as usize).map(|n| col(&|r| r.0.cascades[n])).collect();
    let mut s = format!("{{\n  \"gpu\": \"{gpu}\",\n  \"resolution\": \"{W}x{H}\",\n  \"field\": {field:.4},\n  \"gi\": {gi:.4},\n  \"gi_scene\": {scene:.4},\n");
    for (n, c) in cs.iter().enumerate() {
        s += &format!("  \"c{n}\": {c:.4},\n");
    }
    s += &format!("  \"fluence\": {fluence:.4},\n  \"glass\": {glass:.4},\n  \"frame\": {wall:.4}\n}}\n");
    print!("{s}");
    std::fs::write(out.join("bench.json"), s).unwrap();
}
