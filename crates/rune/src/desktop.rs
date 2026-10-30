//! `--desktop BACKDROP.png`: a headless run of every window an app makes, each drawn at its
//! placed frame with rounded corners and a soft shadow over a backdrop, as a screen recording of
//! the app would show them. Each window renders into its own texture with its own state; one
//! pass composites them in z-order.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::time::Instant;

use serde_json::{Value, json};

use crate::app::{Shared, WindowRequest};
use crate::el::Shell;
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::runtime::{Env, Runtime};
use crate::tooling::{Args, Ev, Kind, Pos, Shot, Timeline, Trace, die};
use crate::virtual_desktop::VirtualDesktop;
use crate::window::WakeEvent;
use crate::headless_run::WindowFrame;
use crate::{AnyWindowSpec, WindowHandle, WindowSpec};

/// Corner radius of a titled window, in points.
const RADIUS: f32 = 10.0;
/// Shadows of the key window and of the others: blur sigma, downward offset (points), opacity.
const KEY_SHADOW: (f32, f32, f32) = (14.0, 14.0, 0.5);
const SHADOW: (f32, f32, f32) = (6.0, 5.0, 0.32);
/// Shadow masks are this many window pixels to a texel.
const DOWN: u32 = 4;
/// Bound on rounds of windows asking things of each other within one frame.
const PASSES: usize = 8;
/// Largest backdrop image, in pixels per side, decoded.
const MAX_BACKDROP: u32 = 16384;

/// What a window asked of the desktop while it drained its requests.
#[derive(Default)]
pub(crate) struct Changes {
    /// Shown or ordered in front: it goes to the top.
    front: bool,
    /// It takes the keyboard.
    keys: bool,
}

/// One window of a `--desktop` run, whatever its state type.
pub(crate) trait Member {
    fn key(&self) -> &str;
    /// x, y, width, height in global points.
    fn frame(&self) -> [f32; 4];
    fn visible(&self) -> bool;
    fn click_through(&self) -> bool;
    fn on_top(&self) -> bool;
    /// Corner radius in points.
    fn radius(&self) -> f32;
    fn shadow(&self) -> bool;
    fn features(&self) -> wgpu::Features;
    /// Its first frame: where it opens on `desktop` when it has no position.
    fn start(&mut self, desktop: &VirtualDesktop);
    fn trace_on(&mut self);
    /// Applies what the app asked since the last call (updates, show/hide, place, close).
    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) -> Changes;
    /// Something asked of it since `drain`, such as by a focus change another window's drain made.
    fn pending(&self) -> bool;
    /// Its last frame no longer shows its state, size or input.
    fn stale(&self) -> bool;
    fn advance(&mut self, dt: f64);
    fn focus(&mut self, focused: bool);
    /// Pointer and keyboard input; pointer kinds at `at`, in window points.
    fn input(&mut self, kind: &Kind, at: (f32, f32));
    fn leave(&mut self);
    fn find(&self, key: &str) -> Option<(f32, f32)>;
    fn pointer(&self) -> Option<(f32, f32)>;
    fn shortcut(&self, shortcut: &crate::Shortcut) -> Result<(), crate::ShortcutError>;
    /// How reopening the app sees it.
    fn reopen_candidate(&self) -> crate::reopen::Candidate;
    /// Runs `App::on_reopen`; true when it handled the reopen.
    fn reopen_hook(&mut self, has_visible_windows: bool) -> bool;
    /// Shows it in front with the keyboard, as the default reopen.
    fn reopen(&mut self) -> Changes;
    fn menu(&self, item: &str) -> Result<(), crate::MenuError>;
    fn popup_open(&self) -> bool;
    fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError>;
    fn file_dialog_open(&self) -> bool;
    fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), crate::FileDialogError>;
    /// Draws its frame into `slot`, `px` device pixels.
    fn render(&mut self, gpu: &Gpu, slot: &Slot, px: (u32, u32), dt: f64, update_ms: f64);
    fn backdrop_dark(&self) -> Option<bool>;
    fn tree(&self, t: f64) -> Value;
    fn trace(&mut self, trace: &mut Trace, t_ms: f64, cpu_ms: f64);
    fn summary(&mut self, device: &wgpu::Device) -> Option<String>;
}

pub(crate) struct Window<S: 'static> {
    key: String,
    rt: Runtime<S>,
    shared: Arc<Shared<S>>,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::Guard<S>,
    handle: Option<WindowHandle>,
    comp: Option<Box<dyn crate::compose::Compositor<S>>>,
    renderer: Option<Renderer>,
    on_close: Option<fn(&mut S, &mut crate::Tx)>,
    on_reopen: Option<fn(&mut S, bool, &mut crate::Tx) -> bool>,
    keep_running: bool,
    launcher: bool,
    /// A nonactivating panel, launcher or not: reopening never brings it back.
    panel: bool,
    /// The app's own window, not one `App::window` added.
    main: bool,
    /// When it last hid, in `reopen::stamp` order.
    hidden_at: Option<u64>,
    shadow: bool,
    click_through: bool,
    on_top: bool,
    radius: f32,
    position: Option<(i32, i32)>,
    /// `WindowOptions::physical_size`, in pixels of the display the window opens on.
    physical: Option<(u32, u32)>,
    frame: [f32; 4],
    visible: bool,
    /// Closed without `keep_running`: it never shows again.
    finished: bool,
    placed: Option<Value>,
}

impl<S: 'static> Window<S> {
    pub(crate) fn new(spec: WindowSpec<S>, handle: Option<WindowHandle>, args: &Args, selected: bool, main: bool, scale: f32) -> Self {
        let WindowSpec { key, mut app, options, .. } = spec;
        // --size is the whole output here; each window keeps its own size.
        let mut inherited = args.clone();
        inherited.size = None;
        if !selected { inherited.scene = None; inherited.playground = None; inherited.variant = None; }
        let own = app.resolve_args(inherited);
        let comp = app.compositor.take();
        let (open, keep_running, on_close, on_focus, on_reopen) = (app.open, app.keep_running, app.on_close, app.on_focus, app.on_reopen);
        let on_ime = app.on_ime;
        let native = std::mem::take(&mut app.macos_window);
        let (modifier_keys, tab_visits_controls, srgb_blend) = (app.modifier_keys, app.tab_visits_controls, app.srgb_blend);
        let font_smoothing = app.font_smoothing;
        let titlebar_inset = if options.borderless { 0.0 } else { app.chrome.titlebar_inset() };
        let playground = app.playground.take();
        let (spec, state, shared) = app.into_parts(own.scene.as_deref());
        let shortcuts = crate::shortcuts::Guard::attach(&shared, Arc::new(|| {}), true, false);
        let menus = crate::native_menu::Guard::attach(&shared, true);
        let size = own.size.unwrap_or(spec.size);
        let dark = own.theme.as_deref() != Some("light");
        let mut rt = crate::tooling::runtime_for(&spec, state, size, Env::new(dark, scale), Arc::new(|| {}));
        crate::playground::attach(&mut rt, playground, &spec, own.playground.clone(), own.variant.clone(), false);
        rt.on_focus = on_focus;
        rt.app_ime.hook = on_ime;
        rt.set_modifier_keys(modifier_keys);
        rt.set_tab_visits_controls(tab_visits_controls);
        rt.set_srgb_blend(srgb_blend);
        rt.set_font_smoothing(font_smoothing);
        rt.set_titlebar_inset(titlebar_inset);
        rt.transparent = options.transparent;
        rt.track_marks = comp.is_some();
        if selected && args.tree.is_some() { rt.key_warnings = Some(Vec::new()); }
        let visible = options.visible && open;
        rt.set_live_visible(visible);
        shared.visible.store(visible, Ordering::Release);
        let decorated = !options.borderless && !options.transparent;
        Window {
            key, rt, shared, shortcuts, menus, handle, comp, renderer: None, on_close, on_reopen, keep_running,
            launcher: native.launcher,
            panel: native.nonactivating_panel,
            main,
            hidden_at: None,
            shadow: native.shadow != Some(false),
            click_through: options.click_through,
            on_top: options.always_on_top,
            radius: if decorated { RADIUS } else { 0.0 },
            position: options.position,
            physical: options.physical_size,
            frame: [0.0, 0.0, size.0, size.1],
            visible,
            finished: false,
            placed: None,
        }
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.rt.set_live_visible(visible);
        self.shared.visible.store(visible, Ordering::Release);
    }

    fn request(&mut self, request: WindowRequest, changes: &mut Changes) {
        match request {
            WindowRequest::Open(_) | WindowRequest::Show if self.finished => {}
            WindowRequest::Open(_) => {
                self.show();
                changes.front = true;
                changes.keys = true;
            }
            WindowRequest::Show => {
                self.show();
                changes.front = true;
                // A launcher takes keys when it shows.
                changes.keys |= self.launcher;
            }
            WindowRequest::Hide => self.hide(),
            WindowRequest::Close => self.close(),
        }
    }

    fn show(&mut self) {
        if self.visible { return; }
        // A launcher reveals the frame it kept up to date while hidden; others land their motion.
        if !self.launcher { self.rt.settle_all(); }
        self.set_visible(true);
    }

    fn hide(&mut self) {
        if !self.visible { return; }
        self.hidden_at = Some(crate::reopen::stamp());
        self.rt.cancel_popups();
        self.rt.cancel_file_dialogs();
        self.rt.cancel_pointer();
        self.set_visible(false);
    }

    fn close(&mut self) {
        if !(self.visible || !self.keep_running) { return; }
        if let Some(f) = self.on_close { self.rt.update(f); }
        self.hide();
        if !self.keep_running {
            self.rt.cancel_popups();
            self.rt.cancel_file_dialogs();
            self.finished = true;
        }
    }

    fn place(&mut self, desktop: &VirtualDesktop, request: &crate::windows::PlaceRequest, t: f64) {
        let size = (self.frame[2], self.frame[3]);
        let (display, frame) = desktop.place(request, size);
        let record = json!({ "t": t, "window": self.key, "display": display, "frame": frame });
        println!("rune-placed {record}");
        self.placed = Some(record);
        self.position = None;
        self.resize(frame);
    }

    fn move_to(&mut self, (x, y): (f32, f32)) {
        self.frame[0] = x;
        self.frame[1] = y;
        self.rt.set_window_origin((x, y));
    }

    fn resize(&mut self, frame: [f32; 4]) {
        self.frame = frame;
        let scale = self.rt.env.scale;
        self.rt.set_viewport((frame[2], frame[3]), scale);
        self.rt.set_window_origin((frame[0], frame[1]));
    }
}

impl<S: 'static> Member for Window<S> {
    fn key(&self) -> &str { &self.key }
    fn frame(&self) -> [f32; 4] { self.frame }
    fn visible(&self) -> bool { self.visible }
    fn click_through(&self) -> bool { self.click_through }
    fn on_top(&self) -> bool { self.on_top }
    fn radius(&self) -> f32 { self.radius }
    fn shadow(&self) -> bool { self.shadow }
    fn features(&self) -> wgpu::Features { self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features()) }

    fn start(&mut self, desktop: &VirtualDesktop) {
        let display = desktop.display_at(self.position);
        let (w, h) = match self.physical {
            Some((w, h)) => ((f64::from(w) / display.scale) as f32, (f64::from(h) / display.scale) as f32),
            None => (self.frame[2], self.frame[3]),
        };
        self.resize(desktop.open_at(self.position, (w, h)));
    }

    fn trace_on(&mut self) {
        self.rt.trace_channels = Some(Default::default());
        self.rt.scene.store_mut().audit_retargets(true);
    }

    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) -> Changes {
        let mut changes = Changes::default();
        self.shortcuts.drain();
        self.menus.drain();
        let requests = self.shared.drain(|f| {
            self.rt.update_if(f);
            self.menus.drain();
        });
        if let Some(p) = requests.position { self.move_to(desktop.to_points(p)); }
        if let Some(place) = &requests.place { self.place(desktop, place, t); }
        if let Some(request) = requests.window { self.request(request, &mut changes); }
        if requests.focus && self.visible { changes.front = true; changes.keys = true; }
        // `quit` ends a native app; a recording runs to the end of its timeline.
        let queued = self.handle.as_ref().map(WindowHandle::take_headless).unwrap_or_default();
        for event in queued {
            match event {
                WakeEvent::Window(_, request) => self.request(request, &mut changes),
                WakeEvent::Place(_, place) => self.place(desktop, &place, t),
                WakeEvent::Position(_, p) => self.move_to(desktop.to_points(p)),
                WakeEvent::Focus(_) if self.visible => { changes.front = true; changes.keys = true; }
                _ => {}
            }
        }
        for _ in 0..8 {
            // A headless desktop draws no Dock to minimize into (`requests.minimize` too).
            self.rt.minimize_request = false;
            match self.rt.shell_request.take() {
                Some(Shell::Close) => self.close(),
                Some(Shell::Quit) | None => {}
            }
            if self.visible {
                self.rt.open_popup_headless();
            } else {
                self.rt.cancel_popups();
                self.rt.refuse_file_dialog("cannot open a file dialog while the window is hidden");
            }
            if self.rt.shell_request.is_none() { break; }
        }
        changes
    }

    fn pending(&self) -> bool {
        self.shared.window_pending() || self.rt.shell_request.is_some() || self.handle.as_ref().is_some_and(WindowHandle::headless_pending)
    }

    fn stale(&self) -> bool { self.rt.stale() || self.rt.text.families_behind() }
    fn advance(&mut self, dt: f64) { self.rt.frame(dt); }
    fn focus(&mut self, focused: bool) { self.rt.window_focus(focused); }

    fn input(&mut self, kind: &Kind, (x, y): (f32, f32)) {
        let rt = &mut self.rt;
        match kind {
            Kind::Focus(focused) => rt.window_focus(*focused),
            Kind::Move => rt.pointer_move(x, y),
            Kind::Down => rt.pointer_down(x, y),
            Kind::Up => rt.pointer_up(x, y),
            Kind::SecondaryDown => rt.secondary_down(x, y),
            Kind::SecondaryUp => rt.secondary_up(x, y),
            Kind::Key(k, false) => rt.key(k.clone()),
            Kind::Key(k, true) => rt.repeat_key(k.clone()),
            Kind::Physical(event) => rt.physical_input(*event),
            Kind::Paste(content) => rt.paste_content(content.clone()),
            Kind::Wheel(dx, dy) => crate::tooling::wheel(rt, (x, y), *dx, *dy),
            Kind::Fling(vx, vy) => crate::tooling::fling(rt, (x, y), *vx, *vy),
            Kind::Files(step) => crate::tooling::file_input(rt, step, (x, y)),
            Kind::Mark(_) | Kind::Reopen | Kind::Menu(_) | Kind::PopupMenu(_) | Kind::FileDialog(_) | Kind::Shortcut(_) => {}
            kind => { crate::tooling::canvas_input(rt, kind, (x, y)); }
        }
    }

    fn leave(&mut self) { self.rt.pointer_leave(); }
    fn find(&self, key: &str) -> Option<(f32, f32)> { self.rt.find(key) }
    fn pointer(&self) -> Option<(f32, f32)> { self.rt.pointer() }
    fn shortcut(&self, shortcut: &crate::Shortcut) -> Result<(), crate::ShortcutError> { self.shared.shortcuts.fire(shortcut) }

    fn reopen_candidate(&self) -> crate::reopen::Candidate {
        crate::reopen::Candidate {
            reopenable: !self.finished && !self.panel,
            visible: self.visible,
            // A headless desktop has no Dock to minimize into.
            minimized: false,
            hidden_at: self.hidden_at,
            main: self.main,
        }
    }

    fn reopen_hook(&mut self, has_visible_windows: bool) -> bool {
        let Some(f) = self.on_reopen.filter(|_| !self.finished) else { return false };
        let mut handled = false;
        self.rt.update(|s, tx| handled = f(s, has_visible_windows, tx));
        handled
    }

    fn reopen(&mut self) -> Changes {
        let mut changes = Changes::default();
        self.request(WindowRequest::Open(None), &mut changes);
        changes
    }
    fn menu(&self, item: &str) -> Result<(), crate::MenuError> { crate::native_menu::fire(&self.shared, item) }
    fn popup_open(&self) -> bool { self.rt.popup_menu().is_some() }
    fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError> { self.rt.resolve_popup(picked) }
    fn file_dialog_open(&self) -> bool { self.rt.file_dialog().is_some() }
    fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), crate::FileDialogError> { self.rt.resolve_file_dialog(path) }

    fn render(&mut self, gpu: &Gpu, slot: &Slot, px: (u32, u32), dt: f64, update_ms: f64) {
        let format = if self.rt.srgb_blend() { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
        let view = if self.rt.srgb_blend() { &slot.srgb_blend_view } else { &slot.view };
        let renderer = match &mut self.renderer {
            Some(renderer) => {
                crate::window::rebuild_renderer(&gpu.device, renderer, format, &mut self.rt);
                renderer
            }
            None => {
                let mut renderer = Renderer::new(&gpu.device, format);
                renderer.set_srgb_blend(self.rt.srgb_blend());
                self.renderer.insert(renderer)
            }
        };
        renderer.upload_images(&gpu.device, &gpu.queue, &mut self.rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &self.rt.surfaces, px);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        if let Some(c) = self.comp.as_mut() {
            let mut f = crate::compose::Frame {
                device: &gpu.device, queue: &gpu.queue, encoder: &mut enc, target: view, format,
                size: px, dt, update_ms, rt: &mut self.rt, ui: renderer,
            };
            c.render(&mut f);
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut self.rt.text.atlas, &self.rt.instances, px);
            renderer.draw(&mut enc, view, self.rt.clear_color());
        }
        renderer.submit(&gpu.queue, enc);
    }

    fn backdrop_dark(&self) -> Option<bool> { self.rt.backdrop_dark() }

    fn tree(&self, t: f64) -> Value {
        let mut tree = crate::tooling::tree_json(&self.rt, t);
        if let Some(placed) = &self.placed { tree["placed"] = placed.clone(); }
        tree
    }

    fn trace(&mut self, trace: &mut Trace, t_ms: f64, cpu_ms: f64) { trace.frame(&mut self.rt, t_ms, cpu_ms); }
    fn summary(&mut self, device: &wgpu::Device) -> Option<String> { self.comp.as_mut().and_then(|c| c.summary(device)) }
}

/// The windows, back to front, and which has the keyboard.
struct Desk {
    members: Vec<Box<dyn Member>>,
    /// Indices into `members`, bottom first.
    z: Vec<usize>,
    key: Option<usize>,
    /// The app is frontmost: its key window has focus.
    active: bool,
    /// The window under the pointer, and the one a press captured.
    hover: Option<usize>,
    pressed: Option<usize>,
    /// The window a script's file drag is over.
    files: Option<usize>,
    /// Output origin in global points, and points to pixels.
    origin: (f32, f32),
    scale: f32,
}

impl Desk {
    fn focus(&mut self, key: Option<usize>, active: bool) {
        let before = self.key.filter(|_| self.active);
        let after = key.filter(|_| active);
        if before != after {
            if let Some(i) = before { self.members[i].focus(false); }
            if let Some(i) = after { self.members[i].focus(true); }
        }
        self.key = key;
        self.active = active;
    }

    /// Brings `i` to the front of its level; always-on-top windows stay above the rest.
    fn front(&mut self, i: usize) {
        self.z.retain(|&j| j != i);
        self.z.push(i);
        let members = &self.members;
        self.z.sort_by_key(|&j| members[j].on_top());
    }

    /// The window that gets the keyboard when no window asks for it: the topmost visible one
    /// that takes clicks.
    fn topmost_visible(&self) -> Option<usize> {
        self.z.iter().rev().copied().find(|&i| self.can_key(i))
    }

    /// Whether window `i` can have the keyboard: it shows and takes clicks.
    fn can_key(&self, i: usize) -> bool {
        self.members[i].visible() && !self.members[i].click_through()
    }

    fn apply(&mut self, i: usize, changes: Changes) {
        if changes.front && self.members[i].visible() { self.front(i); }
        if changes.keys && self.can_key(i) { self.focus(Some(i), true); }
    }

    /// A hidden key window hands the keyboard to the topmost visible one.
    fn settle_key(&mut self) {
        if self.key.is_some_and(|k| self.can_key(k)) { return; }
        let next = self.topmost_visible();
        if next != self.key { self.focus(next, self.active); }
        for i in [self.hover, self.pressed].into_iter().flatten() {
            if !self.members[i].visible() {
                if self.hover == Some(i) { self.hover = None; }
                if self.pressed == Some(i) { self.pressed = None; }
            }
        }
    }

    /// Applies what every window asked, again while draining one window makes requests of
    /// another (a focus change runs `on_focus`, which may close its window or update others).
    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) {
        for _ in 0..PASSES {
            for i in 0..self.members.len() {
                let changes = self.members[i].drain(desktop, t);
                self.apply(i, changes);
            }
            self.settle_key();
            if !self.members.iter().any(|m| m.pending()) { return; }
        }
        eprintln!("rune: windows still made requests of each other after {PASSES} passes at t = {t:.3}; the rest wait a frame");
    }

    /// Steps, without time passing, each shown window whose frame no longer shows what this
    /// frame's handlers asked of it, so the frame draws their result rather than a frame later.
    fn catch_up(&mut self, desktop: &VirtualDesktop, t: f64) {
        for _ in 0..PASSES {
            let mut stepped = false;
            for m in &mut self.members {
                if m.visible() && m.stale() { m.advance(0.0); stepped = true; }
            }
            if !stepped { return; }
            self.drain(desktop, t);
        }
    }

    /// Top-left in output pixels, rounded so window pixels land on output pixels.
    fn origin_px(&self, i: usize) -> (f32, f32) {
        let f = self.members[i].frame();
        (((f[0] - self.origin.0) * self.scale).round(), ((f[1] - self.origin.1) * self.scale).round())
    }

    fn origin_pt(&self, i: usize) -> (f32, f32) {
        let (x, y) = self.origin_px(i);
        (x / self.scale, y / self.scale)
    }

    fn hit(&self, (x, y): (f32, f32)) -> Option<usize> {
        self.z.iter().rev().copied().find(|&i| {
            let m = &self.members[i];
            let (ox, oy) = self.origin_pt(i);
            let f = m.frame();
            m.visible() && !m.click_through() && x >= ox && y >= oy && x < ox + f[2] && y < oy + f[3]
        })
    }

    /// The element's centre in output points: the key window first, then front to back.
    fn find(&self, key: &str) -> Option<(f32, f32)> {
        let order = self.key.into_iter().chain(self.z.iter().rev().copied().filter(|&i| Some(i) != self.key));
        order.filter(|&i| self.members[i].visible()).find_map(|i| {
            let (x, y) = self.members[i].find(key)?;
            let (ox, oy) = self.origin_pt(i);
            Some((x + ox, y + oy))
        })
    }

    fn pointer(&self) -> Option<(f32, f32)> {
        let i = self.hover?;
        let (x, y) = self.members[i].pointer()?;
        let (ox, oy) = self.origin_pt(i);
        Some((x + ox, y + oy))
    }

    /// Pointer input at `at` in output points: to the window a press captured, else the one under it.
    fn pointer_input(&mut self, kind: &Kind, at: (f32, f32)) {
        let captured = (matches!(kind, Kind::Move) || kind.is_up()).then_some(self.pressed).flatten();
        let target = captured.or_else(|| self.hit(at));
        if (matches!(kind, Kind::Move) || kind.is_down()) && self.hover != target && captured.is_none() {
            if let Some(h) = self.hover { self.members[h].leave(); }
            self.hover = target;
        }
        if kind.is_down() {
            self.pressed = target;
            match target {
                // A click brings its window to the front with the keyboard, activating the app.
                Some(i) => { self.front(i); self.focus(Some(i), true); }
                // A click on the backdrop activates another app.
                None => self.focus(self.key, false),
            }
        }
        if kind.is_up() { self.pressed = None; }
        if let Some(i) = target {
            let (ox, oy) = self.origin_pt(i);
            self.members[i].input(kind, (at.0 - ox, at.1 - oy));
        }
    }

    /// A script's file drag goes to the window under it, whether or not that window is key; a
    /// drag that moves off a window leaves it.
    fn file_input(&mut self, step: &crate::tooling::FileStep, at: (f32, f32)) {
        use crate::tooling::FileStep;
        let target = if matches!(step, FileStep::Leave) { self.files } else { self.hit(at) };
        if let Some(old) = self.files.filter(|&old| Some(old) != target) {
            self.members[old].input(&Kind::Files(FileStep::Leave), (0.0, 0.0));
        }
        self.files = target.filter(|_| matches!(step, FileStep::Hover(_)));
        if let Some(i) = target {
            let (ox, oy) = self.origin_pt(i);
            self.members[i].input(&Kind::Files(step.clone()), (at.0 - ox, at.1 - oy));
        }
    }

    /// The app reopened: every window's `on_reopen` runs, then, unless one handled it or the app
    /// is an accessory, the default shows a window when none does (`reopen::target`) and
    /// activates the app with it in front.
    fn reopen(&mut self, accessory: bool, desktop: &VirtualDesktop, t: f64) {
        let candidates = |desk: &Self| -> Vec<_> { desk.members.iter().map(|m| m.reopen_candidate()).collect() };
        let has_visible = crate::reopen::has_visible(&candidates(self));
        let mut handled = false;
        for m in &mut self.members { handled |= m.reopen_hook(has_visible); }
        if handled || accessory { return; }
        // The default sees what the hooks did.
        self.drain(desktop, t);
        if let Some(i) = crate::reopen::target(&candidates(self)) {
            let changes = self.members[i].reopen();
            self.apply(i, changes);
        }
    }

    /// Window `i`'s frame in output points, whether it shows, and whether it has the keyboard
    /// (never while another app is active).
    fn state(&self, i: usize) -> ([f32; 4], bool, bool) {
        let m = &self.members[i];
        let f = m.frame();
        ([f[0] - self.origin.0, f[1] - self.origin.1, f[2], f[3]], m.visible(), self.active && self.key == Some(i))
    }

    fn snapshot(&self, i: usize, t: f64) -> WindowFrame {
        let (frame, visible, focused) = self.state(i);
        WindowFrame { t, key: self.members[i].key().to_owned(), frame, visible, focused }
    }
}

/// Renders `windows` composited over the `--desktop` backdrop. `selected` is the window
/// `--scene`, `--tree` and `--trace` describe.
/// `accessory` is `RunOptions::accessory`: reopening brings back no window.
pub(crate) fn run(windows: Vec<AnyWindowSpec>, selected: usize, args: Args, accessory: bool) {
    let launched = Instant::now();
    let backdrop = args.desktop.clone().expect("--desktop");
    if args.list_scenes { die("--list-scenes lists one window's scenes; drop --desktop"); }
    let source = crate::tooling::read_script(&args);
    let script = source.as_ref().map(crate::tooling::load_script).unwrap_or_default();
    let timeline = Timeline::of(&args, &script).unwrap_or_else(|e| die(&e));
    let scale = args.scale.unwrap_or(2.0);
    let desktop = crate::tooling::desktop(&args, source.as_ref());
    let primary = desktop.primary().frame.map(|v| v as f32);
    let size = args.size.unwrap_or((primary[2], primary[3]));
    let frames_log = Arc::new(std::sync::Mutex::new(Vec::new()));
    crate::headless_run::publish(crate::headless_run::HeadlessRun {
        script: args.script.clone(),
        events: script.iter().map(Ev::public).collect(),
        scale,
        fps: timeline.fps,
        size,
        start: timeline.start_frame as f64 * timeline.dt,
        frames: timeline.frames,
        end: timeline.last_frame as f64 * timeline.dt,
        desktop: Some(backdrop.clone()),
        windows: frames_log.clone(),
    });

    let mut members: Vec<Box<dyn Member>> = windows.into_iter().enumerate()
        .map(|(i, w)| w.desktop(&args, i == selected, scale)).collect();
    for m in &mut members { m.start(&desktop); }
    let Timeline { fps, dt, start_frame, frames, snap_frame, last_frame } = timeline;
    let mut trace = args.trace.as_ref().map(|p| {
        let f = members[selected].frame();
        members[selected].trace_on();
        let mut trace = Trace::open(p, (f[2], f[3]), fps, scale);
        trace.all = args.trace_all;
        trace
    });
    let n = members.len();
    let mut desk = Desk {
        members, z: (0..n).collect(), key: None, active: true, hover: None, pressed: None, files: None,
        origin: (primary[0], primary[1]), scale,
    };
    let members = &desk.members;
    desk.z.sort_by_key(|&i| members[i].on_top());
    let initial = desk.topmost_visible();
    desk.focus(initial, true);

    let t0 = Instant::now();
    let features = desk.members.iter().fold(wgpu::Features::empty(), |f, m| f | m.features());
    let gpu = Gpu::try_with_features(Gpu::instance(), None, features).unwrap_or_else(|e| die(&e));
    let max = gpu.device.limits().max_texture_dimension_2d;
    let (wp, hp) = pixels(size, scale, max).unwrap_or_else(|| die(&format!(
        "--desktop: the output, {}x{} points at --scale {scale}, must be 1 to {max} pixels a side", size.0, size.1)));
    let comp = Composite::new(&gpu, &backdrop, (wp, hp));
    let mut writer = crate::record::Writer::new((wp, hp));
    if let Some(path) = &args.video {
        writer.video(path, fps).unwrap_or_else(|e| die(&e));
    }
    let target = Offscreen::new(&gpu.device, wp, hp);
    let mut readback = crate::record::Readback::new(&gpu.device, &target, crate::record::DEPTH);
    let mut slots: Vec<Option<Slot>> = (0..n).map(|_| None).collect();
    let mut overlay = (args.keys.is_some() || args.pointer)
        .then(|| (crate::keycaps::Overlay::new(size, scale, args.keys, args.keys_theme, args.pointer), Renderer::new(&gpu.device, OFFSCREEN_FORMAT)));
    let dark = args.theme.as_deref() != Some("light");
    let init_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut strip: Vec<Vec<u8>> = Vec::new();
    let crops_px: Vec<[u32; 4]> = args.crops.iter().map(|c| c.map(|v| (v * scale).round() as u32)).collect();
    let gap = (4.0 * scale) as u32;
    let mut frames_written = 0usize;
    let has_video = writer.has_video();
    let mut deliver = |shot: Shot, px: Vec<u8>| {
        if shot.dump {
            if args.strip.is_some() { strip.push(crate::tooling::tile(&px, wp, hp, &crops_px, gap)); }
            if let Some(dir) = &args.out {
                writer.png(dir.join(format!("frame_{:04}.png", shot.k - start_frame)), px.clone()).unwrap_or_else(|e| die(&e));
            }
        }
        if shot.snap { writer.png(args.snap.clone().unwrap(), px.clone()).unwrap_or_else(|e| die(&e)); }
        if shot.video {
            frames_written += 1;
            writer.frame(px).unwrap_or_else(|e| die(&e));
        }
    };

    // Logs a window's frame when it moves, shows, hides or takes or loses the keyboard.
    let mut recorded = vec![None; n];
    let mut record = |desk: &Desk, t: f64| {
        let mut log = frames_log.lock().unwrap();
        for (i, last) in recorded.iter_mut().enumerate() {
            let now = desk.state(i);
            if *last != Some(now) {
                log.push(desk.snapshot(i, t));
                *last = Some(now);
            }
        }
    };

    // Events at t = 0 find keys and hit elements in a laid-out first frame (see `headless_configured`).
    if script.iter().find(|e| !matches!(e.kind, Kind::Mark(_))).is_some_and(|e| e.t <= 1e-9) {
        desk.drain(&desktop, 0.0);
        for m in &mut desk.members { m.advance(0.0); }
        desk.drain(&desktop, 0.0);
    }

    let wall = Instant::now();
    let mut cpu_ms = Vec::new();
    let mut next_ev = 0;
    let mut last_down = (0.0f32, 0.0f32);
    let mut drawn: Vec<(usize, bool)> = Vec::with_capacity(n);
    for k in 0..=last_frame {
        let real_t = k as f64 * dt;
        let frame_start = Instant::now();
        while next_ev < script.len() && script[next_ev].t <= real_t + 1e-9 {
            let e = &script[next_ev];
            next_ev += 1;
            let at = || match &e.pos {
                Pos::Xy(x, y) => (*x, *y),
                Pos::Key(key) => desk.find(key).unwrap_or_else(|| die(&format!("script: no element with key {key:?} in a visible window"))),
                Pos::Rel(_, _) if e.kind.at_pointer() => desk.pointer().unwrap_or((0.0, 0.0)),
                Pos::Rel(dx, dy) => (last_down.0 + dx, last_down.1 + dy),
            };
            let (x, y) = at();
            match &e.kind {
                Kind::Mark(_) => {}
                Kind::Focus(focused) => desk.focus(desk.key, *focused),
                Kind::Reopen => desk.reopen(accessory, &desktop, real_t),
                Kind::Shortcut(shortcut) => fire(&desk, |m| m.shortcut(shortcut)).unwrap_or_else(|e| die(&format!("script shortcut: {e}"))),
                Kind::Menu(item) => fire(&desk, |m| m.menu(item)).unwrap_or_else(|e| die(&format!("script menu: {e}"))),
                Kind::PopupMenu(picked) => {
                    let open = desk.key.into_iter().chain(desk.z.iter().rev().copied()).find(|&i| desk.members[i].popup_open());
                    let i = open.or(desk.key).unwrap_or(selected);
                    desk.members[i].resolve_popup(picked.as_deref()).unwrap_or_else(|e| die(&format!("script pop-up menu: {e}")));
                }
                Kind::FileDialog(path) => {
                    let open = desk.key.into_iter().chain(desk.z.iter().rev().copied()).find(|&i| desk.members[i].file_dialog_open());
                    let i = open.or(desk.key).unwrap_or(selected);
                    desk.members[i].resolve_file_dialog(path.clone()).unwrap_or_else(|e| die(&format!("script file dialog: {e}")));
                }
                // While another app is active, as after a click on the backdrop, keys go to it.
                Kind::Key(..) | Kind::KeyUp(_) | Kind::Physical(_) | Kind::PhysicalKey(..) | Kind::Paste(_) => {
                    if let Some(i) = desk.key.filter(|_| desk.active) { desk.members[i].input(&e.kind, (x, y)); }
                }
                Kind::Files(step) => desk.file_input(step, (x, y)),
                // The rest are pointer input.
                kind => {
                    if kind.is_down() { last_down = (x, y); }
                    desk.pointer_input(&e.kind, (x, y));
                }
            }
            if let Some((ov, _)) = overlay.as_mut() {
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
        desk.drain(&desktop, real_t);
        let step = if k == 0 { 0.0 } else { dt };
        for m in &mut desk.members { m.advance(step); }
        // Handlers that ran this frame may show, hide, place, focus or close windows, or update
        // a window that already stepped.
        desk.drain(&desktop, real_t);
        desk.catch_up(&desktop, real_t);
        record(&desk, real_t);
        let update_ms = frame_start.elapsed().as_secs_f64() * 1000.0;

        let key = desk.key.filter(|_| desk.active);
        drawn.clear();
        for &i in &desk.z {
            let m = &mut desk.members[i];
            if !m.visible() { continue; }
            let f = m.frame();
            let px = (((f[2] * scale).round() as u32).max(1), ((f[3] * scale).round() as u32).max(1));
            if px.0.max(px.1) > max {
                die(&format!("--desktop: window {:?} is {}x{} pixels, over the GPU's {max} a side", m.key(), px.0, px.1));
            }
            if slots[i].as_ref().is_none_or(|s| s.px != px) { slots[i] = Some(Slot::new(&gpu.device, &comp, px, scale)); }
            let slot = slots[i].as_ref().unwrap();
            m.render(&gpu, slot, px, step, update_ms);
            let (ox, oy) = desk.origin_px(i);
            let (sigma, offset, opacity) = if key == Some(i) { KEY_SHADOW } else { SHADOW };
            slot.write(&gpu.queue, Uniform {
                origin: [ox, oy], size: [px.0 as f32, px.1 as f32], out_size: [wp as f32, hp as f32],
                shadow_offset: [0.0, offset * scale], mask_size: [slot.mask.0 as f32, slot.mask.1 as f32],
                radius: desk.members[i].radius() * scale, pad: slot.pad as f32, sigma: sigma * scale / DOWN as f32,
                opacity, _unused: [0.0; 2],
            });
            drawn.push((i, desk.members[i].shadow()));
        }
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        comp.draw(&mut enc, &target.view, drawn.iter().map(|&(i, shadow)| (slots[i].as_ref().unwrap(), shadow)));
        if let Some((ov, ui)) = overlay.as_mut() {
            let app_dark = key.and_then(|i| desk.members[i].backdrop_dark()).unwrap_or(dark);
            let ort = ov.frame(real_t, step, app_dark);
            if !ort.instances.is_empty() {
                ui.prepare(&gpu.device, &gpu.queue, &mut ort.text.atlas, &ort.instances, (wp, hp));
                ui.draw_over(&mut enc, &target.view);
            }
        }
        let in_range = k >= start_frame && k < start_frame + frames;
        let shot = Shot {
            k,
            dump: in_range && (args.out.is_some() || args.strip.is_some()) && (k - start_frame).is_multiple_of(args.every),
            video: in_range && has_video,
            snap: k == snap_frame && args.snap.is_some(),
        };
        let first_pixels = args.first_pixels && k == 0;
        let want_pixels = shot.dump || shot.video || shot.snap || first_pixels;
        if want_pixels {
            for (shot, px) in readback.make_room(&gpu.device) { deliver(shot, px); }
            readback.copy(&mut enc, &target, shot);
        }
        gpu.queue.submit([enc.finish()]);
        if want_pixels { readback.submitted(); }
        let ms = frame_start.elapsed().as_secs_f64() * 1000.0;
        cpu_ms.push(ms);
        if let Some(tr) = trace.as_mut() { desk.members[selected].trace(tr, real_t * 1000.0, ms); }
        if first_pixels {
            let done = readback.finish(&gpu.device);
            let marker = json!({
                "headless_ms": launched.elapsed().as_secs_f64() * 1000.0, "gpu_init_ms": init_ms, "frame_cpu_ms": ms,
                "pixel_bytes": done.last().map_or(0, |(_, px)| px.len()),
            });
            println!("rune-first-pixels {marker}");
            for (shot, px) in done { deliver(shot, px); }
        } else if want_pixels {
            for (shot, px) in readback.ready(&gpu.device) { deliver(shot, px); }
        }
        if k == snap_frame && let Some(p) = &args.tree {
            let mut tree = desk.members[selected].tree(real_t);
            tree["windows"] = (0..n).map(|i| {
                let f = desk.snapshot(i, real_t);
                json!({ "key": f.key, "frame": f.frame, "visible": f.visible, "focused": f.focused })
            }).collect();
            let text = serde_json::to_string_pretty(&tree).unwrap();
            std::fs::write(p, text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
        }
    }
    for (shot, px) in readback.finish(&gpu.device) { deliver(shot, px); }
    drop(deliver);
    if let Some(video) = writer.finish().unwrap_or_else(|e| die(&e)) {
        eprintln!("rune: wrote {} ({frames_written} frames at {fps} fps)", video.display());
    }
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    if let Some(p) = &args.strip { crate::tooling::write_strip(p, &strip, (wp, hp), &crops_px, gap, args.cols); }
    let wall = wall.elapsed().as_secs_f64();
    let mean = cpu_ms.iter().sum::<f64>() / cpu_ms.len() as f64;
    eprintln!("rune: desktop of {n} windows, {} frames in {wall:.2} s wall, gpu init {init_ms:.1} ms, frame cpu mean {mean:.3} ms", last_frame + 1);
    for m in &mut desk.members {
        if let Some(s) = m.summary(&gpu.device) { eprintln!("{s}"); }
    }
}

/// Sends a shortcut or menu pick to the key window's app, else the first other app that knows it.
fn fire<E>(desk: &Desk, f: impl Fn(&dyn Member) -> Result<(), E>) -> Result<(), E> {
    let order = desk.key.into_iter().chain(desk.z.iter().rev().copied().filter(|&i| Some(i) != desk.key));
    let mut first = None;
    for i in order {
        match f(desk.members[i].as_ref()) {
            Ok(()) => return Ok(()),
            Err(e) => { first.get_or_insert(e); }
        }
    }
    first.map_or(Ok(()), Err)
}

// -------------------------------------------------------------------------------------------
// Compositing

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    /// Window top-left and size, in output pixels.
    origin: [f32; 2],
    size: [f32; 2],
    out_size: [f32; 2],
    shadow_offset: [f32; 2],
    mask_size: [f32; 2],
    radius: f32,
    /// Window pixels of shadow mask around the window.
    pad: f32,
    /// Blur sigma in mask texels.
    sigma: f32,
    opacity: f32,
    _unused: [f32; 2],
}

const SHADER: &str = r#"
struct W {
    origin: vec2<f32>,
    size: vec2<f32>,
    out_size: vec2<f32>,
    shadow_offset: vec2<f32>,
    mask_size: vec2<f32>,
    radius: f32,
    pad: f32,
    sigma: f32,
    opacity: f32,
    unused: vec2<f32>,
};
@group(0) @binding(0) var<uniform> w: W;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

const DOWN: f32 = 4.0;

fn corner(i: u32) -> vec2<f32> {
    let c = array(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    return c[i];
}
fn clip(p: vec2<f32>) -> vec4<f32> {
    return vec4(p.x / w.out_size.x * 2.0 - 1.0, 1.0 - p.y / w.out_size.y * 2.0, 0.0, 1.0);
}
@vertex fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let points = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(points[i], 0.0, 1.0);
}
@vertex fn vs_window(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return clip(w.origin + corner(i) * w.size);
}
@vertex fn vs_shadow(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return clip(w.origin + w.shadow_offset - vec2(w.pad) + corner(i) * (w.size + 2.0 * w.pad));
}

// Signed distance to the window's rounded rect; p in window pixels.
fn rounded(p: vec2<f32>) -> f32 {
    let half = w.size * 0.5;
    let r = min(w.radius, min(half.x, half.y));
    let q = abs(p - half) - half + vec2(r);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment fn fs_backdrop(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(tex, vec2<i32>(p.xy), 0);
}

// Premultiplied window pixels, cut to the rounded rect.
@fragment fn fs_window(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let local = p.xy - w.origin;
    let color = textureLoad(tex, vec2<i32>(floor(local)), 0);
    return color * clamp(0.5 - rounded(local), 0.0, 1.0);
}

// The window's alpha inside its corners, DOWN x DOWN pixels to a texel.
@fragment fn fs_mask(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let base = floor(p.xy) * DOWN - vec2(w.pad);
    var sum = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let q = base + vec2(f32(x), f32(y));
            if all(q >= vec2(0.0)) && all(q < w.size) {
                sum += textureLoad(tex, vec2<i32>(q), 0).a * clamp(0.5 - rounded(q + 0.5), 0.0, 1.0);
            }
        }
    }
    return vec4(sum / 16.0, 0.0, 0.0, 1.0);
}

fn blur(p: vec2<f32>, axis: vec2<i32>) -> vec4<f32> {
    let c = vec2<i32>(p);
    let dims = vec2<i32>(textureDimensions(tex));
    let sigma = max(w.sigma, 0.5);
    let r = i32(ceil(sigma * 3.0));
    var sum = 0.0;
    var total = 0.0;
    for (var k = -r; k <= r; k++) {
        let q = c + axis * k;
        let weight = exp(-f32(k * k) / (2.0 * sigma * sigma));
        total += weight;
        if all(q >= vec2(0)) && all(q < dims) {
            sum += weight * textureLoad(tex, q, 0).r;
        }
    }
    return vec4(sum / total, 0.0, 0.0, 1.0);
}
@fragment fn fs_blur_x(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return blur(p.xy, vec2(1, 0)); }
@fragment fn fs_blur_y(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return blur(p.xy, vec2(0, 1)); }

@fragment fn fs_shadow(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let m = (p.xy - w.origin - w.shadow_offset + vec2(w.pad)) / DOWN;
    let a = textureSample(tex, samp, m / w.mask_size).r * w.opacity;
    return vec4(0.0, 0.0, 0.0, a);
}
"#;

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// The backdrop and the pipelines that draw windows and their shadows over it.
struct Composite {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    backdrop: wgpu::BindGroup,
    full: wgpu::RenderPipeline,
    window: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    blur: [wgpu::RenderPipeline; 2],
}

impl Composite {
    fn new(gpu: &Gpu, backdrop: &Path, size: (u32, u32)) -> Composite {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("desktop"), source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("desktop"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, visibility: wgpu::ShaderStages::VERTEX | fragment, count: None,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, visibility: fragment, count: None,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: fragment, count: None, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering) },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("desktop"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = |vs: &str, fs: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs), layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None, cache: None,
            })
        };
        let over = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("desktop shadow"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default()
        });
        let pixels = fit_backdrop(backdrop, size).unwrap_or_else(|e| die(&e));
        let (texture, view) = new_texture(device, "desktop backdrop", size, OFFSCREEN_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0 * 4), rows_per_image: Some(size.1) },
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
        let uniform = uniform_buffer(device);
        let backdrop = bind(device, &layout, &uniform, &view, &sampler);
        Composite {
            full: pipeline("vs_full", "fs_backdrop", OFFSCREEN_FORMAT, None),
            window: pipeline("vs_window", "fs_window", OFFSCREEN_FORMAT, over),
            shadow: pipeline("vs_shadow", "fs_shadow", OFFSCREEN_FORMAT, over),
            mask: pipeline("vs_full", "fs_mask", MASK_FORMAT, None),
            blur: [pipeline("vs_full", "fs_blur_x", MASK_FORMAT, None), pipeline("vs_full", "fs_blur_y", MASK_FORMAT, None)],
            layout, sampler, backdrop,
        }
    }

    /// The backdrop, then each window's shadow and pixels, bottom first.
    fn draw<'a>(&self, enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, windows: impl Iterator<Item = (&'a Slot, bool)> + Clone) {
        for (slot, _) in windows.clone().filter(|(_, shadow)| *shadow) {
            let passes = [(&self.mask, &slot.window, 0), (&self.blur[0], &slot.masks[0].2, 1), (&self.blur[1], &slot.masks[1].2, 0)];
            for (pipeline, source, into) in passes {
                let mut pass = begin(enc, &slot.masks[into].1, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, source, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        let mut pass = begin(enc, target, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
        pass.set_pipeline(&self.full);
        pass.set_bind_group(0, &self.backdrop, &[]);
        pass.draw(0..3, 0..1);
        for (slot, shadow) in windows {
            if shadow {
                pass.set_pipeline(&self.shadow);
                pass.set_bind_group(0, &slot.masks[0].2, &[]);
                pass.draw(0..6, 0..1);
            }
            pass.set_pipeline(&self.window);
            pass.set_bind_group(0, &slot.window, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

/// One window's texture, its shadow masks and what binds them.
pub(crate) struct Slot {
    px: (u32, u32),
    view: wgpu::TextureView,
    srgb_blend_view: wgpu::TextureView,
    _texture: wgpu::Texture,
    /// Ping-pong masks: texture, view, and a bind group that reads it.
    masks: [(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup); 2],
    mask: (u32, u32),
    pad: u32,
    uniform: wgpu::Buffer,
    window: wgpu::BindGroup,
}

impl Slot {
    fn new(device: &wgpu::Device, comp: &Composite, px: (u32, u32), scale: f32) -> Slot {
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let non_srgb = OFFSCREEN_FORMAT.remove_srgb_suffix();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("desktop window"),
            size: wgpu::Extent3d { width: px.0.max(1), height: px.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT, usage, view_formats: &[non_srgb],
        });
        let view = texture.create_view(&Default::default());
        let srgb_blend_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(non_srgb),
            ..Default::default()
        });
        // Room for the key window's blur, whole mask texels on each side.
        let pad = ((KEY_SHADOW.0 * scale * 3.0) / DOWN as f32).ceil() as u32 * DOWN;
        let mask = ((px.0 + 2 * pad).div_ceil(DOWN), (px.1 + 2 * pad).div_ceil(DOWN));
        let uniform = uniform_buffer(device);
        let masks = [0, 1].map(|_| {
            let (t, v) = new_texture(device, "desktop shadow", mask, MASK_FORMAT, usage);
            let b = bind(device, &comp.layout, &uniform, &v, &comp.sampler);
            (t, v, b)
        });
        let window = bind(device, &comp.layout, &uniform, &view, &comp.sampler);
        Slot { px, view, srgb_blend_view, _texture: texture, masks, mask, pad, uniform, window }
    }

    fn write(&self, queue: &wgpu::Queue, uniform: Uniform) {
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }
}

fn begin<'e>(enc: &'e mut wgpu::CommandEncoder, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'e> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("desktop"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
    })
}

fn new_texture(device: &wgpu::Device, label: &str, size: (u32, u32), format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label), size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn uniform_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("desktop window"), size: std::mem::size_of::<Uniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    })
}

fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniform: &wgpu::Buffer, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("desktop"), layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// The backdrop scaled to cover `size` pixels, centred and cropped, as opaque sRGB RGBA.
fn fit_backdrop(path: &Path, (w, h): (u32, u32)) -> Result<Vec<u8>, String> {
    let fail = |e: ::image::ImageError| format!("{}: {e}", path.display());
    let (iw, ih) = ::image::image_dimensions(path).map_err(fail)?;
    if iw.max(ih) > MAX_BACKDROP {
        return Err(format!("{}: {iw}x{ih} pixels; a backdrop is at most {MAX_BACKDROP} pixels a side", path.display()));
    }
    let image = ::image::open(path).map_err(fail)?.into_rgba8();
    let (iw, ih) = image.dimensions();
    // The centred part of the source with the output's aspect, then scaled to the output.
    let (cw, ch) = if u64::from(iw) * u64::from(h) > u64::from(ih) * u64::from(w) {
        (((f64::from(ih) * f64::from(w) / f64::from(h)).round() as u32).clamp(1, iw), ih)
    } else {
        (iw, ((f64::from(iw) * f64::from(h) / f64::from(w)).round() as u32).clamp(1, ih))
    };
    let crop = ::image::imageops::crop_imm(&image, (iw - cw) / 2, (ih - ch) / 2, cw, ch);
    let image = if (cw, ch) == (w, h) { crop.to_image() } else { ::image::imageops::resize(&*crop, w, h, ::image::imageops::FilterType::CatmullRom) };
    let len = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).ok_or_else(|| format!("{w}x{h} pixels is too large"))?;
    let mut out = Vec::with_capacity(len);
    for p in image.pixels() {
        let [r, g, b, a] = p.0;
        let over = |c: u8| (u32::from(c) * u32::from(a) / 255) as u8;
        out.extend([over(r), over(g), over(b), 255]);
    }
    Ok(out)
}

/// `size` points at `scale` in whole pixels, when a texture of at most `max` a side holds them.
fn pixels(size: (f32, f32), scale: f32, max: u32) -> Option<(u32, u32)> {
    let side = |v: f32| {
        let px = (v * scale).round();
        (px >= 1.0 && px <= max as f32).then_some(px as u32)
    };
    Some((side(size.0)?, side(size.1)?))
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn output_pixels_fit_a_texture() {
        assert_eq!(pixels((1512.0, 982.0), 2.0, 8192), Some((3024, 1964)));
        assert_eq!(pixels((0.2, 10.0), 2.0, 8192), None, "under a pixel");
        assert_eq!(pixels((5000.0, 10.0), 2.0, 8192), None, "over the GPU's limit");
        assert_eq!(pixels((f32::NAN, 10.0), 2.0, 8192), None);
        assert_eq!(pixels((10.0, 10.0), f32::INFINITY, 8192), None);
    }
}
