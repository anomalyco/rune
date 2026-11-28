//! JNI bridge between `dev.rune.RuneView` (Android UI thread), a shared [`AndroidSurface`],
//! and Rune's in-activity [`PageStack`].
//!
//! `rune::android_main!` exports `JNI_OnLoad`, which stores `JavaVM*`, binds all `RuneView`
//! native methods via `RegisterNatives`, installs logcat forwarding, and records the app entry
//! function. When `RuneView` initializes on the UI thread, `nativeInit` invokes the app entry
//! function; `App::try_run` and `run_windows` hand their window specs into the thread-local
//! Android host slot and return immediately so Android's `Looper` and `Choreographer` drive every
//! frame.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};

use jni_sys::{
    JNI_ERR, JNI_FALSE, JNI_OK, JNI_TRUE, JNI_VERSION_1_6, JNIEnv, JavaVM, jboolean, jfloat, jint,
    jlong, jmethodID, jobject, jvalue,
};

use crate::app::App;
use crate::runtime::Waker;
use crate::windows::{AnyWindowSpec, Display, WindowSpec};
use crate::{Key, KeyEvent};

use super::bridge::{
    self, call_void_0, call_void_bool_bool_int_int, call_void_long, jni_clear_exception,
    jni_delete_global_ref, jni_method,
};
use super::document;
use super::editor::EditorConfig;
use super::input::{self, TextInputAction};
use super::machine::{self, Effect, Event, State, Surface};
use super::picker::{PickAnswer, PickRequest, PickStep};
use super::selection::{self, HandleKind, Presentation};
use super::selection_jni;
use super::stack::{BackOutcome, PageStack};
use super::surface::AndroidSurface;
use super::{haptics, logcat};
static APP_ENTRY: OnceLock<fn()> = OnceLock::new();

/// JNI global reference to the live `dev.rune.RuneView` instance and its cached Java callbacks.
pub(super) struct ViewBridge {
    pub(super) obj: jobject,
    schedule_frame: jmethodID,
    cancel_frame: jmethodID,
    schedule_wake_after: jmethodID,
    post_wake: jmethodID,
    perform_haptic: jmethodID,
    set_ime_state: jmethodID,
    schedule_a11y_wake_after: jmethodID,
    cancel_a11y_wake: jmethodID,
    api_level: i32,
    pub(super) start_document_pick: jmethodID,
    pub(super) open_document_fd: jmethodID,
    pub(super) update_selection_ui: jmethodID,
}

// SAFETY: JNI global references and `jmethodID`s are process-wide and safe to pass across threads
// when accessed through a thread-attached `JNIEnv*`.
unsafe impl Send for ViewBridge {}
// SAFETY: all mutations/destruction are synchronized by the owning `Mutex`.
unsafe impl Sync for ViewBridge {}

pub(super) struct Host {
    state: State,
    selection: selection::State,
    started_at: std::time::Instant,
    pub(super) stack: PageStack,
    surface: Option<AndroidSurface>,
    native_window: *mut ndk_sys::ANativeWindow,
    bridge: Arc<Mutex<Option<ViewBridge>>>,
    waker: Waker,
    #[cfg(feature = "accessibility")]
    pub(super) a11y: crate::a11y::android::Adapter,
}

impl Host {
    fn new(bridge: ViewBridge) -> Self {
        let bridge = Arc::new(Mutex::new(Some(bridge)));
        let wake_bridge = Arc::clone(&bridge);
        let waker: Waker = Arc::new(move || post_wake_from_any_thread(&wake_bridge));
        let stack = PageStack::new(waker.clone());
        #[cfg(feature = "accessibility")]
        let a11y = crate::a11y::android::Adapter::new(waker.clone());
        Self {
            state: State::default(),
            selection: selection::State::default(),
            started_at: std::time::Instant::now(),
            stack,
            surface: None,
            native_window: ptr::null_mut(),
            bridge,
            waker,
            #[cfg(feature = "accessibility")]
            a11y,
        }
    }

    fn now_secs(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    fn current_scale(&self) -> f32 {
        match self.state.surface {
            Surface::Attached { scale, .. } if scale.is_finite() && scale > 0.0 => scale,
            _ => 1.0,
        }
    }

    fn release_native_window(&mut self) {
        if !self.native_window.is_null() {
            let win = self.native_window;
            self.native_window = ptr::null_mut();
            // SAFETY: `win` was acquired from `ANativeWindow_fromSurface` and any `wgpu::Surface`
            // referencing it has already been dropped via `surface.detach_surface()`.
            unsafe { ndk_sys::ANativeWindow_release(win) };
        }
    }

    fn take_bridge_for_shutdown(&mut self, env: *mut JNIEnv) -> Option<ViewBridge> {
        if let Some(surface) = &mut self.surface {
            surface.detach_surface();
        }
        let _ = self.stack.take_haptics();
        self.stack.shutdown();
        self.surface = None;
        self.release_native_window();
        // SAFETY: `env` is the UI thread's valid `JNIEnv*`.
        unsafe { bridge::clear_link_target(env) };
        self.bridge.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn shutdown(&mut self, env: *mut JNIEnv) {
        if let Some(bridge) = self.take_bridge_for_shutdown(env) {
            // SAFETY: `env` is the UI thread's valid `JNIEnv*` and `bridge.obj` is a live global ref.
            unsafe { jni_delete_global_ref(env, bridge.obj) };
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(surface) = &mut self.surface { surface.detach_surface(); }
        self.stack.shutdown();
        self.surface = None;
        self.release_native_window();
    }
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    static PENDING_SPECS: RefCell<Option<Vec<AnyWindowSpec>>> = const { RefCell::new(None) };
}

#[cfg(feature = "accessibility")]
pub(super) fn with_host_mut<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|slot| slot.borrow_mut().as_mut().map(f))
}

fn install_initial_specs(specs: Vec<AnyWindowSpec>) {
    HOST.with(|slot| {
        if let Some(host) = slot.borrow_mut().as_mut() {
            host.stack.set_initial(specs);
            let _ = host.stack.pump();
        } else {
            PENDING_SPECS.with(|pending| *pending.borrow_mut() = Some(specs));
        }
    });
}

/// Installs an `App<S>` (and any secondary windows added via `App::window`) into the Android
/// UI-thread host slot and returns immediately. Called by `App::try_run` on `target_os = "android"`.
pub(crate) fn run<S: 'static>(app: App<S>) -> Result<(), String> {
    install_initial_specs(vec![WindowSpec::from_app("main", app).into()]);
    Ok(())
}

/// Installs windows built by a `run_windows` factory into the Android UI-thread host slot and
/// returns immediately.
pub(crate) fn run_windows(
    factory: Box<dyn FnOnce(&[Display]) -> Vec<AnyWindowSpec>>,
) -> Result<(), String> {
    let display = Display {
        id: None,
        name: Some("Android".into()),
        position: (0, 0),
        size: (1080, 2400),
        scale: 3.0,
        primary: true,
    };
    let specs = factory(&[display]);
    if specs.is_empty() {
        return Err("the app created no windows".into());
    }
    install_initial_specs(specs);
    Ok(())
}

/// Wakes the Android host when `rune::open_window` is called outside an active host turn.
/// Inside an active turn (`HOST` already borrowed), the turn's own `stack.pump()` drains `OPENED`.
pub(crate) fn notify_opened() {
    let waker = HOST.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|borrow| borrow.as_ref().map(|host| host.waker.clone()))
    });
    if let Some(waker) = waker {
        waker();
    }
}

/// Entry point invoked by `rune::android_main!` inside `JNI_OnLoad`.
///
/// # Safety
/// `vm` must be the valid `JavaVM*` supplied by ART to `JNI_OnLoad`.
pub unsafe fn jni_on_load(vm: *mut c_void, entry: fn()) -> i32 {
    if vm.is_null() { return JNI_ERR; }
    let vm = vm.cast::<JavaVM>();
    bridge::set_java_vm(vm);
    let _ = APP_ENTRY.set(entry);
    logcat::install();

    let mut env_ptr: *mut c_void = ptr::null_mut();
    // SAFETY: `vm` is non-null and valid per caller contract; `GetEnv` writes into `env_ptr`.
    let rc = unsafe {
        let Some(get_env) = (**vm).GetEnv else { return JNI_ERR };
        get_env(vm, &mut env_ptr, JNI_VERSION_1_6)
    };
    if rc != JNI_OK || env_ptr.is_null() { return JNI_ERR; }
    let env = env_ptr.cast::<JNIEnv>();

    let mut methods = vec![
        jni_method(c"nativeInit", c"()V", native_init as *const () as *mut c_void),
        jni_method(c"nativeSurfaceCreated", c"(Landroid/view/Surface;IIFZ)V", native_surface_created as *const () as *mut c_void),
        jni_method(c"nativeSurfaceChanged", c"(IIF)V", native_surface_changed as *const () as *mut c_void),
        jni_method(c"nativeSurfaceDestroyed", c"()V", native_surface_destroyed as *const () as *mut c_void),
        jni_method(c"nativeFrame", c"()V", native_frame as *const () as *mut c_void),
        jni_method(c"nativeInsets", c"(IIIIIF)V", native_insets as *const () as *mut c_void),
        jni_method(c"nativeDarkMode", c"(Z)V", native_dark_mode as *const () as *mut c_void),
        jni_method(c"nativeResume", c"()V", native_resume as *const () as *mut c_void),
        jni_method(c"nativePause", c"()V", native_pause as *const () as *mut c_void),
        jni_method(c"nativeBack", c"()Z", native_back as *const () as *mut c_void),
        jni_method(c"nativeTouch", c"(IIIFFJ)V", native_touch as *const () as *mut c_void),
        jni_method(c"nativeLongPressTimeout", c"(I)V", native_long_press_timeout as *const () as *mut c_void),
        jni_method(c"nativeWake", c"()V", native_wake as *const () as *mut c_void),
        jni_method(c"nativeDestroy", c"()V", native_destroy as *const () as *mut c_void),
        jni_method(c"nativeFilePickResult", c"(J[Ljava/lang/String;[Ljava/lang/String;)V", document::native_file_pick_result as *const () as *mut c_void),
        jni_method(c"nativeFilePickCancelled", c"(J)V", document::native_file_pick_cancelled as *const () as *mut c_void),
        jni_method(c"nativeFilePickError", c"(JLjava/lang/String;)V", document::native_file_pick_error as *const () as *mut c_void),
        jni_method(c"nativeCommitText", c"(Ljava/lang/String;)V", input::jni::native_commit_text as *const () as *mut c_void),
        jni_method(c"nativeSetComposingText", c"(Ljava/lang/String;I)V", input::jni::native_set_composing_text as *const () as *mut c_void),
        jni_method(c"nativeFinishComposingText", c"()V", input::jni::native_finish_composing_text as *const () as *mut c_void),
        jni_method(c"nativeDeleteSurroundingText", c"(II)V", input::jni::native_delete_surrounding_text as *const () as *mut c_void),
        jni_method(c"nativeEditorAction", c"(I)Z", input::jni::native_editor_action as *const () as *mut c_void),
        jni_method(c"nativeKeyDown", c"(IIIIZ)Z", input::jni::native_key_down as *const () as *mut c_void),
        jni_method(c"nativeKeyUp", c"(IIII)Z", input::jni::native_key_up as *const () as *mut c_void),
    ];
    methods.extend(super::a11y::native_methods());
    methods.extend(selection_jni::native_methods());

    // SAFETY: `env` is a valid JNIEnv for the thread running `JNI_OnLoad`.
    if !unsafe { bridge::register_natives(env, c"dev/rune/RuneView", &methods) } {
        return JNI_ERR;
    }
    // SAFETY: `env` is a valid JNIEnv; missing `RuneClipboard` class is handled non-fatally.
    unsafe { bridge::register_clipboard_natives(env) };
    JNI_VERSION_1_6
}

unsafe fn build_view_bridge(env: *mut JNIEnv, view: jobject) -> Option<ViewBridge> {
    // SAFETY: `env` and `view` are valid JNI handles passed to `nativeInit`.
    unsafe {
        let get_class = (**env).GetObjectClass?;
        let get_method = (**env).GetMethodID?;
        let new_global = (**env).NewGlobalRef?;
        let cls = get_class(env, view);
        if cls.is_null() {
            jni_clear_exception(env);
            return None;
        }
        let schedule_frame = get_method(env, cls, c"scheduleFrame".as_ptr(), c"()V".as_ptr());
        let cancel_frame = get_method(env, cls, c"cancelFrame".as_ptr(), c"()V".as_ptr());
        let schedule_wake_after = get_method(env, cls, c"scheduleWakeAfter".as_ptr(), c"(J)V".as_ptr());
        let post_wake = get_method(env, cls, c"postWake".as_ptr(), c"()V".as_ptr());
        let perform_haptic = get_method(env, cls, c"performHaptic".as_ptr(), c"(I)V".as_ptr());
        let set_ime_state = get_method(env, cls, c"setImeState".as_ptr(), c"(ZZII)V".as_ptr());
        let schedule_a11y_wake_after =
            get_method(env, cls, c"scheduleA11yWakeAfter".as_ptr(), c"(JJ)V".as_ptr());
        let cancel_a11y_wake = get_method(env, cls, c"cancelA11yWake".as_ptr(), c"()V".as_ptr());
        let start_document_pick = get_method(
            env, cls, c"startDocumentPick".as_ptr(),
            c"(JZZLjava/lang/String;Ljava/lang/String;[Ljava/lang/String;)V".as_ptr(),
        );
        let open_document_fd = get_method(
            env, cls, c"openDocumentFd".as_ptr(),
            c"(Ljava/lang/String;Ljava/lang/String;)I".as_ptr(),
        );
        if schedule_frame.is_null()
            || cancel_frame.is_null()
            || schedule_wake_after.is_null()
            || post_wake.is_null()
            || perform_haptic.is_null()
            || set_ime_state.is_null()
            || schedule_a11y_wake_after.is_null()
            || cancel_a11y_wake.is_null()
            || start_document_pick.is_null()
            || open_document_fd.is_null()
        {
            if let Some(del) = (**env).DeleteLocalRef { del(env, cls); }
            jni_clear_exception(env);
            return None;
        }
        let open_url = get_method(env, cls, c"openUrl".as_ptr(), c"(Ljava/lang/String;)Ljava/lang/String;".as_ptr());
        if open_url.is_null() { jni_clear_exception(env); }
        let get_clipboard = get_method(env, cls, c"getClipboard".as_ptr(), c"()Ldev/rune/RuneClipboard;".as_ptr());
        if get_clipboard.is_null() { jni_clear_exception(env); }
        let update_selection_ui = get_method(env, cls, c"updateSelectionUi".as_ptr(), selection_jni::UPDATE_SELECTION_UI_SIG.as_ptr());
        if update_selection_ui.is_null() { jni_clear_exception(env); }
        if let Some(del) = (**env).DeleteLocalRef { del(env, cls); }
        if !open_url.is_null() { bridge::set_link_target(env, view, open_url); }
        if !get_clipboard.is_null() { bridge::init_clipboard(env, view, get_clipboard); }
        let global = new_global(env, view);
        if global.is_null() {
            jni_clear_exception(env);
            return None;
        }
        let api_level = haptics::read_sdk_int(env);
        Some(ViewBridge {
            obj: global,
            schedule_frame,
            cancel_frame,
            schedule_wake_after,
            post_wake,
            perform_haptic,
            set_ime_state,
            schedule_a11y_wake_after,
            cancel_a11y_wake,
            api_level,
            start_document_pick,
            open_document_fd,
            update_selection_ui,
        })
    }
}

#[cfg(feature = "accessibility")]
pub(super) unsafe fn schedule_a11y_wake(
    env: *mut JNIEnv,
    host: &Host,
    attempt: crate::machine::Attempt,
    delay: std::time::Duration,
) {
    let ms = delay.as_millis().clamp(1, i64::MAX as u128) as jlong;
    let args = [jvalue { j: jlong::from(attempt.0) }, jvalue { j: ms }];
    if let Some(view) = host.bridge.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        // SAFETY: `env`, `view.obj`, and `schedule_a11y_wake_after` are valid `(JJ)V` JNI handles.
        unsafe {
            if let Some(call) = (**env).CallVoidMethodA {
                call(env, view.obj, view.schedule_a11y_wake_after, args.as_ptr());
                jni_clear_exception(env);
            }
        }
    }
}

unsafe fn cancel_a11y_wake(env: *mut JNIEnv, host: &Host) {
    if let Some(view) = host.bridge.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        // SAFETY: `env` is the UI thread's valid `JNIEnv*`.
        unsafe { call_void_0(env, view.obj, view.cancel_a11y_wake) };
    }
}

fn post_wake_from_any_thread(view_bridge: &Mutex<Option<ViewBridge>>) {
    let _ = bridge::with_jni_env(|env| {
        let guard = view_bridge.lock().unwrap_or_else(|e| e.into_inner());
        let Some(view) = guard.as_ref() else { return };
        // SAFETY: `env` is attached to this thread and `view` is held alive under `guard`.
        unsafe { call_void_0(env, view.obj, view.post_wake) };
    });
}

fn apply_view_schedule(env: *mut JNIEnv, bridge: &Mutex<Option<ViewBridge>>, effect: Effect) {
    let guard = bridge.lock().unwrap_or_else(|e| e.into_inner());
    let Some(view) = guard.as_ref() else { return };
    match effect {
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `view` is locked alive.
        Effect::ScheduleFrame => unsafe { call_void_0(env, view.obj, view.schedule_frame) },
        Effect::CancelFrame => unsafe { call_void_0(env, view.obj, view.cancel_frame) },
        Effect::ScheduleWakeAfter(secs) => {
            let ms = (secs * 1000.0).ceil().clamp(1.0, i64::MAX as f64) as jlong;
            // SAFETY: `env` is the UI thread's `JNIEnv*` and `view` is locked alive.
            unsafe { call_void_long(env, view.obj, view.schedule_wake_after, ms) };
        }
        _ => {}
    }
}

pub(super) fn deliver_file_pick_from_jni(env: *mut JNIEnv, id: u64, answer: PickAnswer) {
    HOST.with(|slot| {
        if let Some(host) = slot.borrow_mut().as_mut() {
            host.stack.deliver_file_pick(id, answer, &host.bridge);
        }
    });
    let _ = dispatch(env, ptr::null_mut(), Event::Wake, ptr::null_mut());
}

pub(super) fn deliver_text_input(env: *mut JNIEnv, action: TextInputAction) -> bool {
    let accepted = HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(host) = borrow.as_mut() else { return false };
        if host.state.destroyed { return false; }
        host.stack.text_input(action)
    });
    if accepted { let _ = dispatch(env, ptr::null_mut(), Event::Wake, ptr::null_mut()); }
    accepted
}

pub(super) fn deliver_key_up(env: *mut JNIEnv, event: KeyEvent) -> bool {
    let (consumed, needs_frame) = HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(host) = borrow.as_mut() else { return (false, false) };
        if host.state.destroyed { return (false, false) };
        if matches!(event.key, Key::Media(_)) && !host.stack.media_keys() { return (false, false); }
        (true, host.stack.key_up(event))
    });
    if needs_frame { let _ = dispatch(env, ptr::null_mut(), Event::Wake, ptr::null_mut()); }
    consumed
}

fn step_selection(host: &mut Host, event: selection::Event) -> (Option<Presentation>, bool) {
    let mut latest: Option<Presentation> = None;
    let mut needs_frame = false;
    let mut pending_events = vec![event];
    let mut depth = 0u8;
    while let Some(ev) = pending_events.pop() {
        depth += 1;
        if depth > 4 {
            break;
        }
        for eff in selection::step(&mut host.selection, ev) {
            match eff {
                selection::Effect::GrabHandle { handle, x, y } => {
                    if host.stack.handle_grab(handle.to_runtime(), x, y) {
                        needs_frame = true;
                    }
                }
                selection::Effect::MoveHandle { x, y } => {
                    let _ = host.stack.handle_drag(x, y);
                    needs_frame = true;
                }
                selection::Effect::ReleaseHandle => {
                    host.stack.handle_release();
                    needs_frame = true;
                }
                selection::Effect::ResolveTextMenu(picked) => {
                    host.stack.resolve_text_menu(picked);
                    needs_frame = true;
                }
                selection::Effect::RequestTextMenu => {
                    if host.stack.open_selection_text_menu() {
                        let snap = host.stack.selection_snapshot();
                        let menu = host.stack.take_text_menu(snap.is_some_and(|s| s.can_select_all));
                        let now = host.now_secs();
                        pending_events.push(selection::Event::Sync { now, snapshot: snap, menu });
                        needs_frame = true;
                    }
                }
                selection::Effect::Apply(p) => {
                    latest = Some(p);
                }
            }
        }
    }
    (latest, needs_frame)
}

fn dispatch_selection_jni(env: *mut JNIEnv, make_event: impl FnOnce(&Host) -> Option<selection::Event>) {
    let mut pending_ui: Option<(Arc<Mutex<Option<ViewBridge>>>, f32, Presentation)> = None;
    let mut needs_frame = false;
    HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };
        if host.state.destroyed { return; }
        let Some(ev) = make_event(host) else { return };
        let (apply, frame) = step_selection(host, ev);
        needs_frame = frame;
        if let Some(p) = apply {
            pending_ui = Some((Arc::clone(&host.bridge), host.current_scale(), p));
        }
    });
    if let Some((bridge, scale, p)) = pending_ui {
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `HOST` is not borrowed.
        unsafe { selection_jni::apply_presentation_bridge(env, &bridge, scale, p) };
    }
    if needs_frame {
        let _ = dispatch(env, ptr::null_mut(), Event::Wake, ptr::null_mut());
    }
}

pub(super) fn handle_selection_grab(env: *mut JNIEnv, handle: HandleKind, x_px: f32, y_px: f32) {
    dispatch_selection_jni(env, |host| {
        let s = host.current_scale();
        Some(selection::Event::HandleGrab { handle, x: x_px / s, y: y_px / s })
    });
}

pub(super) fn handle_selection_move(env: *mut JNIEnv, x_px: f32, y_px: f32) {
    dispatch_selection_jni(env, |host| {
        let s = host.current_scale();
        Some(selection::Event::HandleMove { x: x_px / s, y: y_px / s })
    });
}

pub(super) fn handle_selection_release(env: *mut JNIEnv) {
    dispatch_selection_jni(env, |host| Some(selection::Event::HandleRelease { now: host.now_secs() }));
}

pub(super) fn handle_selection_caret_tap(env: *mut JNIEnv) {
    dispatch_selection_jni(env, |host| Some(selection::Event::CaretHandleTapped { now: host.now_secs() }));
}

pub(super) fn handle_selection_toolbar_action(env: *mut JNIEnv, code: i32) {
    dispatch_selection_jni(env, |host| {
        Some(selection::Event::ToolbarAction { code, now: host.now_secs() })
    });
}

fn dispatch(
    env: *mut JNIEnv,
    view_obj: jobject,
    event: Event,
    pending_window: *mut ndk_sys::ANativeWindow,
) -> bool {
    let mut consumed_back = false;
    let mut pending_window = pending_window;
    let mut pending_ime: Option<(jobject, jmethodID, Option<EditorConfig>, bool)> = None;
    let mut pending_picks: Vec<(Arc<Mutex<Option<ViewBridge>>>, PickRequest)> = Vec::new();
    let mut shutdown_bridge: Option<ViewBridge> = None;
    let mut pending_ui: Option<(Arc<Mutex<Option<ViewBridge>>>, f32, Presentation)> = None;
    #[cfg(feature = "accessibility")]
    let mut queued_a11y_events: Option<accesskit_android::QueuedEvents> = None;
    let is_suspend = matches!(event, Event::Paused | Event::SurfaceDestroyed | Event::Destroy);
    let is_resume = matches!(event, Event::Resumed | Event::SurfaceCreated { .. });

    HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };
        if is_suspend {
            let (apply, _) = step_selection(host, selection::Event::Suspend);
            if let Some(p) = apply {
                pending_ui = Some((Arc::clone(&host.bridge), host.current_scale(), p));
            }
        }
        let effects = machine::step(&mut host.state, event);
        let mut shutdown = false;

        for effect in effects {
            match effect {
                Effect::AttachSurface { viewport, scale } => {
                    if !pending_window.is_null() {
                        if let Some(surface) = &mut host.surface { surface.detach_surface(); }
                        host.release_native_window();
                        host.native_window = pending_window;
                        pending_window = ptr::null_mut();
                    }
                    if !host.native_window.is_null() {
                        let srgb_blend = host.stack.top_srgb_blend();
                        let res = if let Some(surface) = &mut host.surface {
                            // SAFETY: `host.native_window` is live until `detach_surface` / drop.
                            unsafe {
                                surface.attach_native_window(
                                    host.native_window.cast::<c_void>(),
                                    viewport,
                                    scale,
                                    srgb_blend,
                                )
                            }
                        } else {
                            // SAFETY: `host.native_window` is live until `detach_surface` / drop.
                            unsafe {
                                AndroidSurface::from_native_window(
                                    host.native_window.cast::<c_void>(),
                                    viewport,
                                    scale,
                                    srgb_blend,
                                )
                            }
                            .map(|(surface, size, applied_scale)| {
                                host.surface = Some(surface);
                                (size, applied_scale)
                            })
                        };
                        match res {
                            Ok((size, applied_scale)) => {
                                host.stack.set_viewport(size, applied_scale);
                                let _ = host.stack.pump();
                            }
                            Err(err) => eprintln!("rune: android surface attach failed: {err}"),
                        }
                    }
                }
                Effect::ResizeSurface { viewport, scale } => {
                    if let Some(surface) = &mut host.surface {
                        if let Some((size, applied_scale)) = surface.resize(viewport, scale) {
                            host.stack.set_viewport(size, applied_scale);
                        }
                    } else {
                        host.stack.set_viewport(viewport, scale);
                    }
                }
                Effect::DetachSurface => {
                    #[cfg(feature = "accessibility")]
                    host.a11y.reset_throttle();
                    unsafe { cancel_a11y_wake(env, host) };
                    let _ = host.stack.take_haptics();
                    if let Some(surface) = &mut host.surface { surface.detach_surface(); }
                    host.release_native_window();
                }
                Effect::SetSafeArea(insets) => host.stack.set_safe_area(insets),
                Effect::SetKeyboardInset(points) => host.stack.set_keyboard_inset(points),
                Effect::SetDark(dark) => host.stack.set_dark(dark),
                Effect::SetTouchLongPress(secs) => host.stack.set_touch_long_press(secs),
                Effect::Touch { id, phase, x, y, time_secs } => {
                    host.stack.touch(id, phase, x, y, time_secs);
                }
                Effect::DispatchBack => {
                    let consumed = host.stack.handle_back() == BackOutcome::Consumed;
                    consumed_back = consumed;
                    let follow = machine::step(&mut host.state, Event::BackDone { consumed });
                    for next in follow {
                        apply_view_schedule(env, &host.bridge, next);
                    }
                }
                Effect::FinishComposition => {
                    host.stack.finish_composition();
                }
                Effect::SetIme { editor, restart } => {
                    if let Some(v) = host.bridge.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        pending_ime = Some((v.obj, v.set_ime_state, editor, restart));
                    }
                }
                Effect::CancelFrame => {
                    #[cfg(feature = "accessibility")]
                    host.a11y.reset_throttle();
                    unsafe { cancel_a11y_wake(env, host) };
                    apply_view_schedule(env, &host.bridge, effect);
                }
                Effect::ScheduleFrame | Effect::ScheduleWakeAfter(_) => {
                    apply_view_schedule(env, &host.bridge, effect);
                }
                Effect::DrawFrame => {
                    #[cfg(feature = "accessibility")]
                    let _ = host.stack.a11y_receive(&mut host.a11y);
                    let frame_result = host.surface.as_mut().and_then(|surface| {
                        host.stack.draw_top(surface)
                    });
                    if let Some((outcome, pending_haptics, pick_step, editor)) = frame_result {
                        if !pending_haptics.is_empty()
                            && let Some(v) = host.bridge.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                        {
                            // SAFETY: `env` is the UI thread's `JNIEnv*` and `v` is locked.
                            unsafe { haptics::flush_jni(env, v.obj, v.perform_haptic, v.api_level, outcome.drew, pending_haptics) };
                        }
                        let mut needs_redraw = outcome.animating;
                        match pick_step {
                            PickStep::None => {}
                            PickStep::Refused => needs_redraw = true,
                            PickStep::Start(req) => pending_picks.push((Arc::clone(&host.bridge), req)),
                        }
                        let now = host.now_secs();
                        let snap = host.stack.selection_snapshot();
                        let menu = host.stack.take_text_menu(snap.is_some_and(|s| s.can_select_all));
                        let (apply, sel_frame) = step_selection(host, selection::Event::Sync { now, snapshot: snap, menu });
                        if let Some(p) = apply {
                            pending_ui = Some((Arc::clone(&host.bridge), host.current_scale(), p));
                        }
                        needs_redraw |= sel_frame;
                        let wake = selection_jni::merge_wake(outcome.wake_after_secs, host.selection.wake_after_secs(now));
                        let follow = machine::step(
                            &mut host.state,
                            Event::FrameDone {
                                animating: needs_redraw,
                                wake_after_secs: wake,
                                editor: editor.config,
                                ime_restart: editor.reset,
                                field_tap: editor.field_tap,
                            },
                        );
                        for next in follow {
                            if let Effect::SetIme { editor, restart } = next {
                                if let Some(v) = host.bridge.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                                    pending_ime = Some((v.obj, v.set_ime_state, editor, restart));
                                }
                            } else {
                                apply_view_schedule(env, &host.bridge, next);
                            }
                        }
                        #[cfg(feature = "accessibility")]
                        if let Some(sync) = host.stack.a11y_sync(&mut host.a11y, crate::time::now()) {
                            if let Some((attempt, delay)) = sync.wake_after {
                                // SAFETY: `env` is the UI thread's valid `JNIEnv*`.
                                unsafe { schedule_a11y_wake(env, host, attempt, delay) };
                            }
                            queued_a11y_events = sync.events;
                        }
                    }
                }
                Effect::Shutdown => {
                    #[cfg(feature = "accessibility")]
                    host.a11y.reset_throttle();
                    unsafe { cancel_a11y_wake(env, host) };
                    shutdown = true;
                }
            }
        }
        if !shutdown && is_resume && host.state.resumed && matches!(host.state.surface, Surface::Attached { .. }) {
            let (apply, _) = step_selection(host, selection::Event::Resume);
            if let Some(p) = apply {
                pending_ui = Some((Arc::clone(&host.bridge), host.current_scale(), p));
            }
        }
        if shutdown {
            shutdown_bridge = host.take_bridge_for_shutdown(env);
            *borrow = None;
        }
    });

    #[cfg(feature = "accessibility")]
    if let Some(events) = queued_a11y_events {
        // SAFETY: `HOST` borrow has ended; raise queued events on the UI thread.
        unsafe { super::a11y::raise_events(env, view_obj, events) };
    }
    #[cfg(not(feature = "accessibility"))]
    let _ = view_obj;

    if let Some((bridge, scale, p)) = pending_ui {
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `HOST` is not borrowed.
        unsafe { selection_jni::apply_presentation_bridge(env, &bridge, scale, p) };
    }

    // Run `setImeState` and SAF pickers after releasing the `HOST` RefCell borrow so
    // synchronous Java callbacks cannot re-enter a borrowed `HOST`.
    if let Some((obj, method, editor, restart)) = pending_ime {
        let (enabled, input_type, ime_options) = match editor {
            Some(cfg) => (true, cfg.input_type, cfg.ime_options),
            None => (false, 0, 0),
        };
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `obj` stays live until
        // `shutdown_bridge` is deleted right below.
        unsafe { call_void_bool_bool_int_int(env, obj, method, enabled, restart, input_type, ime_options) };
    }
    if let Some(bridge) = shutdown_bridge {
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `bridge.obj` is a live global ref.
        unsafe { jni_delete_global_ref(env, bridge.obj) };
    }
    if !pending_window.is_null() {
        // SAFETY: `pending_window` was not adopted by `Host`, so release our reference.
        unsafe { ndk_sys::ANativeWindow_release(pending_window) };
    }
    for (bridge, req) in pending_picks {
        let id = req.id;
        if let Err(err) = document::start_pick(env, &bridge, &req) {
            deliver_file_pick_from_jni(env, id, PickAnswer::Error(err));
        }
    }
    consumed_back
}

unsafe extern "system" fn native_init(env: *mut JNIEnv, this: jobject) {
    // SAFETY: `env` and `this` are valid JNI handles for `RuneView`.
    let Some(bridge) = (unsafe { build_view_bridge(env, this) }) else {
        eprintln!("rune: failed to bind dev.rune.RuneView JNI callbacks");
        return;
    };
    HOST.with(|slot| {
        if let Some(mut old) = slot.borrow_mut().take() { old.shutdown(env); }
        let mut host = Host::new(bridge);
        PENDING_SPECS.with(|pending| {
            if let Some(specs) = pending.borrow_mut().take() {
                host.stack.set_initial(specs);
            }
        });
        *slot.borrow_mut() = Some(host);
    });
    if let Some(entry) = APP_ENTRY.get() { entry(); }
    HOST.with(|slot| {
        if let Some(host) = slot.borrow_mut().as_mut() {
            let _ = host.stack.pump();
        }
    });
}

unsafe extern "system" fn native_surface_created(
    env: *mut JNIEnv, this: jobject, surface: jobject, width: jint, height: jint, density: jfloat, dark: jboolean,
) {
    if surface.is_null() { return; }
    // SAFETY: `env` and `surface` are valid JNI handles for `android.view.Surface`.
    let win = unsafe { ndk_sys::ANativeWindow_fromSurface(env, surface) };
    if win.is_null() {
        eprintln!("rune: ANativeWindow_fromSurface returned null");
        return;
    }
    let _ = dispatch(
        env,
        this,
        Event::SurfaceCreated {
            width_px: width.max(0) as u32,
            height_px: height.max(0) as u32,
            scale: density,
            dark: dark != JNI_FALSE,
        },
        win,
    );
}

unsafe extern "system" fn native_surface_changed(
    env: *mut JNIEnv, this: jobject, width: jint, height: jint, density: jfloat,
) {
    let _ = dispatch(
        env,
        this,
        Event::SurfaceChanged { width_px: width.max(0) as u32, height_px: height.max(0) as u32, scale: density },
        ptr::null_mut(),
    );
}

unsafe extern "system" fn native_surface_destroyed(env: *mut JNIEnv, this: jobject) {
    let _ = dispatch(env, this, Event::SurfaceDestroyed, ptr::null_mut());
}

unsafe extern "system" fn native_frame(env: *mut JNIEnv, this: jobject) {
    let _ = dispatch(env, this, Event::Frame, ptr::null_mut());
}

unsafe extern "system" fn native_insets(
    env: *mut JNIEnv, this: jobject, top: jint, right: jint, bottom: jint, left: jint, ime_bottom: jint, density: jfloat,
) {
    let _ = dispatch(
        env,
        this,
        Event::InsetsChanged { safe_area_px: [top, right, bottom, left], keyboard_px: ime_bottom, scale: density },
        ptr::null_mut(),
    );
}

unsafe extern "system" fn native_dark_mode(env: *mut JNIEnv, this: jobject, dark: jboolean) {
    let _ = dispatch(env, this, Event::DarkModeChanged(dark != JNI_FALSE), ptr::null_mut());
}

unsafe extern "system" fn native_resume(env: *mut JNIEnv, this: jobject) {
    let _ = dispatch(env, this, Event::Resumed, ptr::null_mut());
}

unsafe extern "system" fn native_pause(env: *mut JNIEnv, this: jobject) {
    HOST.with(|slot| {
        if let Some(host) = slot.borrow_mut().as_mut() {
            let _ = host.stack.take_haptics();
        }
    });
    let _ = dispatch(env, this, Event::Paused, ptr::null_mut());
}

unsafe extern "system" fn native_back(env: *mut JNIEnv, this: jobject) -> jboolean {
    if dispatch(env, this, Event::BackPressed, ptr::null_mut()) { JNI_TRUE } else { JNI_FALSE }
}

unsafe extern "system" fn native_touch(
    env: *mut JNIEnv, this: jobject, action: jint, flags: jint, pointer_id: jint, x_px: jfloat, y_px: jfloat, time_ms: jlong,
) {
    let _ = dispatch(env, this, Event::Touch { action, flags, pointer_id, x_px, y_px, time_ms }, ptr::null_mut());
}

unsafe extern "system" fn native_long_press_timeout(env: *mut JNIEnv, this: jobject, timeout_ms: jint) {
    let _ = dispatch(env, this, Event::LongPressTimeoutChanged(timeout_ms), ptr::null_mut());
}

unsafe extern "system" fn native_wake(env: *mut JNIEnv, this: jobject) {
    let mut pending_ui: Option<(Arc<Mutex<Option<ViewBridge>>>, f32, Presentation)> = None;
    HOST.with(|slot| {
        if let Some(host) = slot.borrow_mut().as_mut() {
            let _ = host.stack.pump();
            #[cfg(feature = "accessibility")]
            let _ = host.stack.a11y_receive(&mut host.a11y);
            if !host.state.resumed { let _ = host.stack.take_haptics(); }
            let now = host.now_secs();
            let (apply, _) = step_selection(host, selection::Event::Tick { now });
            if let Some(p) = apply {
                pending_ui = Some((Arc::clone(&host.bridge), host.current_scale(), p));
            }
        }
    });
    if let Some((bridge, scale, p)) = pending_ui {
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `HOST` is not borrowed.
        unsafe { selection_jni::apply_presentation_bridge(env, &bridge, scale, p) };
    }
    let _ = dispatch(env, this, Event::Wake, ptr::null_mut());
}

unsafe extern "system" fn native_destroy(env: *mut JNIEnv, this: jobject) {
    let _ = dispatch(env, this, Event::Destroy, ptr::null_mut());
}
