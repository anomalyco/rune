use super::nv12::byte_frame_to_rgba;
use super::thumbnails::{DecodedFrame, TestDecoder, TestOpener};
use super::{MediaSource, Player, ThumbnailPrecision, ThumbnailRequest, Thumbnails};
use crate::native_surface::{ByteFrame, ColorInfo, Format, Matrix, Orient, Transfer};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex};

fn solid_frame(r: u8, g: u8, b: u8, actual_time: f64) -> DecodedFrame {
    DecodedFrame {
        width: 2,
        height: 2,
        rgba: vec![r, g, b, 255, r, g, b, 255, r, g, b, 255, r, g, b, 255],
        actual_time,
    }
}

struct FnDecoder<F> {
    decode: F,
    dropped: Option<Arc<(Mutex<bool>, Condvar)>> ,
}

impl<F> Drop for FnDecoder<F> {
    fn drop(&mut self) {
        if let Some(signal) = &self.dropped {
            let mut done = signal.0.lock().unwrap();
            *done = true;
            signal.1.notify_all();
        }
    }
}

impl<F: FnMut(f64, u32, ThumbnailPrecision) -> Result<DecodedFrame, String>> TestDecoder for FnDecoder<F> {
    fn decode(&mut self, at: f64, max_size: u32, precision: ThumbnailPrecision) -> Result<DecodedFrame, String> {
        (self.decode)(at, max_size, precision)
    }
}

#[test]
fn ascending_decode_order_slot_mapping_and_duplicate_coalescing() {
    let opens = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(Mutex::new(Vec::<f64>::new()));
    let opens_c = opens.clone();
    let calls_c = calls.clone();
    let opener: TestOpener = Arc::new(move |_, _, max_size, prec| {
        opens_c.fetch_add(1, Ordering::SeqCst);
        assert_eq!(max_size, 160);
        assert_eq!(prec, ThumbnailPrecision::NearestKeyframe);
        let calls = calls_c.clone();
        Ok(Box::new(FnDecoder {
            decode: move |at, _, _| {
                calls.lock().unwrap().push(at);
                let tag = (at * 10.0).round() as u8;
                Ok(solid_frame(tag, 255 - tag, 42, at + 0.01))
            },
            dropped: None,
        }))
    });

    let strip = Thumbnails::request("clip.mp4")
        .times([3.0, 0.5, -2.0, 3.0, -0.0, 0.0, 1.5])
        .max_size(160)
        .precision(ThumbnailPrecision::NearestKeyframe)
        .start_with_decoder(opener);
    strip.wait();

    assert!(strip.is_done());
    assert_eq!(strip.len(), 7);
    assert_eq!(strip.completed(), 7);
    assert_eq!(opens.load(Ordering::SeqCst), 1, "clip opened once per batch");
    assert_eq!(*calls.lock().unwrap(), vec![0.0, 0.5, 1.5, 3.0], "sorted ascending and deduplicated");

    let expected_req: [f64; 7] = [3.0, 0.5, -2.0, 3.0, -0.0, 0.0, 1.5];
    let expected_clamped: [f64; 7] = [3.0, 0.5, 0.0, 3.0, 0.0, 0.0, 1.5];
    for (i, (&req, &clamped)) in expected_req.iter().zip(&expected_clamped).enumerate() {
        let frame = strip.frame(i).unwrap();
        assert_eq!(frame.requested_time().to_bits(), req.to_bits(), "slot {i} preserves requested_time");
        assert!(frame.is_ready(), "slot {i} ready");
        assert_eq!(frame.error(), None);
        assert_eq!(frame.size(), Some((2, 2)));
        assert!((frame.actual_time().unwrap() - (clamped + 0.01)).abs() < 1e-9);
        let tag = (clamped * 10.0).round() as u8;
        let pixel = frame.pixels().with_rgba(|w, h, rgba| {
            assert_eq!((w, h), (2, 2));
            [rgba[0], rgba[1], rgba[2], rgba[3]]
        });
        assert_eq!(pixel, Some([tag, 255 - tag, 42, 255]), "slot {i} pixels mapped accurately");
    }
}

#[test]
fn progressive_waking_waker_catch_up_and_silent_once_done() {
    let step1 = Arc::new(Barrier::new(2));
    let step2 = Arc::new(Barrier::new(2));
    let b1 = step1.clone();
    let b2 = step2.clone();
    let opener: TestOpener = Arc::new(move |_, _, _, _| {
        let b1 = b1.clone();
        let b2 = b2.clone();
        Ok(Box::new(FnDecoder {
            decode: move |at, _, _| {
                if at == 1.0 {
                    b1.wait();
                    b2.wait();
                }
                Ok(solid_frame((at * 50.0) as u8, 10, 20, at))
            },
            dropped: None,
        }))
    });

    let wakes1 = Arc::new(AtomicUsize::new(0));
    let w1 = wakes1.clone();
    let strip = Thumbnails::request("clip.mp4")
        .times([0.0, 1.0, 2.0])
        .waker(Arc::new(move || { w1.fetch_add(1, Ordering::SeqCst); }))
        .start_with_decoder(opener);

    // Wait until frame 0 has landed and the worker reached frame 1.
    step1.wait();
    assert_eq!(strip.completed(), 1);
    assert!(strip.frame(0).unwrap().is_ready());
    assert!(!strip.frame(1).unwrap().is_ready());
    assert!(wakes1.load(Ordering::SeqCst) >= 1, "first frame woke waker progressively");

    // Replacing the waker mid-batch fires the new waker once immediately to catch up.
    let wakes2 = Arc::new(AtomicUsize::new(0));
    let w2 = wakes2.clone();
    strip.set_waker(Arc::new(move || { w2.fetch_add(1, Ordering::SeqCst); }));
    assert_eq!(wakes2.load(Ordering::SeqCst), 1, "WakerSlot catches up once on install");

    step2.wait();
    strip.wait();
    assert!(strip.is_done());
    let after_done = wakes2.load(Ordering::SeqCst);
    assert!(after_done >= 3, "frames 1, 2 and done woke new waker: {after_done}");
    // No further wakes after completion.
    assert_eq!(wakes2.load(Ordering::SeqCst), after_done);
}

#[test]
fn drop_to_cancel_mid_batch_frees_decoder_and_preserves_cloned_thumbnail() {
    let reached_second = Arc::new(Barrier::new(2));
    let resume_second = Arc::new(Barrier::new(2));
    let decoder_dropped = Arc::new((Mutex::new(false), Condvar::new()));
    let decoded_times = Arc::new(Mutex::new(Vec::<f64>::new()));

    let r2 = reached_second.clone();
    let res2 = resume_second.clone();
    let dd = decoder_dropped.clone();
    let dt = decoded_times.clone();

    let opener: TestOpener = Arc::new(move |_, _, _, _| {
        let r2 = r2.clone();
        let res2 = res2.clone();
        let dd = dd.clone();
        let dt = dt.clone();
        Ok(Box::new(FnDecoder {
            decode: move |at, _, _| {
                dt.lock().unwrap().push(at);
                if at == 1.0 {
                    r2.wait();
                    res2.wait();
                }
                Ok(solid_frame(200, 100, 50, at))
            },
            dropped: Some(dd),
        }))
    });

    let settled_wake = Arc::new((Mutex::new(0usize), Condvar::new()));
    let sw = settled_wake.clone();
    let strip = Thumbnails::request("clip.mp4")
        .times([0.0, 1.0, 2.0, 3.0])
        .waker(Arc::new(move || {
            let mut n = sw.0.lock().unwrap();
            *n += 1;
            sw.1.notify_all();
        }))
        .start_with_decoder(opener);
    let cloned_first = strip.frame(0).unwrap().clone();
    let cloned_third = strip.frame(2).unwrap().clone();

    // Wait until frame 0 landed and worker entered frame 1.
    reached_second.wait();
    assert!(cloned_first.is_ready());

    // Drop the parent batch while the worker is mid-batch: Handle::drop cancels immediately.
    drop(strip);
    // Let the in-flight frame 1 decode return so the worker sees cancellation.
    resume_second.wait();

    // Decoder must drop promptly without decoding frames 2 or 3, and finish_done wakes after drop.
    let mut dropped = decoder_dropped.0.lock().unwrap();
    while !*dropped {
        dropped = decoder_dropped.1.wait(dropped).unwrap();
    }
    drop(dropped);
    let mut wakes = settled_wake.0.lock().unwrap();
    while cloned_third.error().is_none() {
        wakes = settled_wake.1.wait(wakes).unwrap();
    }
    drop(wakes);
    assert_eq!(*decoded_times.lock().unwrap(), vec![0.0, 1.0], "frames 2 and 3 never decoded");

    // Cloned Thumbnail for frame 0 stays readable after parent Thumbnails dropped.
    assert!(cloned_first.is_ready());
    assert_eq!(cloned_first.actual_time(), Some(0.0));
    assert_eq!(cloned_first.size(), Some((2, 2)));
    assert_eq!(cloned_first.pixels().with_rgba(|_, _, b| [b[0], b[1], b[2], b[3]]), Some([200, 100, 50, 255]));

    // Unreached slot settled with cancelled error, never a blank ready frame.
    assert!(!cloned_third.is_ready());
    assert_eq!(cloned_third.error(), Some("cancelled"));
    assert_eq!(cloned_third.size(), None);
}

#[test]
fn explicit_cancel_settles_remaining_slots_and_unblocks_wait() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let e = entered.clone();
    let r = release.clone();
    let opener: TestOpener = Arc::new(move |_, _, _, _| {
        let e = e.clone();
        let r = r.clone();
        Ok(Box::new(FnDecoder {
            decode: move |at, _, _| {
                if at == 1.0 {
                    e.wait();
                    r.wait();
                }
                Ok(solid_frame(10, 20, 30, at))
            },
            dropped: None,
        }))
    });

    let strip = Thumbnails::request("clip.mp4").times([0.0, 1.0, 2.0]).start_with_decoder(opener);
    entered.wait();
    strip.cancel();
    release.wait();
    strip.wait();

    assert!(strip.is_done());
    assert_eq!(strip.completed(), 3);
    assert!(strip.frame(0).unwrap().is_ready());
    assert_eq!(strip.frame(1).unwrap().error(), Some("cancelled"));
    assert_eq!(strip.frame(2).unwrap().error(), Some("cancelled"));
}

#[test]
fn worker_pool_caps_concurrent_decoders_at_four_under_twenty_batches() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let started_any = Arc::new(AtomicBool::new(false));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));

    let mut batches = Vec::with_capacity(20);
    for i in 0..20 {
        let active = active.clone();
        let peak = peak.clone();
        let started_any = started_any.clone();
        let gate = gate.clone();
        let opener: TestOpener = Arc::new(move |_, _, _, _| {
            let cur = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(cur, Ordering::SeqCst);
            started_any.store(true, Ordering::Release);
            // Wait briefly for the main test thread to finish enqueueing all 20 batches.
            let mut open = gate.0.lock().unwrap();
            while !*open {
                open = gate.1.wait(open).unwrap();
            }
            drop(open);
            let active_drop = active.clone();
            struct ActiveGuard(Arc<AtomicUsize>);
            impl Drop for ActiveGuard {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let guard = ActiveGuard(active_drop);
            Ok(Box::new(FnDecoder {
                decode: move |at, _, _| {
                    let _keep = &guard;
                    Ok(solid_frame(i as u8, 50, 100, at))
                },
                dropped: None,
            }))
        });
        batches.push(Thumbnails::request(format!("clip-{i}.mp4")).at(0.0).start_with_decoder(opener));
    }

    // Release all queued workers and wait for every batch to finish.
    {
        let mut open = gate.0.lock().unwrap();
        *open = true;
        gate.1.notify_all();
    }
    for (i, batch) in batches.iter().enumerate() {
        batch.wait();
        assert!(batch.is_done(), "batch {i} done");
        assert!(batch.frame(0).unwrap().is_ready(), "batch {i} ready");
    }
    assert!(started_any.load(Ordering::Acquire));
    assert_eq!(active.load(Ordering::SeqCst), 0, "all workers exited cleanly");
    let observed_peak = peak.load(Ordering::SeqCst);
    assert!((1..=4).contains(&observed_peak), "concurrent decoder bound exceeded: {observed_peak}");
}

#[test]
fn edge_cases_empty_non_finite_max_size_headers_and_errors() {
    // Empty batch settles immediately without opening decoder.
    let opens = Arc::new(AtomicUsize::new(0));
    let wakes = Arc::new(AtomicUsize::new(0));
    let o = opens.clone();
    let w = wakes.clone();
    let empty = Thumbnails::request("clip.mp4")
        .waker(Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); }))
        .start_with_decoder(Arc::new(move |_, _, _, _| {
            o.fetch_add(1, Ordering::SeqCst);
            Err("should not open".into())
        }));
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    assert_eq!(empty.completed(), 0);
    assert!(empty.is_done());
    assert_eq!(opens.load(Ordering::SeqCst), 0);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    empty.wait();

    // All non-finite timestamps fail individually without opening decoder.
    let opens = Arc::new(AtomicUsize::new(0));
    let o = opens.clone();
    let bad = Thumbnails::request("clip.mp4")
        .times([f64::NAN, f64::INFINITY, f64::NEG_INFINITY])
        .start_with_decoder(Arc::new(move |_, _, _, _| {
            o.fetch_add(1, Ordering::SeqCst);
            Err("should not open".into())
        }));
    assert!(bad.is_done());
    assert_eq!(bad.completed(), 3);
    assert_eq!(opens.load(Ordering::SeqCst), 0);
    for f in bad.frames() {
        assert!(!f.is_ready());
        assert_eq!(f.error(), Some("invalid timestamp"));
        assert_eq!(f.size(), None);
        assert_eq!(f.actual_time(), None);
    }

    // Mixed non-finite and valid timestamps decode only the valid ones.
    let mixed = Thumbnails::request("clip.mp4")
        .times([f64::NAN, 1.0, f64::INFINITY])
        .start_with_decoder(Arc::new(|_, _, _, _| {
            Ok(Box::new(FnDecoder { decode: |at, _, _| Ok(solid_frame(1, 2, 3, at)), dropped: None }))
        }));
    mixed.wait();
    assert_eq!(mixed.frame(0).unwrap().error(), Some("invalid timestamp"));
    assert!(mixed.frame(1).unwrap().is_ready());
    assert_eq!(mixed.frame(2).unwrap().error(), Some("invalid timestamp"));

    // max_size clamps 0 -> 1, >8192 -> 8192, default -> 320.
    assert_eq!(ThumbnailRequest::new("a.mp4").max_size_value(), 320);
    assert_eq!(ThumbnailRequest::new("a.mp4").max_size(0).max_size_value(), 1);
    assert_eq!(ThumbnailRequest::new("a.mp4").max_size(99_999).max_size_value(), 8192);

    // Debug redacts header values; MediaRequest::thumbnails carries URL and headers.
    let req = Player::request("https://example.com/clip.mp4")
        .header("Authorization", "Bearer super-secret-value")
        .header("authorization", "Bearer replaced-secret")
        .thumbnails()
        .header("X-Custom", "hidden-token");
    assert_eq!(*req.source_value(), MediaSource::Url("https://example.com/clip.mp4".into()));
    assert_eq!(req.headers_value().len(), 2);
    let debug = format!("{req:?}");
    assert!(debug.contains("authorization") || debug.contains("Authorization"), "{debug}");
    assert!(debug.contains("X-Custom"), "{debug}");
    assert!(!debug.contains("super-secret-value"), "{debug}");
    assert!(!debug.contains("replaced-secret"), "{debug}");
    assert!(!debug.contains("hidden-token"), "{debug}");

    // Open failure settles all slots once with the error and never writes blank pixels.
    let fail_wakes = Arc::new(AtomicUsize::new(0));
    let fw = fail_wakes.clone();
    let failed = Thumbnails::request("missing.mp4")
        .times([0.0, 1.0, 2.0])
        .waker(Arc::new(move || { fw.fetch_add(1, Ordering::SeqCst); }))
        .start_with_decoder(Arc::new(|_, _, _, _| Err("open exploded".into())));
    failed.wait();
    assert!(failed.is_done());
    assert_eq!(failed.completed(), 3);
    assert_eq!(fail_wakes.load(Ordering::SeqCst), 1);
    for f in failed.frames() {
        assert!(!f.is_ready());
        assert_eq!(f.error(), Some("open exploded"));
        assert_eq!(f.size(), None);
        assert_eq!(f.pixels().size(), (0, 0));
    }

    // Per-frame error settles that slot while later slots still succeed.
    let partial = Thumbnails::request("clip.mp4")
        .times([0.0, 1.0])
        .start_with_decoder(Arc::new(|_, _, _, _| {
            Ok(Box::new(FnDecoder {
                decode: |at, _, _| if at == 0.0 { Err("bad frame".into()) } else { Ok(solid_frame(9, 8, 7, at)) },
                dropped: None,
            }))
        }));
    partial.wait();
    assert_eq!(partial.frame(0).unwrap().error(), Some("bad frame"));
    assert!(partial.frame(1).unwrap().is_ready());

    // Unsupported-platform settlement path produces clean errors without spawning threads.
    let unsup_wakes = Arc::new(AtomicUsize::new(0));
    let uw = unsup_wakes.clone();
    let unsup = Thumbnails::request("clip.mp4")
        .times([0.0, 1.0])
        .waker(Arc::new(move || { uw.fetch_add(1, Ordering::SeqCst); }))
        .start_as_unsupported();
    assert!(unsup.is_done());
    assert_eq!(unsup.completed(), 2);
    assert_eq!(unsup_wakes.load(Ordering::SeqCst), 1);
    for f in unsup.frames() {
        assert!(!f.is_ready());
        assert_eq!(f.error(), Some("video thumbnails aren't supported on this platform yet"));
        assert_eq!(f.size(), None);
    }
    unsup.wait();
}

fn reference_wgsl_convert(y_u8: u8, cb_u8: u8, cr_u8: u8, format: Format, color: ColorInfo) -> [u8; 4] {
    let mut y = y_u8 as f32 / 255.0;
    let mut cb = cb_u8 as f32 / 255.0 - 128.0 / 255.0;
    let mut cr = cr_u8 as f32 / 255.0 - 128.0 / 255.0;
    if format == Format::Nv12Video {
        y = (y - 16.0 / 255.0) * (255.0 / 219.0);
        cb *= 255.0 / 224.0;
        cr *= 255.0 / 224.0;
    }
    let (kr, kb) = match color.matrix {
        Matrix::Bt601 => (0.299f32, 0.114f32),
        Matrix::Bt709 => (0.2126f32, 0.0722f32),
    };
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    let enc = |v: f32| -> u8 {
        let x = v.clamp(0.0, 1.0);
        let out = match color.transfer {
            Transfer::Srgb => x,
            Transfer::Bt709 => {
                let lin = if x < 0.081 { x / 4.5 } else { ((x + 0.099) / 1.099).powf(1.0 / 0.45) }.clamp(0.0, 1.0);
                if lin <= 0.003_130_8 { lin * 12.92 } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 }
            }
        };
        (out.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [enc(r), enc(g), enc(b), 255]
}

#[test]
fn nv12_color_conversions_match_video_wgsl_equations() {
    let samples = [
        (16u8, 128u8, 128u8),  // limited black
        (235, 128, 128),       // limited white
        (0, 128, 128),         // full black
        (255, 128, 128),       // full white
        (81, 90, 240),         // vivid red
        (145, 54, 34),         // vivid green
        (41, 240, 110),        // vivid blue
        (126, 160, 96),        // mid-tone teal
    ];
    for format in [Format::Nv12Video, Format::Nv12Full] {
        for matrix in [Matrix::Bt601, Matrix::Bt709] {
            for transfer in [Transfer::Srgb, Transfer::Bt709] {
                let color = ColorInfo { matrix, transfer };
                for &(y, cb, cr) in &samples {
                    let frame = ByteFrame {
                        width: 2,
                        height: 2,
                        format,
                        color,
                        planes: [vec![y; 4], vec![cb, cr]],
                    };
                    let (w, h, rgba) = byte_frame_to_rgba(&frame, Orient::UP).unwrap();
                    assert_eq!((w, h), (2, 2));
                    let want = reference_wgsl_convert(y, cb, cr, format, color);
                    for px in rgba.chunks_exact(4) {
                        for c in 0..4 {
                            assert!(
                                px[c].abs_diff(want[c]) <= 1,
                                "{format:?} {color:?} y={y} cb={cb} cr={cr}: got {px:?} != {want:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn nv12_applies_all_eight_orient_transforms() {
    // 4x2 grayscale full-range NV12 where each buffer pixel has a distinct luma value.
    let (bw, bh) = (4usize, 2usize);
    let luma: Vec<u8> = vec![10, 40, 70, 100, 130, 160, 190, 220];
    let chroma: Vec<u8> = vec![128; bw * (bh / 2)];
    let frame = ByteFrame {
        width: bw as u32,
        height: bh as u32,
        format: Format::Nv12Full,
        color: ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Srgb },
        planes: [luma.clone(), chroma],
    };
    let all_orients = [
        Orient([1, 0, 0, 1]),   // UP
        Orient([0, 1, -1, 0]),  // 90
        Orient([-1, 0, 0, -1]), // 180
        Orient([0, -1, 1, 0]),  // 270
        Orient([-1, 0, 0, 1]),  // flip H
        Orient([1, 0, 0, -1]),  // flip V
        Orient([0, 1, 1, 0]),   // transpose
        Orient([0, -1, -1, 0]), // transverse
    ];
    for orient in all_orients {
        let (uw, uh, rgba) = byte_frame_to_rgba(&frame, orient).unwrap();
        assert_eq!([uw, uh], orient.upright([bw as u32, bh as u32]));
        let [m0, m1, m2, m3] = orient.0.map(f32::from);
        for y in 0..uh as usize {
            for x in 0..uw as usize {
                let ux = (x as f32 + 0.5) - uw as f32 * 0.5;
                let uy = (y as f32 + 0.5) - uh as f32 * 0.5;
                let bx = (m0 * ux + m1 * uy + bw as f32 * 0.5).floor() as usize;
                let by = (m2 * ux + m3 * uy + bh as f32 * 0.5).floor() as usize;
                let want = luma[by * bw + bx];
                let px = &rgba[(y * uw as usize + x) * 4..][..4];
                assert_eq!(px, &[want, want, want, 255], "orient={orient:?} at ({x},{y}) -> ({bx},{by})");
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn unsupported_platform_settles_immediately_with_error_and_no_blank_image() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let strip = Thumbnails::request("clip.mp4")
        .times([0.0, 1.0])
        .waker(Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); }))
        .start();
    assert!(strip.is_done());
    assert_eq!(strip.completed(), 2);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    for f in strip.frames() {
        assert!(!f.is_ready());
        assert_eq!(f.error(), Some("video thumbnails aren't supported on this platform yet"));
        assert_eq!(f.size(), None);
        assert_eq!(f.pixels().size(), (0, 0));
    }
    strip.wait();
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rune/tests/fixtures").join(name)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn real_mp4_rgbw_exact_downscale_keyframe_and_clamping() {
    let path = fixture_path("rgbw.mp4");
    let strip = Thumbnails::request(path.as_path())
        .times([0.0, 0.25, 0.5, 0.75])
        .max_size(320)
        .precision(ThumbnailPrecision::Exact)
        .start();
    strip.wait();
    assert!(strip.is_done());
    assert_eq!(strip.completed(), 4);

    let expected_times = [0.0, 0.25, 0.5, 0.75];
    let mut natural = (0u32, 0u32);
    for (i, &want_t) in expected_times.iter().enumerate() {
        let frame = strip.frame(i).unwrap();
        assert!(frame.is_ready(), "frame {i} failed: {:?}", frame.error());
        assert_eq!(frame.error(), None);
        let (w, h) = frame.size().unwrap();
        assert!(w > 0 && h > 0 && w.max(h) < 320, "never upscaled to 320: {w}x{h}");
        natural = (w, h);
        let actual = frame.actual_time().unwrap();
        assert!((actual - want_t).abs() < 0.05, "frame {i}: actual={actual} want={want_t}");
        let center = frame.pixels().with_rgba(|rw, rh, rgba| {
            assert_eq!((rw, rh), (w, h));
            let idx = ((rh / 2) * rw + (rw / 2)) as usize * 4;
            [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
        }).unwrap();
        assert_eq!(center[3], 255, "opaque alpha");
        match i {
            0 => assert!(center[0] > 200 && center[1] < 50 && center[2] < 50, "frame 0 red: {center:?}"),
            1 => assert!(center[0] < 50 && center[1] > 200 && center[2] < 50, "frame 1 green: {center:?}"),
            2 => assert!(center[0] < 50 && center[1] < 50 && center[2] > 200, "frame 2 blue: {center:?}"),
            3 => assert!(center[0] > 200 && center[1] > 200 && center[2] > 200, "frame 3 white: {center:?}"),
            _ => unreachable!(),
        }
    }

    // Downscaling at max_size = 32 shrinks longest side to 32 while preserving aspect ratio.
    let small = Thumbnails::request(path.as_path()).at(0.25).max_size(32).start();
    small.wait();
    let f = small.frame(0).unwrap();
    assert!(f.is_ready(), "{:?}", f.error());
    let (sw, sh) = f.size().unwrap();
    assert_eq!(sw.max(sh), 32, "downscaled longest side to 32 (natural was {natural:?}, got {sw}x{sh})");
    let px = f.pixels().with_rgba(|rw, rh, rgba| {
        let idx = ((rh / 2) * rw + (rw / 2)) as usize * 4;
        [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
    }).unwrap();
    assert!(px[0] < 50 && px[1] > 200 && px[2] < 50 && px[3] == 255, "downscaled green: {px:?}");

    // NearestKeyframe succeeds with a valid frame and actual_time <= requested.
    let kf = Thumbnails::request(path.as_path())
        .times([0.0, 0.5, 10.0])
        .precision(ThumbnailPrecision::NearestKeyframe)
        .start();
    kf.wait();
    for (i, frame) in kf.frames().iter().enumerate() {
        assert!(frame.is_ready(), "keyframe slot {i} failed: {:?}", frame.error());
        assert_eq!(frame.size(), Some(natural));
        assert!(frame.actual_time().unwrap() <= frame.requested_time() + 0.05);
    }

    // Negative clamps to 0.0 (red) and past-duration clamps to the last picture (white, ~0.75 s).
    let clamped = Thumbnails::request(path.as_path()).times([-5.0, 10.0]).start();
    clamped.wait();
    let first = clamped.frame(0).unwrap();
    assert!(first.is_ready(), "{:?}", first.error());
    assert!(first.actual_time().unwrap().abs() < 0.05);
    let c0 = first.pixels().with_rgba(|rw, rh, rgba| {
        let idx = ((rh / 2) * rw + (rw / 2)) as usize * 4;
        [rgba[idx], rgba[idx + 1], rgba[idx + 2]]
    }).unwrap();
    assert!(c0[0] > 200 && c0[1] < 50 && c0[2] < 50, "clamped -5.0 -> red: {c0:?}");

    let last = clamped.frame(1).unwrap();
    assert!(last.is_ready(), "{:?}", last.error());
    assert!((last.actual_time().unwrap() - 0.75).abs() < 0.05, "{:?}", last.actual_time());
    let c1 = last.pixels().with_rgba(|rw, rh, rgba| {
        let idx = ((rh / 2) * rw + (rw / 2)) as usize * 4;
        [rgba[idx], rgba[idx + 1], rgba[idx + 2]]
    }).unwrap();
    assert!(c1[0] > 200 && c1[1] > 200 && c1[2] > 200, "clamped 10.0 -> white: {c1:?}");
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn real_mp4_quadrants_rot90_decodes_upright_with_four_distinct_quadrants() {
    let path = fixture_path("quadrants-rot90.mp4");
    let strip = Thumbnails::request(path.as_path()).at(0.125).max_size(320).start();
    strip.wait();
    let frame = strip.frame(0).unwrap();
    assert!(frame.is_ready(), "{:?}", frame.error());
    assert_eq!(frame.size(), Some((32, 64)), "portrait upright dimensions after rotate=90");
    let quadrants = frame.pixels().with_rgba(|w, h, rgba| {
        assert_eq!((w, h), (32, 64));
        [(0u32, 0u32), (1, 0), (0, 1), (1, 1)]
            .map(|(qx, qy)| {
                let x = qx * 16 + 8;
                let y = qy * 32 + 16;
                let idx = (y * w + x) as usize * 4;
                [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
            })
    }).unwrap();
    for q in &quadrants {
        assert_eq!(q[3], 255);
    }
    // Every quadrant must be distinct (clear separation in at least one RGB channel).
    for i in 0..4 {
        for j in (i + 1)..4 {
            let diff = (0..3).map(|c| quadrants[i][c].abs_diff(quadrants[j][c])).max().unwrap();
            assert!(diff > 50, "quadrants {i} and {j} should be distinct: {quadrants:?}");
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn real_audio_only_missing_and_headers_fail_cleanly_without_blank_images() {
    for name in ["silence.m4a", "silence.wav"] {
        let strip = Thumbnails::request(fixture_path(name)).times([0.0, 0.5]).start();
        strip.wait();
        assert!(strip.is_done());
        assert_eq!(strip.completed(), 2);
        for f in strip.frames() {
            assert!(!f.is_ready(), "{name} should not produce a video thumbnail");
            assert!(f.error().is_some(), "{name} should report an error");
            assert_eq!(f.size(), None);
            assert_eq!(f.pixels().size(), (0, 0));
        }
    }

    let missing = Thumbnails::request(fixture_path("does-not-exist.mp4")).at(0.0).start();
    missing.wait();
    assert!(!missing.frame(0).unwrap().is_ready());
    assert!(missing.frame(0).unwrap().error().is_some());

    #[cfg(target_os = "windows")]
    {
        let headers = Player::request("https://example.com/clip.mp4")
            .header("Authorization", "Bearer secret")
            .thumbnails()
            .at(0.0)
            .start();
        headers.wait();
        assert!(!headers.frame(0).unwrap().is_ready());
        assert!(headers.frame(0).unwrap().error().unwrap().contains("headers"));
    }
}
