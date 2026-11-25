//! Offscreen deterministic frame loop, timeline resolution and `--chrome` traffic-light checks.

use std::sync::Arc;

use crate::app::SharedCore;
use crate::render::{Gpu, OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR, Offscreen, Renderer};
use crate::runtime::RuntimeCore;
use crate::time::Instant;

use super::*;

#[path = "headless/app.rs"]
mod app;
#[path = "headless/frame.rs"]
mod frame;
use app::{HeadlessApp, TypedHeadlessApp};
pub(in crate::tooling) use frame::{step_window_frame, take_window_frame};
use frame::{FrameShot, HeadlessFrame, HeadlessLoop};

#[cfg(test)]
pub(crate) use crate::render::surface_alpha::straight_alpha;

struct HeadlessPrep {
    source: Option<Value>,
    script: Vec<Ev>,
    timeline: Timeline,
    scale: f32,
    size: (f32, f32),
    dark: bool,
    writer: crate::tooling::record::Writer,
}

impl HeadlessPrep {
    fn begin(args: &Args, spec_size: (f32, f32)) -> Self {
        #[cfg(all(windows, feature = "frame-profile"))]
        let _ = crate::frame::host_clock::cycles_per_ns();
        crate::tooling::interaction::gate::headless();
        let source = read_script(args);
        let script = source.as_ref().map(load_script).unwrap_or_default();
        let timeline = Timeline::of(args, &script).unwrap_or_else(|e| die(&e));
        let scale = args.scale.unwrap_or(2.0);
        let size = args.size.unwrap_or(spec_size);
        // Check the encoder before app initialization or GPU work.
        let mut writer = crate::tooling::record::Writer::new(((size.0 * scale).round() as u32, (size.1 * scale).round() as u32));
        if let Some(path) = &args.video {
            writer.video(path, timeline.fps).unwrap_or_else(|e| die(&e));
        }
        crate::tooling::headless_run::publish(crate::tooling::headless_run::HeadlessRun {
            script: args.script.clone(),
            events: script.iter().map(Ev::public).collect(),
            scale,
            fps: timeline.fps,
            size,
            start: timeline.start_frame as f64 * timeline.dt,
            frames: timeline.frames,
            end: timeline.last_frame as f64 * timeline.dt,
            desktop: None,
            windows: Default::default(),
        });
        let dark = args.theme.as_deref() != Some("light");
        Self { source, script, timeline, scale, size, dark, writer }
    }
}

struct HeadlessConfig {
    launched: Instant,
    state_ms: f64,
    args: Args,
    transparent: bool,
    chrome: Chrome,
    titlebar: Option<crate::Titlebar>,
    fit_height: bool,
    app_options: crate::WindowOptions,
    min_size: Option<(f32, f32)>,
    on_ready: Option<crate::app::OnWindow>,
    on_frame_encoded: Option<crate::frame::encoded::OnFrameEncoded>,
    shared_core: Arc<SharedCore>,
    shortcuts_core: Arc<crate::shortcuts::HubCore>,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::MenuGuard,
}

fn wait_for_profiler() -> Instant {
    // RUNE_HEADLESS_WAIT=SECS holds the run before it starts, so a sampling profiler can attach.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(secs) = rune_core::env::var("RUNE_HEADLESS_WAIT").and_then(|s| s.parse::<f64>().ok()) {
        std::thread::sleep(std::time::Duration::from_secs_f64(secs));
    }
    crate::time::now()
}

fn single_window_list_only(args: &Args) -> bool {
    if args.list_windows { println!("main"); return true; }
    if args.window.as_deref().is_some_and(|key| key != "main") {
        die("no such window; this app has only 'main'");
    }
    false
}

fn try_headless_multi(main: crate::AnyWindowSpec, windows: Vec<crate::AnyWindowSpec>, args: Args, accessory: bool) -> Result<(), StartError> {
    let displays = desktop(&args, read_script(&args).as_ref()).displays();
    crate::window::displays::reset();
    crate::window::displays::remember(&displays);
    headless_window(std::iter::once(main).chain(windows).collect(), &displays, args, crate::RunOptions { accessory })
}

pub fn headless<S: 'static>(app: crate::app::App<S>, args: Args) {
    #[cfg(test)]
    if let Err(error) = try_headless(app, args) {
        panic!("{}", error.message());
    }
    #[cfg(not(test))]
    finish_start(try_headless(app, args));
}

/// `headless`, returning the error when GPU/renderer setup fails, when a window whose own
/// arguments aren't headless can't open, or when `--html` fails to read its template or write.
pub(crate) fn try_headless<S: 'static>(mut app: crate::app::App<S>, args: Args) -> Result<(), StartError> {
    crate::render::discard_prewarm();
    if app.windows.is_empty() && args.desktop.is_none() {
        if single_window_list_only(&args) { return Ok(()); }
        let scale = args.scale.unwrap_or(2.0);
        let options = app.window_options.clone();
        crate::window::displays::reset();
        crate::window::displays::remember(&desktop(&args, read_script(&args).as_ref()).displays());
        return headless_window_spec(app, options, args, scale);
    }
    let windows = std::mem::take(&mut app.windows);
    let accessory = app.accessory;
    let main = crate::WindowSpec::from_app("main", app).into();
    try_headless_multi(main, windows, args, accessory)
}

pub(crate) fn headless_window_spec<S: 'static>(mut app: crate::app::App<S>, options: crate::WindowOptions, args: Args, scale: f32) -> Result<(), StartError> {
    if let Some((w, h)) = options.physical_size {
        app.spec.size = (w as f32 / scale, h as f32 / scale);
    }
    if args.html.is_some() {
        return crate::tooling::html::export(app, &args).map_err(StartError::Window);
    }
    let transparent = options.transparent;
    app.window_options = options;
    try_headless_configured(app, args, transparent).map_err(StartError::Headless)
}

#[cfg(test)]
pub(crate) fn headless_configured<S: 'static>(app: crate::app::App<S>, args: Args, transparent: bool) {
    if let Err(error) = try_headless_configured(app, args, transparent) {
        panic!("{error}");
    }
}

fn try_headless_configured<S: 'static>(mut app: crate::app::App<S>, args: Args, transparent: bool) -> Result<(), String> {
    let launched = wait_for_profiler();
    if args.list_scenes {
        for (n, _) in app.spec.scenes { println!("{n}"); }
        return Ok(());
    }
    #[cfg(feature = "hot")]
    if args.snap_on_patch.is_some() {
        crate::hot::snap_window(app, args);
        return Ok(());
    }
    let prep = HeadlessPrep::begin(&args, app.spec.size);
    let (chrome, titlebar, fit_height) = (app.chrome, app.titlebar, app.fit_content_height);
    let app_options = app.window_options.clone();
    let comp = app.compositor.take();
    let (on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files) = (
        app.on_close_request, app.on_quit_request, app.on_close, app.on_quit, app.on_reopen, app.on_open_url, app.on_open_files,
    );
    let on_ready = app.on_ready.take();
    let on_frame_encoded = app.on_frame_encoded.take();
    let (playground, attach_playground) = (app.playground.take(), app.attach_playground);
    crate::credentials::note_headless_runtime();
    let (spec, state, shared, runtime) = app.into_parts(args.scene.as_deref());
    let shortcuts = crate::shortcuts::Guard::attach(&shared, Arc::new(|| {}), true, false);
    let menus = crate::native_menu::MenuGuard::attach(&shared, true);
    let state_ms = launched.elapsed().as_secs_f64() * 1000.0;
    let mut rt = runtime_for(&spec, state, prep.size, Env::new(prep.dark, prep.scale), Arc::new(|| {}));
    if let Some(attach) = attach_playground {
        attach(&mut rt, playground, &spec, args.playground.clone(), args.variant.clone(), false);
    }
    rt.configure(runtime);
    let config = HeadlessConfig {
        launched, state_ms, args, transparent, chrome, titlebar, fit_height, app_options,
        min_size: spec.min_size, on_ready, on_frame_encoded,
        shared_core: shared.core.clone(), shortcuts_core: shared.shortcuts.core.clone(), shortcuts, menus,
    };
    let mut typed = TypedHeadlessApp { rt, shared, comp, on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files };
    run_headless(prep, config, &mut typed)
}

fn run_headless(prep: HeadlessPrep, config: HeadlessConfig, app: &mut dyn HeadlessApp) -> Result<(), String> {
    let HeadlessPrep { source, script, timeline, scale, size, dark, mut writer } = prep;
    let HeadlessConfig {
        launched, state_ms, args, transparent, chrome, titlebar, fit_height, app_options,
        min_size, on_ready, on_frame_encoded, shared_core, shortcuts_core, shortcuts, mut menus,
    } = config;
    // A `--realtime` run stands in for a window, so it starts the app's live work as one does.
    let mut on_ready = on_ready.filter(|_| args.realtime);
    let mut reports = crate::frame::encoded::FrameReports::new(on_frame_encoded, Some("main"));
    app.rt_mut().headless_chrome(chrome, titlebar, app_options.borderless);
    app.rt_mut().transparent = transparent;
    app.rt_mut().host.set_fit_height(fit_height);
    if let Some(bars) = args.scroll_bars { app.rt_mut().set_system_scroll_bars(bars); }
    // Only limits the app set: the canvas opens at any size, below the default minimum too.
    let limits = (app_options.min_size.or(min_size).unwrap_or((0.0, 0.0)), app_options.max_size);
    if fit_height {
        let height = crate::window::frame::clamp_size((size.0, app.content_height()), limits.0, limits.1).1;
        app.rt_mut().set_viewport((size.0, height), scale);
    }
    if args.tree.is_some() { app.rt_mut().commit.enable_key_warnings(); }
    let has_comp = app.has_compositor();
    app.rt_mut().track_marks = has_comp;
    let runtime_ms = launched.elapsed().as_secs_f64() * 1000.0 - state_ms;

    let desktop = desktop(&args, source.as_ref());
    let opened = desktop.open_at(app_options.position, size);
    app.rt_mut().set_window_origin((opened[0], opened[1]));
    let Timeline { fps, dt, start_frame, frames, snap_frame, last_frame } = timeline;
    reports.set_refresh(Some(fps));

    let t0 = crate::time::now();
    let features = app.compositor_features();
    let srgb_blend = app.rt().srgb_blend();
    let target_format = if srgb_blend { OFFSCREEN_FORMAT } else { OFFSCREEN_FORMAT_LINEAR };
    let (gpu, mut renderer) = Gpu::for_headless_setup(features, |gpu| {
        Renderer::try_new_classified(&gpu.device, target_format).map_err(crate::render::SurfaceSetupError::from_shader)
    })
    .map_err(|e| e.message)?;
    renderer.attach_gpu(&gpu);
    renderer.set_srgb_blend(srgb_blend);
    renderer.attach_text(&mut app.rt_mut().text.atlas);
    let (wp, hp) = ((size.0 * scale).round() as u32, (size.1 * scale).round() as u32);
    let target = Offscreen::with_format(&gpu.device, wp, hp, target_format);
    let readback = crate::tooling::record::Readback::new(&gpu.device, &target, crate::tooling::record::DEPTH);
    let alpha = transparent.then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (wp, hp)));
    let overlay = if args.keys.is_some() || args.pointer {
        let mut ov = crate::input::keycaps::Overlay::new(size, scale, args.keys, args.keys_theme, args.pointer);
        ov.set_srgb_blend(srgb_blend);
        let mut ui = Renderer::try_new(&gpu.device, target_format)?;
        ui.set_srgb_blend(srgb_blend);
        Some((ov, ui))
    } else {
        None
    };
    let init_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let trace = args.trace.as_ref().map(|p| {
        let mut trace = Trace::open(p, size, fps, scale);
        trace.all = args.trace_all;
        trace
    });
    app.rt_mut().frame.enable_trace_channels(trace.is_some());
    let gate_out = std::env::var_os("RUNE_MOTION_GATE").map(std::path::PathBuf::from);
    let gate = gate_out.as_ref().map(|_| app.new_gate(fps as f64));
    app.rt_mut().scene.store_mut().audit_retargets(trace.is_some());
    let mut strip: Vec<Vec<u8>> = Vec::new();
    let crops_px: Vec<[u32; 4]> = args.crops.iter().map(|c| c.map(|v| (v * scale).round() as u32)).collect();
    let gap = (4.0 * scale) as u32;
    let wall = crate::time::now();
    let mut next_ev = 0;
    let mut last_down = (0.0f32, 0.0f32);
    // Events at t = 0 find keys and hit elements in a laid-out first frame, as input reaches a
    // real window only once it has drawn. That frame mounts the initial state with any queued
    // updates; frame 0 then draws the events' effects.
    let take_requests = |app: &mut dyn HeadlessApp, menus: &mut crate::native_menu::MenuGuard| {
        shortcuts.drain();
        let requests = shared_core.take_window();
        menus.drain();
        let _receiving = shortcuts.receiving();
        app.drain_updates(menus);
        if let Some(p) = requests.position { app.rt_mut().set_window_origin(desktop.to_points(p)); }
        requests.place
    };
    let mut frames_written = 0usize;
    let has_video = writer.has_video();
    let snap_crop = std::cell::Cell::new(None::<[u32; 4]>);
    let cpu_boot = std::cell::RefCell::new(args.cpu_boot.clone().map(|dir| crate::render::cpu_boot::tool::Tool::new(dir, args.cpu_boot_threads, gpu.adapter.get_info().vendor)));
    // Finished readbacks, in frame order.
    let mut deliver = |FrameShot { shot, chrome }: FrameShot, mut px: Vec<u8>| {
        if shot.snap && let Some(tool) = cpu_boot.borrow_mut().as_mut() { tool.gpu(&px, (wp, hp)); }
        if let Some(chrome) = chrome
            && let Some(lights) = chrome.traffic_light_rects_in(size.0, crate::labels::rtl())
        {
            paint_traffic_lights(&mut px, wp, hp, scale, &lights);
        }
        if shot.dump {
            if args.strip.is_some() {
                strip.push(tile(&px, wp, hp, &crops_px, gap));
            }
            if let Some(dir) = &args.out {
                writer.png(dir.join(format!("frame_{:04}.png", shot.k - start_frame)), px.clone()).unwrap_or_else(|e| die(&e));
            }
        }
        if shot.snap && let (Some(path), Some(r)) = (&args.snap, snap_crop.get()) {
            write_png(path, r[2], r[3], &crop::crop(&px, wp, r)).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
        } else if shot.snap {
            writer.png(args.snap.clone().unwrap(), px.clone()).unwrap_or_else(|e| die(&e));
        }
        if shot.video {
            frames_written += 1;
            writer.frame(px).unwrap_or_else(|e| die(&e));
        }
    };
    let mut state = HeadlessLoop {
        launched,
        state_ms,
        runtime_ms,
        init_ms,
        args: &args,
        dark,
        scale,
        size,
        wp,
        hp,
        limits,
        has_comp,
        has_video,
        start_frame,
        frames,
        snap_frame,
        dt,
        shared_core: &shared_core,
        desktop: &desktop,
        gpu: &gpu,
        target_format,
        renderer,
        target,
        readback,
        in_flight: crate::tooling::record::InFlight::new(crate::tooling::record::DEPTH),
        alpha,
        overlay,
        reports,
        trace,
        gate,
        frame_anim: None,
        // As on macOS, a frame change asked for while nothing moves waits a frame to start.
        idle: true,
        early_place: None,
        placed: None,
        snap_crop: &snap_crop,
        cpu_boot: &cpu_boot,
        loop_check: super::loop_check::Tally::default(),
        cpu_ms: Vec::new(),
        anim_ms: Vec::new(),
        rest_ms: Vec::new(),
        #[cfg(feature = "frame-profile")]
        profile: crate::frame::profile::Log::new(last_frame + 1),
    };
    let mut resize_from = app.rt().window_viewport();
    if script.iter().find(|e| !matches!(e.kind, Kind::Mark(_))).is_some_and(|e| e.t <= 1e-9) {
        state.early_place = take_requests(app, &mut menus);
        app.frame(0.0);
        app.open_popup_headless();
    }

    for k in 0..=last_frame {
        #[cfg(feature = "frame-profile")]
        let (alloc0, commits0, cpu0) = {
            crate::frame::profile::take();
            (procwatch::alloc::thread_allocations(), app.rt().stats.commits, crate::frame::profile::thread_cpu_mark())
        };
        let real_t = k as f64 * dt;
        #[cfg(not(target_arch = "wasm32"))]
        if args.realtime {
            let ahead = real_t - wall.elapsed().as_secs_f64();
            if ahead > 0.0 { std::thread::sleep(std::time::Duration::from_secs_f64(ahead)); }
        }
        let frame_start = crate::time::now();
        let script_before = next_ev;
        #[cfg(feature = "frame-profile")]
        let events0 = next_ev;
        while next_ev < script.len() && script[next_ev].t <= real_t + 1e-9 {
            let e = &script[next_ev];
            next_ev += 1;
            if let Some(g) = state.gate.as_mut() && !matches!(e.kind, Kind::Mark(_)) { g.input("script"); }
            let (x, y) = crate::tooling::script_point(app.rt(), e, last_down).unwrap_or_else(|msg| die(&format!("script: {msg}")));
            match &e.kind {
                Kind::Mark(name) if args.realtime => {
                    use std::io::Write as _;
                    println!("rune-mark {name} {}", e.t);
                    std::io::stdout().flush().ok();
                }
                Kind::Mark(_) => {}
                // The one window always shows here.
                Kind::Reopen => app.run_reopen(),
                Kind::CloseRequest => { let _ = app.run_close_request(); }
                Kind::QuitRequest => { let _ = app.run_quit_request(); }
                Kind::OpenUrl(url) => app.run_open_url(url),
                Kind::OpenFiles(paths) => app.run_open_files(paths),
                Kind::Shortcut(shortcut) => shortcuts_core.fire(shortcut).unwrap_or_else(|e| die(&format!("script shortcut: {e}"))),
                Kind::Menu(item) => menus.fire(item).unwrap_or_else(|e| die(&format!("script menu: {e}"))),
                Kind::PopupMenu(picked) => app.resolve_popup(picked.as_deref()).unwrap_or_else(|e| die(&format!("script pop-up menu: {e}"))),
                Kind::FileDialog(path) => app.rt_mut().resolve_file_dialog(path.clone()).unwrap_or_else(|e| die(&format!("script file dialog: {e}"))),
                Kind::FileDialogFiles(paths) => app.rt_mut().resolve_file_dialog_files(Some(paths.clone())).unwrap_or_else(|e| die(&format!("script file dialog: {e}"))),
                &Kind::Resize(dw, dh) => eased_input::step_resize(app.rt_mut(), &mut resize_from, (dw, dh), scale),
                kind => {
                    if kind.is_down() { last_down = (x, y); }
                    app.script_input(kind, (x, y));
                }
            }
            if let Some((ov, _)) = state.overlay.as_mut() {
                match &e.kind {
                    Kind::Key(key, _) | Kind::PhysicalKey(_, key, _) => ov.key(e.t, key),
                    Kind::Shortcut(chord) => ov.chord(e.t, chord),
                    &Kind::Physical(crate::PhysicalInput::Key { key, mods, pressed: true, .. }) => {
                        if let Ok(chord) = crate::Shortcut::from_physical_key(key, mods) { ov.chord(e.t, &chord) }
                    }
                    Kind::Down => ov.press(e.t, x, y),
                    _ => {}
                }
            }
        }
        // Frame 0 has drawn: a window runs `on_ready` after its first frame.
        if k == 1 && let Some(mut ready) = on_ready.take() { ready(&crate::WindowHandle::new("main")); }
        let requested_place = take_requests(app, &mut menus);
        let mut host = HeadlessFrame {
            state: &mut state,
            app,
            deliver: &mut deliver,
            k,
            real_t,
            frame_start,
            script_had_events: next_ev != script_before,
            requested_place,
            want_pixels: false,
            first_pixels: false,
            encoded_stamp: None,
            #[cfg(feature = "frame-profile")]
            fp_alloc0: alloc0,
            #[cfg(feature = "frame-profile")]
            fp_commits0: commits0,
            #[cfg(feature = "frame-profile")]
            fp_cpu0: cpu0,
            #[cfg(feature = "frame-profile")]
            fp_events: next_ev - events0,
        };
        crate::frame::host_step::run(&mut host);
        if app.rt_mut().take_user_close_request() {
            let _ = app.run_close_request();
        }
    }
    for (shot, px) in state.readback.finish(&gpu.device) { deliver(shot, px); }
    app.rt_mut().interaction_flush();
    drop(deliver);
    let written = frames_written;
    if let Some(video) = writer.finish().unwrap_or_else(|e| die(&e)) {
        eprintln!("rune: wrote {} ({written} frames at {fps} fps)", video.display());
    }
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok();

    if let Some(p) = &args.strip { write_strip(p, &strip, (wp, hp), &crops_px, gap, args.cols); }
    if let (Some(g), Some(p)) = (state.gate, &gate_out) {
        crate::motion_gate::evidence("", &g.rec, Some(p.display().to_string()));
        let text: String = crate::motion_gate::check(&g.rec).iter().map(|f| format!("{f}\n")).collect();
        std::fs::write(p, text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    }

    if frames > 0 {
        let wall = wall.elapsed().as_secs_f64();
        eprintln!("rune: {} frames in {wall:.2} s wall ({:.1} frames/s)", last_frame + 1, (last_frame + 1) as f64 / wall);
    }
    let loop_checked = state.loop_check.report();
    let q = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f64 * p).round() as usize] }
    };
    let mut sorted = state.cpu_ms.clone();
    #[cfg(feature = "frame-profile")]
    state.profile.report();
    let steady: Vec<f64> = state.cpu_ms.iter().skip(1).copied().collect();
    let mean = if steady.is_empty() { state.cpu_ms[0] } else { steady.iter().sum::<f64>() / steady.len() as f64 };
    eprintln!(
        "rune: {} frames at {fps} fps, gpu init {init_ms:.1} ms, first frame {:.2} ms, frame cpu mean {mean:.3} ms p50 {:.3} p95 {:.3} max {:.3}, {} instances",
        state.cpu_ms.len(),
        state.cpu_ms[0],
        q(&mut sorted, 0.5),
        q(&mut sorted, 0.95),
        q(&mut sorted, 1.0),
        app.rt().instances.len()
    );
    // The tail, on a line of its own so that readers of the line above keep working: p99 over every frame, and how many
    // frames after the first (which holds the first layout and the pipelines) cost more CPU than one frame's time.
    let budget_ms = 1000.0 / fps as f64;
    eprintln!(
        "rune: frame cpu p99 {:.3} ms, over budget {} of {} frames after the first ({budget_ms:.2} ms a frame at {fps} fps)",
        q(&mut sorted, 0.99),
        steady.iter().filter(|ms| **ms > budget_ms).count(),
        steady.len()
    );
    let (a50, a95) = (q(&mut state.anim_ms, 0.5), q(&mut state.anim_ms, 0.95));
    let (r50, r95) = (q(&mut state.rest_ms, 0.5), q(&mut state.rest_ms, 0.95));
    eprintln!(
        "rune: animating frames {} (cpu p50 {a50:.3} p95 {a95:.3} ms), at-rest frames {} (the window draws none; forced cpu p50 {r50:.3} p95 {r95:.3} ms), layout nodes {}, glyphs {}",
        state.anim_ms.len(),
        state.rest_ms.len(),
        app.rt().stats.layout_nodes,
        app.rt().stats.glyphs
    );
    if let Some(s) = app.compositor_summary(&gpu.device) {
        eprintln!("{s}");
    }
    // A headless run opens no links (`Tx::open_url`) or local paths (`Tx::open_path`, `Tx::reveal_path`): say which its handlers asked for.
    for url in app.rt_mut().host.take_url_requests() {
        eprintln!("rune: open_url {url} (headless: not opened)");
    }
    for req in app.rt_mut().host.take_path_requests() {
        eprintln!("rune: {} {} (headless: not opened)", req.action.verb(), req.path.display());
    }
    if !loop_checked { std::process::exit(3); }
    Ok(())
}

/// Which frames a headless run renders and dumps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Timeline {
    pub fps: f64,
    pub dt: f64,
    pub start_frame: usize,
    /// Frames dumped from `start_frame` (0 when nothing is dumped).
    pub frames: usize,
    pub snap_frame: usize,
    pub last_frame: usize,
}

impl Timeline {
    pub(crate) fn of(args: &Args, script: &[Ev]) -> Result<Timeline, String> {
        let mark = |name: &str| script.iter().find_map(|e| matches!(&e.kind, Kind::Mark(n) if n == name).then_some(e.t));
        let resolve = |spec: &String| {
            // Exact names win over offsets, including names such as "step-1" or "end+0.5".
            if let Some(t) = mark(spec) { return Ok(t); }
            let names: Vec<_> = script.iter().filter_map(|e| match &e.kind { Kind::Mark(n) => Some(n.as_str()), _ => None }).collect();
            let known = if names.is_empty() { "none".into() } else { names.join(", ") };
            // Only a complete numeric suffix is an offset. Starting at the first valid sign
            // keeps a scientific exponent's sign inside the number (end-1e-3).
            for (i, _) in spec.char_indices().skip(1).filter(|&(_, c)| c == '+' || c == '-') {
                if let Ok(offset) = spec[i..].parse::<f64>() && offset.is_finite() {
                    let name = &spec[..i];
                    return mark(name).map(|t| (t + offset).max(0.0))
                        .ok_or_else(|| format!("no mark {name:?} in the script (marks: {known})"));
                }
            }
            if spec.contains(['+', '-']) {
                Err(format!("invalid mark time {spec:?}: expected an exact mark name or NAME+SECONDS/NAME-SECONDS (marks: {known})"))
            } else {
                Err(format!("no mark {spec:?} in the script (marks: {known})"))
            }
        };
        let fps = args.fps.max(1.0);
        let dt = 1.0 / fps;
        let start = args.start_mark.as_ref().map_or(Ok(args.start), resolve)?;
        let at = args.at_mark.as_ref().map(resolve).transpose()?.or(args.at);
        let start_frame = (start * fps).round() as usize;
        let script_end = script.last().map_or(0.0, |e| e.t);
        // Output without a length runs to the `end` mark, or one second after the script.
        let until = |t: f64| ((t * fps).round() as usize).saturating_sub(start_frame) + 1;
        let auto = || until(mark("end").unwrap_or(script_end + 1.0));
        let frames = args.frames.unwrap_or_else(|| match (args.duration, args.video.is_some() || args.out.is_some()) {
            (Some(Length::Seconds(d)), _) => (d * fps).round().max(1.0) as usize,
            (Some(Length::Auto), _) | (None, true) => auto(),
            (None, false) => 0,
        });
        let snap_t = at.unwrap_or(if frames > 0 {
            (start_frame + frames - 1) as f64 * dt
        } else if script.is_empty() {
            0.0
        } else {
            script_end + 1.0
        });
        let snap_frame = (snap_t * fps).round() as usize;
        let last_frame = snap_frame.max(start_frame + frames.max(1) - 1);
        Ok(Timeline { fps, dt, start_frame, frames, snap_frame, last_frame })
    }
}

/// A frame whose pixels are being read back, and where they go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shot {
    pub k: usize,
    pub dump: bool,
    pub video: bool,
    pub snap: bool,
}

/// The 4 pt clearance box `[left, top, right, bottom]` around all three traffic lights,
/// whether Close sits on the left (LTR) or the right (RTL).
fn traffic_light_zone(lights: &[[f32; 4]; 3]) -> [f32; 4] {
    let m = 4.0;
    let left = lights.iter().map(|r| r[0]).fold(f32::INFINITY, f32::min);
    let top = lights.iter().map(|r| r[1]).fold(f32::INFINITY, f32::min);
    let right = lights.iter().map(|r| r[0] + r[2]).fold(f32::NEG_INFINITY, f32::max);
    let bottom = lights.iter().map(|r| r[1] + r[3]).fold(f32::NEG_INFINITY, f32::max);
    [left - m, top - m, right + m, bottom + m]
}

/// Antialiased discs where macOS draws close, minimise and zoom, plus a hairline box 4 pt
/// around them (the clearance an app should keep).
pub(super) fn paint_traffic_lights(px: &mut [u8], w: u32, h: u32, scale: f32, lights: &[[f32; 4]; 3]) {
    const COLORS: [[f32; 3]; 3] = [[255.0, 95.0, 87.0], [254.0, 188.0, 46.0], [40.0, 200.0, 64.0]];
    let mut blend = |x: i64, y: i64, c: [f32; 3], a: f32| {
        if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) || a <= 0.0 {
            return;
        }
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        for k in 0..3 {
            px[i + k] = (f32::from(px[i + k]) * (1.0 - a) + c[k] * a).round() as u8;
        }
    };
    for (r, c) in lights.iter().zip(COLORS) {
        let (cx, cy, rad) = ((r[0] + r[2] / 2.0) * scale, (r[1] + r[3] / 2.0) * scale, r[2] / 2.0 * scale);
        for y in (cy - rad - 1.0) as i64..=(cy + rad + 1.0) as i64 {
            for x in (cx - rad - 1.0) as i64..=(cx + rad + 1.0) as i64 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                blend(x, y, c, (rad - d + 0.5).clamp(0.0, 1.0));
            }
        }
    }
    let zone = traffic_light_zone(lights);
    let (x0, y0) = (zone[0] * scale, zone[1] * scale);
    let (x1, y1) = (zone[2] * scale, zone[3] * scale);
    let cyan = [0.0, 200.0, 255.0];
    for x in x0 as i64..=x1 as i64 {
        blend(x, y0 as i64, cyan, 0.8);
        blend(x, y1 as i64, cyan, 0.8);
    }
    for y in y0 as i64..=y1 as i64 {
        blend(x0 as i64, y, cyan, 0.8);
        blend(x1 as i64, y, cyan, 0.8);
    }
}

/// Visible text, icons and controls whose drawn rect comes within 4 pt of a traffic light.
pub(super) fn traffic_light_overlaps(rt: &RuntimeCore, lights: &[[f32; 4]; 3]) -> Vec<(String, [f32; 4])> {
    let zone = traffic_light_zone(lights);
    rt.elements().into_iter().filter_map(|e| {
        let drawn = e.text.is_some() || matches!(e.role, "icon" | "button" | "textbox");
        let v = e.visible;
        let (x1, y1) = (v[0] + v[2], v[1] + v[3]);
        (drawn && e.opacity > 0.01 && v[2] > 0.0 && v[3] > 0.0
            && v[0] < zone[2] && x1 > zone[0] && v[1] < zone[3] && y1 > zone[1])
            .then_some((e.path, e.visible))
    }).collect()
}

/// Lists visible text, icons and controls whose drawn rect comes within 4 pt of a traffic light.
fn report_traffic_light_overlaps(rt: &RuntimeCore, lights: &[[f32; 4]; 3]) {
    let overlaps = traffic_light_overlaps(rt, lights);
    for (path, visible) in &overlaps {
        eprintln!("rune: chrome: {path} overlaps the traffic lights at {:?}", visible.map(r2));
    }
    eprintln!("rune: chrome: traffic lights at {:?}, {} overlapping elements", lights[0].map(r2), overlaps.len());
}
