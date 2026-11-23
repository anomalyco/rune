#[test]
fn transparent_png_unpremultiplies_in_linear_space() {
    let mut pixels = [188, 0, 0, 128, 255, 255, 255, 0, 12, 34, 56, 255];
    super::straight_alpha(&mut pixels);
    assert_eq!(pixels, [255, 0, 0, 128, 0, 0, 0, 0, 12, 34, 56, 255]);
}

#[test]
fn headless_snap_after_uncaptured_frames_paces_and_writes_pixels() {
    let _turn = super::headless_run::test_turn();
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

#[test]
fn chrome_overlay_mirrors_traffic_lights_and_overlap_zone_in_rtl() {
    let width = 400.0;
    let chrome = crate::Titlebar::new(36.0).mac_chrome(crate::Chrome::DEFAULT);
    let inset = chrome.titlebar_inset();

    let ltr = chrome.traffic_light_rects_in(width, false).expect("LTR traffic lights");
    assert_eq!(ltr, chrome.traffic_light_rects().unwrap());
    assert_eq!(ltr[0][0], 16.0);
    assert_eq!(ltr[2][0] + ltr[2][2], inset);

    let rtl = chrome.traffic_light_rects_in(width, true).expect("RTL traffic lights");
    // Close sits outermost on the right, Zoom innermost on the left, matching macOS RTL insets.
    assert_eq!(rtl[0], [width - 16.0 - crate::Chrome::TRAFFIC_LIGHT, 11.0, crate::Chrome::TRAFFIC_LIGHT, crate::Chrome::TRAFFIC_LIGHT]);
    assert_eq!(rtl[1], [width - 39.0 - crate::Chrome::TRAFFIC_LIGHT, 11.0, crate::Chrome::TRAFFIC_LIGHT, crate::Chrome::TRAFFIC_LIGHT]);
    assert_eq!(rtl[2], [width - inset, 11.0, crate::Chrome::TRAFFIC_LIGHT, crate::Chrome::TRAFFIC_LIGHT]);
    assert_eq!(width - rtl[2][0], inset, "RTL traffic lights must match trailing chrome inset");

    let standard = crate::Chrome { full_size_content: false, traffic_lights: None, drag_regions: false };
    assert!(standard.traffic_light_rects_in(width, true).is_none());

    // Paint into a blank 400x40 frame at 1x and verify discs and all four clearance-box edges.
    let (w, h) = (400u32, 40u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for p in px.chunks_exact_mut(4) { p[3] = 255; }
    super::paint_traffic_lights(&mut px, w, h, 1.0, &rtl);
    let at = |px: &[u8], x: u32, y: u32| -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    // Close (red) on the right, Minimize (yellow) middle, Zoom (green) on the left.
    let close = at(&px, (rtl[0][0] + 7.0) as u32, 18);
    let mini = at(&px, (rtl[1][0] + 7.0) as u32, 18);
    let zoom = at(&px, (rtl[2][0] + 7.0) as u32, 18);
    assert!(close[0] > 200 && close[1] < 120 && close[2] < 120, "expected red close disc on the right: {close:?}");
    assert!(mini[0] > 200 && mini[1] > 150 && mini[2] < 80, "expected yellow minimize disc in the middle: {mini:?}");
    assert!(zoom[0] < 80 && zoom[1] > 170 && zoom[2] < 100, "expected green zoom disc on the left: {zoom:?}");
    // Cyan 4 pt clearance box on top, bottom, left and right edges around the RTL lights.
    let is_cyan = |c: [u8; 3]| c[0] == 0 && c[1] > 140 && c[2] > 180;
    assert!(is_cyan(at(&px, (rtl[1][0] + 7.0) as u32, 7)), "top cyan clearance edge missing");
    assert!(is_cyan(at(&px, (rtl[1][0] + 7.0) as u32, 29)), "bottom cyan clearance edge missing");
    assert!(is_cyan(at(&px, (rtl[2][0] - 4.0) as u32, 18)), "left cyan clearance edge missing");
    assert!(is_cyan(at(&px, (rtl[0][0] + rtl[0][2] + 4.0) as u32, 18)), "right cyan clearance edge missing");
    // Left edge (where LTR lights would be) remains untouched.
    assert_eq!(at(&px, 23, 18), [0, 0, 0], "LTR close position must stay unpainted in RTL");

    // Overlap check flags elements on the right (inside the RTL clearance zone), not on the left.
    let view = |&(w, ins): &(f32, f32), _: &mut crate::Cx<(f32, f32)>| {
        crate::el().w_full().h_full()
            .child(crate::el().key("left-btn").role("button").absolute(16.0, 10.0).size(40.0, 20.0))
            .child(crate::el().key("clear-btn").role("button").absolute(w - ins - 44.0, 10.0).size(40.0, 20.0))
            .child(crate::el().key("right-btn").role("button").absolute(w - ins + 4.0, 10.0).size(40.0, 20.0))
    };
    let mut rt = crate::Runtime::new((width, inset), view, (width, 40.0), crate::Env::new(true, 1.0));
    rt.frame(0.0);

    let rtl_hits = super::traffic_light_overlaps(rt.core(), &rtl);
    assert_eq!(rtl_hits.len(), 1, "RTL overlap check should flag only right-btn: {rtl_hits:?}");
    assert!(rtl_hits[0].0.contains("right-btn"), "expected right-btn overlap, got {rtl_hits:?}");

    let ltr_hits = super::traffic_light_overlaps(rt.core(), &ltr);
    assert_eq!(ltr_hits.len(), 1, "LTR overlap check should flag only left-btn: {ltr_hits:?}");
    assert!(ltr_hits[0].0.contains("left-btn"), "expected left-btn overlap, got {ltr_hits:?}");
}

#[test]
fn headless_chrome_overlay_paints_nothing_over_borderless_window() {
    let _turn = super::headless_run::test_turn();
    let snap = std::env::temp_dir().join(format!("rune-borderless-chrome-{}.png", std::process::id()));
    let run = |chrome: crate::Chrome, titlebar: Option<crate::Titlebar>| {
        let spec = crate::AppSpec::new("borderless", (120.0, 40.0), || (), |_, _| {
            crate::el().w_full().h_full().fill(crate::Color::BLACK)
        });
        let mut app = crate::App::new(spec)
            .chrome(chrome)
            .window_options(crate::WindowOptions { borderless: true, ..Default::default() });
        if let Some(titlebar) = titlebar {
            app = app.titlebar(titlebar);
        }
        super::headless(app, super::Args {
            snap: Some(snap.clone()),
            chrome_overlay: true,
            scale: Some(1.0),
            ..Default::default()
        });
        let img = image::open(&snap).unwrap().into_rgba8();
        std::fs::remove_file(&snap).unwrap();
        img
    };

    let custom = crate::Chrome { full_size_content: true, traffic_lights: Some((16.0, 12.0)), drag_regions: true };
    for (label, chrome, titlebar) in [
        ("default chrome", crate::Chrome::DEFAULT, None),
        ("explicit traffic lights and titlebar", custom, Some(crate::Titlebar::new(36.0))),
    ] {
        let img = run(chrome, titlebar);
        assert_eq!(img.dimensions(), (120, 40));
        let painted = img.pixels().find(|p| p.0 != [0, 0, 0, 255]);
        assert!(painted.is_none(), "{label}: borderless --chrome must paint no traffic lights, found {painted:?}");
    }
}

#[test]
fn headless_chrome_overlay_follows_dynamic_titlebar_changes() {
    let _turn = super::headless_run::test_turn();
    let dir = std::env::temp_dir().join(format!("rune-dynamic-chrome-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    struct State { phase: usize }
    fn poll(s: &mut State, t: f64, _: &mut crate::Tx) -> bool {
        let next = (t * 10.0).round() as usize;
        if next != s.phase { s.phase = next; true } else { false }
    }
    fn view(s: &State, cx: &mut crate::Cx<State>) -> crate::El {
        match s.phase {
            1 => cx.titlebar_height(72.0),
            2 => cx.set_titlebar(crate::Titlebar::new(72.0).traffic_lights(30.0, 52.0)),
            _ => {}
        }
        crate::el().w_full().h_full().fill(crate::Color::BLACK)
    }

    let mut spec = crate::AppSpec::new("dynamic-titlebar", (300.0, 100.0), || State { phase: 0 }, view);
    spec.poll = Some(poll);
    let app = crate::App::new(spec).titlebar(crate::Titlebar::new(36.0));
    super::headless(app, super::Args {
        out: Some(dir.clone()),
        frames: Some(4),
        every: 1,
        fps: 10.0,
        chrome_overlay: true,
        scale: Some(1.0),
        ..Default::default()
    });

    let at = |img: &image::RgbaImage, x: u32, y: u32| -> [u8; 3] {
        let p = img.get_pixel(x, y);
        [p[0], p[1], p[2]]
    };
    let is_red = |c: [u8; 3]| c[0] > 200 && c[1] < 120 && c[2] < 120;
    let centers = [(23u32, 18u32), (23, 36), (37, 59)];
    let expected = [
        (0, 0, "frame 0 default App::titlebar(36)"),
        (1, 1, "frame 1 Cx::titlebar_height(72)"),
        (2, 2, "frame 2 Cx::set_titlebar((30, 52))"),
        (3, 0, "frame 3 cleared back to App::titlebar(36)"),
    ];
    for (frame, want, label) in expected {
        let img = image::open(dir.join(format!("frame_{frame:04}.png"))).unwrap().into_rgba8();
        for (i, &(x, y)) in centers.iter().enumerate() {
            let c = at(&img, x, y);
            if i == want {
                assert!(is_red(c), "{label}: expected red close disc at ({x}, {y}), got {c:?}");
            } else {
                assert_eq!(c, [0, 0, 0], "{label}: unexpected paint at old/other center ({x}, {y}): {c:?}");
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();

    // --snap on a later frame paints the updated titlebar too.
    let snap = std::env::temp_dir().join(format!("rune-dynamic-chrome-snap-{}.png", std::process::id()));
    let mut spec = crate::AppSpec::new("dynamic-titlebar-snap", (300.0, 100.0), || State { phase: 0 }, view);
    spec.poll = Some(poll);
    super::headless(crate::App::new(spec).titlebar(crate::Titlebar::new(36.0)), super::Args {
        snap: Some(snap.clone()),
        at: Some(0.2),
        fps: 10.0,
        chrome_overlay: true,
        scale: Some(1.0),
        ..Default::default()
    });
    let img = image::open(&snap).unwrap().into_rgba8();
    std::fs::remove_file(&snap).unwrap();
    assert!(is_red(at(&img, 37, 59)), "snap at phase 2 expected red close disc at (37, 59), got {:?}", at(&img, 37, 59));
    assert_eq!(at(&img, 23, 18), [0, 0, 0], "snap at phase 2 must not paint old close disc at (23, 18)");

    // Overlap check uses the runtime's current headless chrome on each frame.
    fn overlap_view(s: &State, cx: &mut crate::Cx<State>) -> crate::El {
        match s.phase {
            1 => cx.titlebar_height(72.0),
            2 => cx.set_titlebar(crate::Titlebar::new(72.0).traffic_lights(30.0, 52.0)),
            _ => {}
        }
        crate::el().w_full().h_full()
            .child(crate::el().key("default-btn").role("button").absolute(18.0, 12.0).size(12.0, 12.0))
            .child(crate::el().key("tall-btn").role("button").absolute(18.0, 30.0).size(12.0, 12.0))
            .child(crate::el().key("custom-btn").role("button").absolute(32.0, 54.0).size(12.0, 12.0))
    }
    let mut rt = crate::Runtime::new(State { phase: 0 }, overlap_view, (300.0, 100.0), crate::Env::new(true, 1.0));
    rt.headless_chrome(crate::Chrome::DEFAULT, Some(crate::Titlebar::new(36.0)), false);
    for (phase, want) in [(0, "default-btn"), (1, "tall-btn"), (2, "custom-btn"), (3, "default-btn")] {
        rt.update(move |s, _| s.phase = phase);
        rt.frame(0.1);
        let lights = rt.core().headless_chrome_current().traffic_light_rects_in(300.0, false).expect("traffic lights");
        let hits = super::traffic_light_overlaps(rt.core(), &lights);
        assert_eq!(hits.len(), 1, "phase {phase}: expected only {want} to overlap {lights:?}, got {hits:?}");
        assert!(hits[0].0.contains(want), "phase {phase}: expected {want}, got {hits:?}");
    }
}
