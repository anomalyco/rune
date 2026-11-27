//! Desktop development only. Use `scripts/rune-dev`: it validates edits before dx sees them.
//! Long-lived state belongs to Runtime, never to tip-crate statics or cached view trait objects.

use std::any::Any;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::render::{Gpu, OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR, Offscreen, Renderer};
use crate::runtime::{AppDriver, Env, RuntimeCore, Waker};
use crate::tooling::{AppSpec, Args, runtime_for_configured, tree_json_core};

thread_local! {
    static ORIGINAL_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with views built by their startup code, skipping the patch table: the fallback
/// after a patched view, or a row it built, panicked.
pub(crate) fn original_only<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) { ORIGINAL_ONLY.set(self.0); }
    }
    let _reset = Reset(ORIGINAL_ONLY.replace(true));
    f()
}

/// Whether views run as built at startup, inside [`original_only`].
pub(crate) fn originals_only() -> bool {
    ORIGINAL_ONLY.get()
}

/// Reuses one callback argument across a patched call and its startup fallback if the patched
/// body panics:
/// - `&mut T` reborrows for the patched attempt and hands the same mutable reference to the fallback;
/// - any `T: Clone` (`Copy` values, shared references, or owned `Clone` types such as
///   [`crate::FieldMenuRequest`]) clones for the patched attempt and moves the original into the
///   fallback on panic.
///
/// Inherent methods on `HotArg<&mut T>` take precedence over [`HotCloneArg`] via method
/// resolution, without `unsafe` or specialization.
pub(crate) struct HotArg<T>(pub(crate) T);

impl<'a, T: ?Sized> HotArg<&'a mut T> {
    #[inline(always)]
    pub(crate) fn attempt(&mut self) -> &mut T {
        self.0
    }

    #[inline(always)]
    pub(crate) fn fallback(self) -> &'a mut T {
        self.0
    }
}

pub(crate) trait HotCloneArg {
    type Output;
    fn attempt(&mut self) -> Self::Output;
    fn fallback(self) -> Self::Output;
}

impl<T: Clone> HotCloneArg for HotArg<T> {
    type Output = T;

    #[inline(always)]
    fn attempt(&mut self) -> T {
        self.0.clone()
    }

    #[inline(always)]
    fn fallback(self) -> T {
        self.0
    }
}

/// Binds arguments against `f`'s parameter types so `&mut` places (such as `self.state`)
/// reborrow automatically before wrapping each argument in [`HotArg`].
#[inline(always)]
pub(crate) fn wrap_args_1<A0, R>(_: fn(A0) -> R, a0: A0) -> (HotArg<A0>,) {
    (HotArg(a0),)
}

#[inline(always)]
pub(crate) fn wrap_args_2<A0, A1, R>(_: fn(A0, A1) -> R, a0: A0, a1: A1) -> (HotArg<A0>, HotArg<A1>) {
    (HotArg(a0), HotArg(a1))
}

#[inline(always)]
pub(crate) fn wrap_args_3<A0, A1, A2, R>(_: fn(A0, A1, A2) -> R, a0: A0, a1: A1, a2: A2) -> (HotArg<A0>, HotArg<A1>, HotArg<A2>) {
    (HotArg(a0), HotArg(a1), HotArg(a2))
}

/// Calls `f` through the patch table; if the patched code panics, calls the startup `f` once
/// instead, as a panicking view renders the original. Side effects of the panicked call stay.
///
/// Each argument expression is evaluated once into a [`HotArg`] so owned non-`Copy` `Clone`
/// arguments (such as [`crate::FieldMenuRequest`]) work alongside `&mut T` and `Copy` arguments.
macro_rules! patched_or_original {
    ($what:literal, $f:expr, $a0:expr $(,)?) => {
        $crate::hot::patched_or_original!(@invoke $what, $f, wrap_args_1, (a0), ($a0))
    };
    ($what:literal, $f:expr, $a0:expr, $a1:expr $(,)?) => {
        $crate::hot::patched_or_original!(@invoke $what, $f, wrap_args_2, (a0, a1), ($a0, $a1))
    };
    ($what:literal, $f:expr, $a0:expr, $a1:expr, $a2:expr $(,)?) => {
        $crate::hot::patched_or_original!(@invoke $what, $f, wrap_args_3, (a0, a1, a2), ($a0, $a1, $a2))
    };
    (@invoke $what:literal, $f:expr, $wrap:ident, ($($id:ident),+), ($($arg:expr),+)) => {{
        use $crate::hot::HotCloneArg as _;
        $crate::hot::HotArg(()).attempt();
        let f = $f;
        let ($(mut $id,)+) = $crate::hot::$wrap(f, $($arg),+);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            subsecond::HotFn::current(f).call(($($id.attempt(),)+))
        })) {
            Ok(value) => value,
            Err(_) => {
                eprintln!(concat!("rune-hot: patched ", $what, " panicked; calling the original"));
                f($($id.fallback()),+)
            }
        }
    }};
}
pub(crate) use patched_or_original;

/// `--snap-on-patch` for a window from `rune::run`, `App::run`, `run_windows*`,
/// `WindowSpec::from_app` or `App::window`: the same typed `Runtime<S>` a native window
/// builds, so its view and callbacks patch the way they do on screen.
pub(crate) fn snap_window<S: 'static>(app: crate::App<S>, args: Args) {
    crate::credentials::note_headless_runtime();
    let (spec, state, _, runtime) = app.into_parts(args.scene.as_deref());
    headless(&spec, state, args, runtime);
}

static ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn enable() {
    assert!(std::env::var_os("RUNE_DEV_GUARDED").is_some(), "use scripts/rune-dev: unguarded Rust patches can change live state layouts");
    if let Some(path) = std::env::var_os("RUNE_HOT_PARK") {
        // Desk mode: another Mac runs this binary. The build Mac's copy only reports where dx
        // built it, and exits before it can connect and take the patches meant for the desk.
        let exe = std::env::current_exe().expect("rune-hot: current exe");
        atomic_write(Path::new(&path), exe.as_os_str().as_encoded_bytes());
        eprintln!("rune-hot: parked {}; the desk Mac runs it", exe.display());
        std::process::exit(0);
    }
    ENABLED.store(true, Ordering::Release);
}

/// `RUNE_HOT=1` enables live editing like `--hot`, for apps that keep their argv from Rune.
pub(crate) fn enable_from_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| if std::env::var_os("RUNE_HOT").is_some_and(|v| v == "1") { enable() });
}

pub fn enabled() -> bool {
    enable_from_env();
    ENABLED.load(Ordering::Acquire)
}

/// One devserver connection per process: dx patches for the last pid and ASLR base that
/// connected, and every connection would apply each patch again. Windows share it.
#[derive(Default)]
pub(crate) struct Hub {
    generation: AtomicU64,
    next: AtomicU64,
    wakers: Mutex<Vec<(u64, Waker)>>,
}

impl Hub {
    pub(crate) fn patched(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        let wakers: Vec<Waker> = self.wakers.lock().unwrap().iter().map(|(_, w)| w.clone()).collect();
        for wake in wakers { wake(); }
    }
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    let mut fresh = false;
    let hub = HUB.get_or_init(|| { fresh = true; Hub::default() });
    if fresh {
        subsecond::register_handler(Arc::new(|| hub.patched()));
        connect();
    }
    hub
}

#[allow(unsafe_code)]
fn connect() {
    // dioxus-devtools reads the same variables and connects silently, or not at all.
    let var = rune_core::env::var;
    let Some(addr) = var("DIOXUS_DEVSERVER_IP").zip(var("DIOXUS_DEVSERVER_PORT"))
        .and_then(|(ip, port)| Some(std::net::SocketAddr::new(ip.parse().ok()?, port.parse().ok()?)))
    else {
        eprintln!("rune-hot: DIOXUS_DEVSERVER_IP/PORT are unset; edits will not patch (use scripts/rune-dev)");
        return;
    };
    if let Err(error) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
        eprintln!("rune-hot: no devserver at {addr} ({error}); edits will not patch");
        return;
    }
    let pid = std::process::id();
    dioxus_devtools::connect(move |msg| {
        if let dioxus_devtools::DevserverMsg::HotReload(msg) = msg
            && let Some(table) = msg.jump_table
            && msg.for_pid == Some(pid)
        {
            let lib = table.lib.clone();
            // SAFETY: rune-dev's guard admits only edits that keep every type, static and
            // signature; the patch was linked against this binary's ASLR reference.
            if let Err(error) = unsafe { subsecond::apply_patch(table) } {
                eprintln!("rune-hot: patch {} did not load: {error}", lib.display());
            }
        }
    });
}

pub(crate) struct Session {
    hub: &'static Hub,
    id: u64,
    seen: u64,
}

impl Session {
    pub(crate) fn connect(wake: Waker) -> Self { Self::join(hub(), wake) }

    fn join(hub: &'static Hub, wake: Waker) -> Self {
        let id = hub.next.fetch_add(1, Ordering::Relaxed);
        hub.wakers.lock().unwrap().push((id, wake));
        Self { hub, id, seen: hub.generation.load(Ordering::Acquire) }
    }

    pub(crate) fn changed(&mut self) -> bool {
        let next = self.hub.generation.load(Ordering::Acquire);
        let changed = next != self.seen;
        self.seen = next;
        changed
    }

    pub(crate) fn pending(&self) -> bool { self.hub.generation.load(Ordering::Acquire) != self.seen }

    /// Moves this session, waker and all, to a test's own hub, so a test patch reaches only it.
    #[cfg(test)]
    pub(crate) fn move_to(&mut self, hub: &'static Hub) {
        let wake = {
            let mut wakers = self.hub.wakers.lock().unwrap();
            let at = wakers.iter().position(|(id, _)| *id == self.id).unwrap();
            wakers.remove(at).1
        };
        *self = Self::join(hub, wake);
    }
}

#[cfg(test)]
pub(crate) fn enable_for_tests() { ENABLED.store(true, Ordering::Release); }

impl Drop for Session {
    fn drop(&mut self) { self.hub.wakers.lock().unwrap().retain(|(id, _)| *id != self.id); }
}

fn atomic_write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).unwrap(); }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes).unwrap();
    std::fs::rename(temp, path).unwrap();
}

#[cfg(test)]
#[inline]
fn control_input<S: 'static>(rt: &mut crate::Runtime<S>, command: &Value) {
    let (core, mut app) = rt.split_mut();
    control_input_core(core, &mut app, command);
}

fn control_input_core(core: &mut RuntimeCore, app: &mut dyn AppDriver, command: &Value) {
    if let Some(tap) = command.get("tap") {
        if let Some((x, y)) = tap.as_str().and_then(|key| core.find(key)) {
            core.pointer_move(app, x, y);
            core.pointer_down(app, x, y);
            core.pointer_up(app, x, y);
        } else {
            eprintln!("rune-hot: skipping tap: expected an existing element key");
        }
    }
    if let Some(scroll) = command.get("scroll") {
        match serde_json::from_value::<[f32; 3]>(scroll.clone()) {
            Ok([x, y, dy]) if [x, y, dy].iter().all(|v| v.is_finite()) => core.wheel(x, y, dy),
            _ => eprintln!("rune-hot: skipping scroll: expected three finite numbers [x,y,dy]"),
        }
    }
}

/// Persistent offscreen GPU session. A control file is an atomically replaced JSON request:
/// `{"id":1,"tap":"login"}`, `{"id":2,"scroll":[800,400,100]}`,
/// `{"id":3,"pause":true}`, `{"id":4,"step":0.05}`, or `{"id":5,"quit":true}`.
/// Every request and patch publishes a PNG + JSON tree, then a flushed completion marker.
#[inline]
fn headless<S: 'static>(spec: &AppSpec<S>, state: S, args: Args, runtime: crate::app::RuntimeConfig<S>) {
    assert!(enabled(), "--snap-on-patch requires --hot or RUNE_HOT=1 (use scripts/rune-dev)");
    let size = args.size.unwrap_or(spec.size);
    let scale = args.scale.unwrap_or(2.0);
    let (wake, sleep) = std::sync::mpsc::channel();
    let mut rt = runtime_for_configured(spec, state, size, Env { system_dark: args.theme.as_deref() != Some("light"), scale }, Arc::new(move || { wake.send(()).ok(); }), Some(runtime));
    let state_addr = format!("{:p}", crate::runtime::handler_state_ref(&rt.state) as *const dyn Any as *const ());
    let (core, mut app) = rt.split_mut();
    headless_core(core, &mut app, state_addr, size, scale, args, sleep);
}

fn headless_core(
    core: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    state_addr: String,
    size: (f32, f32),
    scale: f32,
    args: Args,
    sleep: std::sync::mpsc::Receiver<()>,
) {
    if let Some(bars) = args.scroll_bars { core.set_system_scroll_bars(bars); }
    let srgb_blend = core.srgb_blend();
    let target_format = if srgb_blend { OFFSCREEN_FORMAT } else { OFFSCREEN_FORMAT_LINEAR };
    let (gpu, mut renderer) = Gpu::for_headless_setup(wgpu::Features::empty(), |gpu| {
        Renderer::try_new_classified(&gpu.device, target_format)
            .map_err(crate::render::SurfaceSetupError::from_shader)
    })
    .unwrap_or_else(|e| panic!("{}", e.message));
    renderer.set_srgb_blend(srgb_blend);
    let (w, h) = ((size.0 * scale).round() as u32, (size.1 * scale).round() as u32);
    let mut target = Offscreen::with_format(&gpu.device, w, h, target_format);
    let png = args.snap_on_patch.as_ref().unwrap();
    let mut first = true;
    let mut paused = false;
    let mut command = Value::Null;
    let mut previous = crate::time::now();
    let mut woken = true;
    loop {
        let now = crate::time::now();
        let elapsed = (now - previous).as_secs_f64();
        // `None`: a frame `elapsed` after the last, under the shared frame rule.
        let mut fixed = (first || paused).then_some(0.0);
        previous = now;
        let mut requested = false;
        if !first && let Some(path) = &args.hot_control
            && let Ok(bytes) = std::fs::read(path)
            && let Ok(next) = serde_json::from_slice::<Value>(&bytes)
            && next != command
        {
            command = next;
            if command["quit"] == true { break; }
            if let Some(pause) = command["pause"].as_bool() { paused = pause; fixed = Some(0.0); }
            if let Some(step) = command["step"].as_f64() { fixed = Some(step.clamp(0.0, 10.0)); }
            control_input_core(core, app, &command);
            requested = true;
        }
        let timer_due = core.wake_deadline().is_some_and(|deadline| deadline <= core.clock + elapsed);
        if !first && !requested && !core.frame.hot().unwrap().pending() && (paused || (!woken && !core.animating && !timer_due)) {
            // Paused/idle sessions must not compete with their own patch compiler.
            if !paused { core.clock += elapsed; }
            woken = sleep.recv_timeout(Duration::from_millis(16)).is_ok();
            continue;
        }
        let before = core.frame.hot().unwrap().seen;
        match fixed {
            Some(dt) => { core.frame(app, dt); }
            None => { crate::frame::clock::advance_by_core(core, app, elapsed); }
        }
        let generation = core.frame.hot().unwrap().seen;
        if first || requested || before != generation {
            if renderer.srgb_blend() != core.srgb_blend() {
                let format = if core.srgb_blend() { OFFSCREEN_FORMAT } else { OFFSCREEN_FORMAT_LINEAR };
                crate::window::rebuild_renderer_core(&gpu.device, &mut renderer, format, core);
                target = Offscreen::with_format(&gpu.device, w, h, format);
            }
            renderer.upload_images(&gpu.device, &gpu.queue, &mut core.images);
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &core.surfaces, (w, h));
            renderer.set_fade_stops(core.fade_stops());
            renderer.prepare_ui(&gpu.device, &gpu.queue, &mut core.text.atlas, &core.instances, (w, h));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, core.clear_color());
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            let pixels = target.read(&gpu.device);
            let pixels_ns = crate::time::system_now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let temp = png.with_extension("tmp.png");
            if let Some(parent) = temp.parent() { std::fs::create_dir_all(parent).unwrap(); }
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&temp).unwrap()), w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fastest);
            encoder.write_header().unwrap().write_image_data(&pixels).expect("hot snapshot");
            std::fs::rename(temp, png).expect("publish hot snapshot");
            let mut tree = tree_json_core(core, core.clock);
            tree["hot"] = json!({"generation": generation, "pid": std::process::id(), "state": state_addr, "request": command["id"], "pixels_ns": pixels_ns, "scrolls": core.hot_scrolls(), "motion": core.tree.hot_motion(&core.scene)});
            atomic_write(&png.with_extension("json"), &serde_json::to_vec_pretty(&tree).unwrap());
            println!("rune-hot-pixels {}", tree["hot"]);
            use std::io::Write;
            std::io::stdout().flush().unwrap();
            first = false;
        }
        // The patch callback wakes immediately; control files are checked at a bounded cadence.
        woken = sleep.recv_timeout(Duration::from_millis(16)).is_ok();
    }
}

#[cfg(test)]
#[path = "hot_tests.rs"]
mod tests;
