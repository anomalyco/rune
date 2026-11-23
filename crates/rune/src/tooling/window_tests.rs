#[test]
fn transparent_png_unpremultiplies_in_linear_space() {
    let mut pixels = [188, 0, 0, 128, 255, 255, 255, 0, 12, 34, 56, 255];
    super::straight_alpha(&mut pixels);
    assert_eq!(pixels, [255, 0, 0, 128, 0, 0, 0, 0, 12, 34, 56, 255]);
}

#[test]
fn headless_snap_after_uncaptured_frames_paces_and_writes_pixels() {
    let _turn = super::tests::HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let snap = std::env::temp_dir().join(format!("rune-paced-snap-{}.png", std::process::id()));
    let spec = crate::AppSpec::new("paced", (32.0, 20.0), || (), |_, _| {
        crate::el().w_full().h_full().fill(crate::oklch(0.7, 0.15, 220.0))
    });
    // 13 frames at 60 fps: frames 0..12 submit without readback (wrapping InFlight::DEPTH several
    // times) and frame 12 reads back and writes the PNG.
    super::headless(crate::App::new(spec), super::Args {
        snap: Some(snap.clone()),
        at: Some(0.2),
        fps: 60.0,
        scale: Some(1.0),
        ..Default::default()
    });
    let img = image::open(&snap).unwrap().into_rgba8();
    std::fs::remove_file(&snap).unwrap();
    assert_eq!(img.dimensions(), (32, 20));
    assert!(img.pixels().all(|p| p[3] == 255 && p[2] > p[0]), "snap rendered the filled canvas");
}
