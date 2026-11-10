use super::composite::{MAX_BACKDROP, fit_backdrop, pixels};
use super::*;

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
        w.rt.frame(0.0);
        w.rt.set_full_keyboard_access(false);
        w.rt.key(KeyEvent { key: Key::Tab, mods: Default::default() });
        w.rt.frame(0.1);
        let focused = w.rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
        (focused, w.rt.presented("inset").unwrap().0[2])
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
    fn render(&mut self, _: &Gpu, _: &Slot, _: (u32, u32), _: f64, _: f64) {}
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
    w.rt.update(|_, tx| tx.close_window());
    w.drain(&desktop, 0.0);
    assert_eq!(w.rt.state, 1);
    assert!(!w.visible() && !w.reopen_candidate().reopenable);
}
