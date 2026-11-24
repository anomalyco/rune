use super::composite::{MAX_BACKDROP, fit_backdrop, pixels};
use super::*;
use crate::time::Instant;

#[test]
fn backdrop_is_cropped_to_the_output_aspect_then_scaled() {
    let dir = std::env::temp_dir().join(format!("rune-backdrop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Red, green and blue thirds side by side: a square output keeps the green middle.
    let wide = dir.join("wide.png");
    ::image::RgbaImage::from_fn(300, 100, |x, _| ::image::Rgba([if x < 100 { 255 } else { 0 }, if (100..200).contains(&x) { 255 } else { 0 }, if x >= 200 { 255 } else { 0 }, 255]))
        .save(&wide).unwrap();
    let px = fit_backdrop(&wide, (50, 50)).unwrap();
    assert_eq!(px.len(), 50 * 50 * 4);
    assert!(px.chunks(4).all(|p| p[1] > 200 && p[0] < 40 && p[2] < 40), "only the centre survives");
    assert_eq!(fit_backdrop(&wide, (300, 100)).unwrap()[..4], [255, 0, 0, 255], "same size: pixels as they are");

    let huge = dir.join("huge.png");
    ::image::GrayImage::new(MAX_BACKDROP + 1, 1).save(&huge).unwrap();
    let error = fit_backdrop(&huge, (10, 10)).unwrap_err();
    assert!(error.contains("16385x1"), "refused before decoding: {error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn translucent_backdrop_premultiplies_in_linear_light() {
    let dir = std::env::temp_dir().join(format!("rune-backdrop-alpha-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("alpha.png");
    let pixels_in = [
        [255u8, 255, 255, 128], // 50% white -> srgb_encode(1.0 * 128/255) = 188
        [188, 188, 188, 128],   // 50% of ~0.5 linear -> srgb_encode(0.5 * 0.5) = 137
        [12, 34, 56, 255],      // opaque: unchanged
        [255, 0, 0, 0],         // transparent: black
    ];
    ::image::RgbaImage::from_fn(4, 1, |x, _| ::image::Rgba(pixels_in[x as usize]))
        .save(&path).unwrap();
    let px = fit_backdrop(&path, (4, 1)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(&px[0..4], &[188, 188, 188, 255], "half white over black in linear light");
    assert!((px[4] as i32 - 137).abs() <= 1 && px[4] == px[5] && px[5] == px[6] && px[7] == 255, "half mid-grey: {:?}", &px[4..8]);
    assert_eq!(&px[8..12], &[12, 34, 56, 255], "opaque pixel stays exact");
    assert_eq!(&px[12..16], &[0, 0, 0, 255], "transparent pixel becomes black");
}

/// Each window of a headless multi-window run takes its own Tab opt-in and titlebar inset,
/// and a borderless window has no traffic lights to inset for.
#[test]
fn each_window_takes_its_tab_opt_in_and_titlebar_inset() {
    use crate::{AppSpec, Cx, El, Key, KeyEvent, col, el, row};
    use crate::WindowSpec;
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        col().size(300.0, 100.0)
            .child(row().child(el().key("inset").size(cx.titlebar_inset().max(1.0), 10.0)))
            .child(el().key("button").size(30.0, 30.0).focusable(true).role("button"))
    }
    let window = |tab: bool, borderless: bool| {
        let spec = WindowSpec::new("w", AppSpec::new("W", (300.0, 100.0), || (), view)).tab_visits_controls(tab);
        let spec = WindowSpec { options: crate::WindowOptions { borderless, ..Default::default() }, ..spec };
        let mut w = Window::new(spec, None, &Args::default(), true, true, 1.0);
        let rt = w.rt_mut::<()>();
        rt.frame(0.0);
        rt.set_full_keyboard_access(false);
        rt.key(KeyEvent { key: Key::Tab, mods: Default::default() });
        rt.frame(0.1);
        let focused = rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
        (focused, rt.presented("inset").unwrap().0[2])
    };
    assert_eq!(window(true, false), (Some("button".into()), 67.0));
    assert_eq!(window(false, false).0, None, "keyboard navigation off skips buttons");
    assert_eq!(window(true, true).1, 1.0, "borderless: no inset");
}

type Log = std::rc::Rc<std::cell::RefCell<Vec<(usize, String)>>>;

/// A window that only has a frame and visibility, and logs the pointer input it gets.
struct Fake { i: usize, frame: [f32; 4], visible: bool, log: Log }

impl Member for Fake {
    fn key(&self) -> &str { "fake" }
    fn frame(&self) -> [f32; 4] { self.frame }
    fn visible(&self) -> bool { self.visible }
    fn click_through(&self) -> bool { false }
    fn on_top(&self) -> bool { false }
    fn radius(&self) -> f32 { 0.0 }
    fn shadow(&self) -> bool { false }
    fn features(&self) -> wgpu::Features { wgpu::Features::empty() }
    fn start(&mut self, _: &VirtualDesktop) {}
    fn trace_on(&mut self) {}
    fn drain(&mut self, _: &VirtualDesktop, _: f64) -> Changes { Changes::default() }
    fn pending(&self) -> bool { false }
    fn stale(&self) -> bool { false }
    fn advance(&mut self, _: f64) {}
    fn focus(&mut self, _: bool) {}
    fn resize_by(&mut self, _: f32, _: f32) {}
    fn input(&mut self, kind: &Kind, _: (f32, f32)) { self.log.borrow_mut().push((self.i, format!("{kind:?}"))); }
    fn leave(&mut self) { self.log.borrow_mut().push((self.i, "Leave".into())); }
    fn find(&self, _: &str) -> Option<(f32, f32)> { None }
    fn pointer(&self) -> Option<(f32, f32)> { None }
    fn shortcut(&self, _: &crate::Shortcut) -> Result<(), crate::ShortcutError> { Ok(()) }
    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate { unimplemented!() }
    fn reopen_hook(&mut self, _: bool) -> bool { false }
    fn reopen(&mut self) -> Changes { Changes::default() }
    fn menu(&self, _: &str) -> Result<(), crate::MenuError> { Ok(()) }
    fn popup_open(&self) -> bool { false }
    fn resolve_popup(&mut self, _: Option<&str>) -> Result<(), crate::PopupMenuError> { Ok(()) }
    fn file_dialog_open(&self) -> bool { false }
    fn resolve_file_dialog(&mut self, _: Option<std::path::PathBuf>) -> Result<(), crate::FileDialogError> { Ok(()) }
    fn resolve_file_dialog_files(&mut self, _: Option<Vec<std::path::PathBuf>>) -> Result<(), crate::FileDialogError> { Ok(()) }
    fn render(&mut self, _: &Gpu, _: &Slot, _: (u32, u32), _: f64, _: f64, _: Instant) {}
    fn flush_interaction(&mut self) {}
    fn backdrop_dark(&self) -> Option<bool> { None }
    fn tree(&self, _: f64) -> serde_json::Value { serde_json::Value::Null }
    fn trace(&mut self, _: &mut Trace, _: f64, _: f64) {}
    fn summary(&mut self, _: &wgpu::Device) -> Option<String> { None }
}

/// Two 100-point windows side by side, the right one in front.
fn two_windows() -> (Desk, Log) {
    let log = Log::default();
    let members: Vec<Box<dyn Member>> = (0..2)
        .map(|i| Box::new(Fake { i, frame: [i as f32 * 100.0, 0.0, 100.0, 100.0], visible: true, log: log.clone() }) as Box<dyn Member>)
        .collect();
    let desk = Desk { members, z: vec![0, 1], focus: Focus::of(Some(1), true), pointer: Pointer::Out, files: Files::None, origin: (0.0, 0.0), scale: 1.0 };
    (desk, log)
}

fn hide(desk: &mut Desk, i: usize) {
    let fake = Fake { i, frame: desk.members[i].frame(), visible: false, log: Log::default() };
    desk.members[i] = Box::new(fake);
}

/// A press holds the pointer until it lifts; a window that hides meanwhile lets go of it even
/// when it no longer had the keyboard, so the pointer goes to the window under it again.
#[test]
fn a_hidden_window_lets_go_of_a_captured_pointer() {
    let (mut desk, log) = two_windows();
    desk.pointer_input(&Kind::Down, (150.0, 50.0));
    assert_eq!(desk.pointer, Pointer::Pressed(1));
    desk.pointer_input(&Kind::Move, (50.0, 50.0));
    assert_eq!(log.borrow().last(), Some(&(1, "Move".into())), "a drag stays with the window it pressed");
    // Another window takes the keyboard, then the pressed one hides.
    desk.focus(Some(0), true);
    hide(&mut desk, 1);
    desk.settle_key();
    assert_eq!(desk.pointer, Pointer::Out);
    desk.pointer_input(&Kind::Move, (50.0, 50.0));
    assert_eq!(log.borrow().last(), Some(&(0, "Move".into())), "the window under the pointer gets it");
    assert_eq!(desk.pointer, Pointer::Over(0));
}

#[test]
fn a_release_keeps_the_pointer_over_the_window_it_pressed() {
    let (mut desk, log) = two_windows();
    desk.pointer_input(&Kind::Move, (50.0, 50.0));
    desk.pointer_input(&Kind::Down, (50.0, 50.0));
    assert_eq!((desk.pointer, desk.focus.key()), (Pointer::Pressed(0), Some(0)), "a press takes the keyboard");
    desk.pointer_input(&Kind::Up, (150.0, 50.0));
    assert_eq!(desk.pointer, Pointer::Over(0), "until the pointer next moves");
    desk.pointer_input(&Kind::Move, (150.0, 50.0));
    assert_eq!(desk.pointer, Pointer::Over(1));
    let log = log.borrow();
    assert_eq!(log[log.len() - 2..], [(0, "Leave".into()), (1, "Move".into())]);
}

#[test]
fn a_click_on_the_backdrop_deactivates_the_app_but_keeps_its_key_window() {
    let (mut desk, _) = two_windows();
    desk.pointer_input(&Kind::Down, (150.0, 50.0));
    assert_eq!(desk.focus.focused(), Some(1));
    desk.pointer_input(&Kind::Down, (500.0, 500.0));
    assert_eq!(desk.pointer, Pointer::Out, "the backdrop takes the press");
    assert_eq!((desk.focus.key(), desk.focus.focused()), (Some(1), None), "no focus while the app is behind");
    desk.set_active(true);
    assert_eq!(desk.focus.focused(), Some(1), "the window it comes back with has focus again");
}

#[test]
fn output_pixels_fit_a_texture() {
    assert_eq!(pixels((1512.0, 982.0), 2.0, 8192), Some((3024, 1964)));
    assert_eq!(pixels((0.2, 10.0), 2.0, 8192), None, "under a pixel");
    assert_eq!(pixels((5000.0, 10.0), 2.0, 8192), None, "over the GPU's limit");
    assert_eq!(pixels((f32::NAN, 10.0), 2.0, 8192), None);
    assert_eq!(pixels((10.0, 10.0), f32::INFINITY, 8192), None);
}

/// A window closed for good runs no more close hooks, even when its own `on_close` asks to
/// close again.
#[test]
fn a_finished_window_runs_its_close_hook_once() {
    use crate::{App, AppSpec, El, Tx, WindowSpec, el};
    fn close(closes: &mut u32, tx: &mut Tx) { *closes += 1; tx.close_window(); }
    let app = App::new(AppSpec::new("W", (100.0, 100.0), || 0u32, |_: &u32, _: &mut crate::Cx<u32>| -> El { el() })).on_close(close);
    let mut w = Window::new(WindowSpec::from_app("w", app), None, &Args::default(), true, true, 1.0);
    let desktop = VirtualDesktop::from_script(None, 2.0).unwrap();
    w.rt_mut::<u32>().update(|_, tx| tx.close_window());
    w.drain(&desktop, 0.0);
    assert_eq!(w.rt_mut::<u32>().state, 1);
    assert!(!w.visible() && !w.reopen_candidate().reopenable);
}

#[test]
fn virtual_desktop_window_scales_input_by_zoom_and_releases_keys_on_hide() {
    use crate::{App, AppSpec, El, PhysicalInput, WindowSpec, el};
    #[derive(Default)]
    struct S { wheel_dy: Vec<f32>, focus_lost: u32 }
    fn view(_: &S, cx: &mut crate::Cx<S>) -> El {
        cx.zoom(2.0);
        el().key("root").size(200.0, 100.0).focusable(true)
            .child(
                el().key("btn")
                    .absolute(20.0, 10.0)
                    .size(40.0, 20.0)
                    .hit()
                    .on_wheel(|s: &mut S, ev, _| { s.wheel_dy.push(ev.dy); true }),
            )
            .on_physical_input(|s: &mut S, ev, _| {
                if matches!(ev, PhysicalInput::FocusLost) { s.focus_lost += 1; }
                true
            })
    }
    let app = App::new(AppSpec::new("Z", (200.0, 100.0), S::default, view)).keep_running();
    let mut w = Window::new(WindowSpec::from_app("z", app), None, &Args::default(), true, true, 1.0);
    let desktop = VirtualDesktop::from_script(None, 1.0).unwrap();
    w.start(&desktop);
    w.advance(0.0);
    w.rt_mut::<S>().update(|_, tx| tx.focus("root", false));
    w.advance(0.0);

    // Element at view (20..60, 10..30) -> centre at view (40, 20) -> window (80, 40) at 2x zoom.
    assert_eq!(w.find("btn"), Some((80.0, 40.0)), "find reports window points, scaled by zoom");

    // Pointer move and wheel arrive in window points; runtime must receive view points and zoomed delta.
    w.input(&Kind::Move, (80.0, 40.0));
    assert_eq!(w.rt_mut::<S>().pointer(), Some((40.0, 20.0)), "pointer in view points");
    assert_eq!(w.pointer(), Some((80.0, 40.0)), "Window::pointer reports window points");
    w.input(&Kind::Wheel(0.0, -40.0), (80.0, 40.0));
    w.advance(0.0);
    assert_eq!(w.rt_mut::<S>().state.wheel_dy, [-20.0], "wheel delta divides by zoom like window and headless hosts");

    // Hiding releases keys via FocusLost, matching window/host::wind_down.
    w.focus(true);
    w.advance(0.0);
    w.rt_mut::<S>().update(|_, tx| tx.close_window());
    w.drain(&desktop, 0.0);
    w.advance(0.0);
    assert_eq!(w.rt_mut::<S>().state.focus_lost, 1, "hiding a virtual desktop window must emit keys_lost");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn uniform_layout_matches_wgsl() {
    use std::mem::{offset_of, size_of};
    use wgpu::naga::{TypeInner, front::wgsl, proc::Layouter, valid::{Capabilities, ValidationFlags, Validator}};
    use super::composite::{SHADER, Uniform};

    let module = wgsl::parse_str(SHADER).unwrap_or_else(|e| panic!("virtual_desktop: {}", e.emit_to_string(SHADER)));
    Validator::new(ValidationFlags::all(), Capabilities::default())
        .validate(&module)
        .unwrap_or_else(|e| panic!("virtual_desktop: {e}"));
    let mut layouter = Layouter::default();
    layouter.update(module.to_ctx()).unwrap_or_else(|e| panic!("virtual_desktop: {e}"));

    let (handle, ty) = module
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some("W"))
        .expect("WGSL struct W not found");
    assert_eq!(layouter[handle].size as usize, size_of::<Uniform>(), "W size");
    let TypeInner::Struct { members, span } = &ty.inner else { panic!("W is not a struct") };
    assert_eq!(*span as usize, size_of::<Uniform>(), "W span");
    let expected: &[(&str, usize)] = &[
        ("origin", offset_of!(Uniform, origin)),
        ("size", offset_of!(Uniform, size)),
        ("out_size", offset_of!(Uniform, out_size)),
        ("shadow_offset", offset_of!(Uniform, shadow_offset)),
        ("mask_size", offset_of!(Uniform, mask_size)),
        ("radius", offset_of!(Uniform, radius)),
        ("pad", offset_of!(Uniform, pad)),
        ("sigma", offset_of!(Uniform, sigma)),
        ("opacity", offset_of!(Uniform, opacity)),
        ("unused", offset_of!(Uniform, _unused)),
    ];
    assert_eq!(members.len(), expected.len(), "W member count");
    for (member, &(field, offset)) in members.iter().zip(expected) {
        assert_eq!(member.name.as_deref(), Some(field), "W.{field} name");
        assert_eq!(member.offset as usize, offset, "W.{field} offset");
    }
}

#[test]
fn virtual_desktop_window_follows_fit_content_height_and_resize_window() {
    use crate::{App, AppSpec, El, WindowSpec, col, el};
    fn view(rows: &usize, _: &mut crate::Cx<usize>) -> El {
        col().key("root").w_full()
            .child(el().key("head").size(50.0, 50.0))
            .children((0..*rows).map(|i| el().key(format!("row{i}")).size(100.0, 40.0)))
    }
    let app = App::new(AppSpec::new("fit", (300.0, 400.0), || 1usize, view)).fit_content_height();
    let mut w = Window::new(WindowSpec::from_app("fit", app), None, &Args::default(), true, true, 1.0);
    let desktop = VirtualDesktop::from_script(None, 1.0).unwrap();
    w.start(&desktop);
    assert_eq!(w.frame()[2..4], [300.0, 90.0], "opens at content height");

    w.advance(0.0);
    w.rt_mut::<usize>().update(|rows, _| *rows = 2);
    // Run 2 seconds of virtual time so the fit-height spring settles.
    for _ in 0..120 {
        w.advance(1.0 / 60.0);
    }
    assert_eq!(w.frame()[2..4], [300.0, 130.0], "grows to match new content height");

    // Explicit Tx::resize_window_instant updates the window frame too.
    w.rt_mut::<usize>().update(|_, tx| tx.resize_window_instant((360.0, 130.0)));
    w.advance(1.0 / 60.0);
    w.advance(1.0 / 60.0);
    assert_eq!(w.frame()[2..4], [360.0, 130.0], "Tx::resize_window_instant resizes virtual desktop window");
}

#[test]
fn virtual_desktop_reports_frames_to_on_frame_encoded() {
    if !crate::render::Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();
    let dir = std::env::temp_dir().join(format!("rune-desktop-encoded-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let backdrop = dir.join("backdrop.png");
    ::image::RgbaImage::from_pixel(64, 64, ::image::Rgba([64, 64, 64, 255])).save(&backdrop).unwrap();
    let snap = dir.join("out.png");
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = seen.clone();
    let app = crate::App::new(crate::AppSpec::new("enc", (20.0, 20.0), || (), |_, _| crate::el()))
        .on_frame_encoded(move |f| {
            log.borrow_mut().push((f.seq(), f.outcome(), f.window().map(str::to_owned), f.refresh_hz()));
        });
    let args = [
        "--desktop", backdrop.to_str().unwrap(),
        "--at", "0.1", "--fps", "30", "--scale", "1",
        "--snap", snap.to_str().unwrap(),
    ];
    crate::tooling::headless(app, Args::parse_from(args.map(String::from).into()));
    std::fs::remove_dir_all(&dir).unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 4, "frames 0..=3 at 30 fps must report to on_frame_encoded: {seen:?}");
    for (i, (seq, outcome, window, hz)) in seen.iter().enumerate() {
        assert_eq!((*seq, *outcome, window.as_deref(), *hz), (i as u64, crate::FrameOutcome::Drawn, Some("main"), Some(30.0)));
    }
}

