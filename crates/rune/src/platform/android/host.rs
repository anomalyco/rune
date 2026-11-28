//! JNI bridge between `dev.rune.RuneView` (Android UI thread) and Rune's [`LayerHost`].
//!
//! `rune::android_main!` exports `JNI_OnLoad`, which stores `JavaVM*`, binds all `RuneView`
//! native methods via `RegisterNatives`, installs logcat forwarding, and records the app entry
//! function. When `RuneView` initializes on the UI thread, `nativeInit` invokes the app entry
//! function; `App::try_run` hands its type-erased app into the thread-local Android host slot and
//! returns immediately so Android's `Looper` and `Choreographer` drive every frame.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};

use jni_sys::{
    JNI_ERR, JNI_FALSE, JNI_OK, JNI_TRUE, JNI_VERSION_1_6, JNIEnv, JavaVM, jboolean, jfloat, jint,
    jlong, jmethodID, jobject,
};

use crate::app::{App, RuntimeConfig, Shared};
use crate::compose::Compositor;
use crate::input::host_events::{ImeStep, ImeSync, ime_sync_step};
use crate::platform::embed::{EmbedFrame, LayerHost};
use crate::runtime::Waker;
use crate::tooling::AppSpec;
use crate::{Haptic, Key, KeyEvent, TouchPhase};

use super::bridge::{
    self, call_void_0, call_void_bool_bool, call_void_long, jni_clear_exception,
    jni_delete_global_ref, jni_method,
};
use super::document::{self, Document};
use super::input::{self, TextInputAction};
use super::machine::{self, Effect, Event, State};
use super::picker::{self, PickAnswer, PickRequest, PickStep};
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
    set_ime_enabled: jmethodID,
    api_level: i32,
    pub(super) start_document_pick: jmethodID,
    pub(super) open_document_fd: jmethodID,
}

// SAFETY: JNI global references and `jmethodID`s are process-wide and safe to pass across threads
// when accessed through a thread-attached `JNIEnv*`.
unsafe impl Send for ViewBridge {}
// SAFETY: all mutations/destruction are synchronized by the owning `Mutex`.
unsafe impl Sync for ViewBridge {}

/// Type-erased boundary between the Android UI-thread host and a concrete `App<S>` / `LayerHost<S>`.
trait AndroidRunner {
    fn attach_or_create(
        &mut self,
        native_window: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
    ) -> Result<(), String>;
    fn resize(&mut self, viewport: (f32, f32), scale: f32);
    fn detach(&mut self);
    fn set_safe_area(&mut self, insets: [f32; 4]);
    fn set_keyboard_inset(&mut self, points: f32);
    fn set_dark(&mut self, dark: bool);
    fn set_touch_long_press(&mut self, secs: Option<f64>);
    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time_secs: f64);
    fn media_keys(&self) -> bool;
    fn dispatch_back(&mut self) -> bool;
    fn text_input(&mut self, action: TextInputAction) -> bool;
    fn key_up(&mut self, ev: KeyEvent) -> bool;
    fn finish_composition(&mut self);
    fn ime_sync_step(&mut self, focused: bool) -> ImeStep;
    fn clear_ime_sync(&mut self);
    fn drain_shared(&mut self);
    fn frame(&mut self) -> Option<EmbedFrame>;
    fn take_haptics(&mut self) -> Vec<Haptic>;
    fn take_file_pick(&mut self) -> PickStep;
    fn deliver_file_pick(&mut self, id: u64, answer: PickAnswer, bridge: &Arc<Mutex<Option<ViewBridge>>>);
    fn cancel_file_dialogs(&mut self);
}

struct PendingAndroidApp<S: 'static> {
    spec: AppSpec<S>,
    state: S,
    runtime_config: RuntimeConfig<S>,
    compositor: Option<Box<dyn Compositor<S>>>,
}

struct TypedRunner<S: 'static> {
    pending: Option<PendingAndroidApp<S>>,
    host: Option<LayerHost<S>>,
    shared: Arc<Shared<S>>,
    clipboard: crate::clipboard::Client,
    ime: ImeSync,
}

impl<S: 'static> AndroidRunner for TypedRunner<S> {
    fn attach_or_create(
        &mut self,
        native_window: *mut c_void,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
    ) -> Result<(), String> {
        if let Some(host) = &mut self.host {
            // SAFETY: caller guarantees `native_window` is valid until `detach` or drop.
            return unsafe { host.attach_native_window(native_window, viewport, scale) };
        }
        let Some(pending) = self.pending.take() else {
            return Err("Android runner has neither pending app nor active LayerHost".into());
        };
        self.shared.connect(wake.clone());
        self.clipboard.connect(wake.clone());
        // SAFETY: caller guarantees `native_window` is valid until `detach` or drop.
        let mut host = unsafe {
            LayerHost::from_native_window_configured(
                native_window,
                &pending.spec,
                pending.state,
                viewport,
                scale,
                dark,
                wake,
                Some(pending.runtime_config),
            )
        }?;
        host.runtime_mut().core_mut().host.file_dialogs = true;
        if let Some(compositor) = pending.compositor {
            host = host.with_boxed_compositor(compositor);
        }
        self.host = Some(host);
        Ok(())
    }

    fn resize(&mut self, viewport: (f32, f32), scale: f32) {
        if let Some(h) = &mut self.host { h.resize(viewport, scale); }
    }
    fn detach(&mut self) {
        if let Some(h) = &mut self.host { h.detach_surface(); }
    }
    fn set_safe_area(&mut self, insets: [f32; 4]) {
        if let Some(h) = &mut self.host { h.runtime_mut().set_safe_area(insets); }
    }
    fn set_keyboard_inset(&mut self, points: f32) {
        if let Some(h) = &mut self.host { h.runtime_mut().set_keyboard_inset(points); }
    }
    fn set_dark(&mut self, dark: bool) {
        if let Some(h) = &mut self.host { h.runtime_mut().set_system_dark(dark); }
    }
    fn set_touch_long_press(&mut self, secs: Option<f64>) {
        if let Some(h) = &mut self.host { h.runtime_mut().set_touch_long_press(secs.unwrap_or(0.0)); }
    }
    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time_secs: f64) {
        if let Some(h) = &mut self.host { h.runtime_mut().touch(id, phase, x, y, time_secs); }
    }
    fn media_keys(&self) -> bool {
        self.host.as_ref().is_some_and(|h| h.runtime().media_keys())
    }
    fn dispatch_back(&mut self) -> bool {
        let Some(host) = self.host.as_mut() else { return false };
        let _ = self.shared.drain_window(host.runtime_mut(), |_| {});
        host.dispatch_back_escape()
    }
    fn text_input(&mut self, action: TextInputAction) -> bool {
        let Some(host) = &mut self.host else { return false };
        input::apply_text_input(host.runtime_mut().core_mut(), action)
    }
    fn key_up(&mut self, ev: KeyEvent) -> bool {
        let Some(host) = &mut self.host else { return false };
        host.runtime_mut().key_up(ev)
    }
    fn finish_composition(&mut self) {
        if let Some(host) = &mut self.host { host.runtime_mut().finish_composition(); }
    }
    fn ime_sync_step(&mut self, focused: bool) -> ImeStep {
        let Some(host) = &mut self.host else {
            return ImeStep { cancel: false, set_allowed: None, cursor_area: None };
        };
        ime_sync_step(host.runtime_mut().core_mut(), &mut self.ime, focused)
    }
    fn clear_ime_sync(&mut self) { self.ime.clear(); }
    fn drain_shared(&mut self) {
        if let Some(host) = &mut self.host {
            let _ = self.clipboard.receive_core(host.runtime_mut().core_mut());
            let _ = self.shared.drain_window(host.runtime_mut(), |_| {});
            self.clipboard.flush_core(host.runtime_mut().core_mut());
        }
    }
    fn frame(&mut self) -> Option<EmbedFrame> {
        let host = self.host.as_mut()?;
        let _ = self.clipboard.receive_core(host.runtime_mut().core_mut());
        let _ = self.shared.drain_window(host.runtime_mut(), |_| {});
        let outcome = host.frame();
        self.clipboard.flush_core(host.runtime_mut().core_mut());
        Some(outcome)
    }
    fn take_haptics(&mut self) -> Vec<Haptic> {
        self.host.as_mut().map(LayerHost::take_haptics).unwrap_or_default()
    }
    fn take_file_pick(&mut self) -> PickStep {
        self.host.as_mut().map_or(PickStep::None, |h| picker::take_pick(h.runtime_mut().core_mut()))
    }
    fn deliver_file_pick(&mut self, id: u64, answer: PickAnswer, bridge: &Arc<Mutex<Option<ViewBridge>>>) {
        if let Some(host) = &mut self.host {
            let bridge = Arc::clone(bridge);
            picker::deliver_pick(host.runtime_mut().core_mut(), id, answer, move |uri| {
                Document::new(Arc::clone(&bridge), uri)
            });
        }
    }
    fn cancel_file_dialogs(&mut self) {
        if let Some(host) = &mut self.host { host.runtime_mut().cancel_file_dialogs(); }
    }
}

struct Host {
    state: State,
    runner: Option<Box<dyn AndroidRunner>>,
    native_window: *mut ndk_sys::ANativeWindow,
    bridge: Arc<Mutex<Option<ViewBridge>>>,
    waker: Waker,
}

impl Host {
    fn new(bridge: ViewBridge) -> Self {
        let bridge = Arc::new(Mutex::new(Some(bridge)));
        let wake_bridge = Arc::clone(&bridge);
        let waker: Waker = Arc::new(move || post_wake_from_any_thread(&wake_bridge));
        Self { state: State::default(), runner: None, native_window: ptr::null_mut(), bridge, waker }
    }

    fn release_native_window(&mut self) {
        if !self.native_window.is_null() {
            let win = self.native_window;
            self.native_window = ptr::null_mut();
            // SAFETY: `win` was acquired from `ANativeWindow_fromSurface` and any `wgpu::Surface`
            // referencing it has already been dropped via `runner.detach()`.
            unsafe { ndk_sys::ANativeWindow_release(win) };
        }
    }

    fn take_bridge_for_shutdown(&mut self, env: *mut JNIEnv) -> Option<ViewBridge> {
        if let Some(runner) = &mut self.runner {
            runner.cancel_file_dialogs();
            runner.detach();
        }
        self.runner = None;
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
        if let Some(runner) = &mut self.runner { runner.detach(); }
        self.runner = None;
        self.release_native_window();
    }
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    static PENDING_RUNNER: RefCell<Option<Box<dyn AndroidRunner>>> = const { RefCell::new(None) };
}

/// Installs an `App<S>` into the Android UI-thread host slot and returns immediately.
/// Called by `App::try_run` on `target_os = "android"`.
pub(crate) fn run<S: 'static>(mut app: App<S>) -> Result<(), String> {
    let compositor = app.compositor.take();
    let (spec, state, shared, runtime_config) = app.into_parts(None);
    let runner: Box<dyn AndroidRunner> = Box::new(TypedRunner {
        pending: Some(PendingAndroidApp { spec, state, runtime_config, compositor }),
        host: None,
        shared,
        clipboard: crate::clipboard::Client::default(),
        ime: ImeSync::default(),
    });
    HOST.with(|slot| {
        if let Some(host) = slot.borrow_mut().as_mut() {
            host.runner = Some(runner);
        } else {
            PENDING_RUNNER.with(|pending| *pending.borrow_mut() = Some(runner));
        }
    });
    Ok(())
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

    let methods = [
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
        let set_ime_enabled = get_method(env, cls, c"setImeEnabled".as_ptr(), c"(ZZ)V".as_ptr());
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
            || set_ime_enabled.is_null()
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
            set_ime_enabled,
            api_level,
            start_document_pick,
            open_document_fd,
        })
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
        if let Some(host) = slot.borrow_mut().as_mut()
            && let Some(runner) = &mut host.runner
        {
            runner.deliver_file_pick(id, answer, &host.bridge);
        }
    });
    let _ = dispatch(env, Event::Wake, ptr::null_mut());
}

pub(super) fn deliver_text_input(env: *mut JNIEnv, action: TextInputAction) -> bool {
    let accepted = HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(host) = borrow.as_mut() else { return false };
        if host.state.destroyed { return false; }
        host.runner.as_mut().is_some_and(|r| r.text_input(action))
    });
    if accepted { let _ = dispatch(env, Event::Wake, ptr::null_mut()); }
    accepted
}

pub(super) fn deliver_key_up(env: *mut JNIEnv, event: KeyEvent) -> bool {
    let (consumed, needs_frame) = HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(host) = borrow.as_mut() else { return (false, false) };
        if host.state.destroyed { return (false, false); }
        let Some(runner) = &mut host.runner else { return (false, false) };
        if matches!(event.key, Key::Media(_)) && !runner.media_keys() { return (false, false); }
        (true, runner.key_up(event))
    });
    if needs_frame { let _ = dispatch(env, Event::Wake, ptr::null_mut()); }
    consumed
}

fn dispatch(env: *mut JNIEnv, event: Event, pending_window: *mut ndk_sys::ANativeWindow) -> bool {
    let mut consumed_back = false;
    let mut pending_window = pending_window;
    let mut pending_ime: Option<(jobject, jmethodID, bool, bool)> = None;
    let mut pending_picks: Vec<(Arc<Mutex<Option<ViewBridge>>>, PickRequest)> = Vec::new();
    let mut shutdown_bridge: Option<ViewBridge> = None;

    HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };
        let effects = machine::step(&mut host.state, event);
        let mut shutdown = false;

        for effect in effects {
            match effect {
                Effect::AttachSurface { viewport, scale } => {
                    if !pending_window.is_null() {
                        if let Some(runner) = &mut host.runner { runner.detach(); }
                        host.release_native_window();
                        host.native_window = pending_window;
                        pending_window = ptr::null_mut();
                    }
                    if !host.native_window.is_null()
                        && let Some(runner) = &mut host.runner
                        && let Err(err) = runner.attach_or_create(
                            host.native_window.cast::<c_void>(),
                            viewport,
                            scale,
                            host.state.dark,
                            host.waker.clone(),
                        )
                    {
                        eprintln!("rune: android surface attach failed: {err}");
                    }
                }
                Effect::ResizeSurface { viewport, scale } => {
                    if let Some(r) = &mut host.runner { r.resize(viewport, scale); }
                }
                Effect::DetachSurface => {
                    if let Some(r) = &mut host.runner {
                        let _ = r.take_haptics();
                        r.detach();
                    }
                    host.release_native_window();
                }
                Effect::SetSafeArea(insets) => {
                    if let Some(r) = &mut host.runner { r.set_safe_area(insets); }
                }
                Effect::SetKeyboardInset(points) => {
                    if let Some(r) = &mut host.runner { r.set_keyboard_inset(points); }
                }
                Effect::SetDark(dark) => {
                    if let Some(r) = &mut host.runner { r.set_dark(dark); }
                }
                Effect::SetTouchLongPress(secs) => {
                    if let Some(r) = &mut host.runner { r.set_touch_long_press(secs); }
                }
                Effect::Touch { id, phase, x, y, time_secs } => {
                    if let Some(r) = &mut host.runner { r.touch(id, phase, x, y, time_secs); }
                }
                Effect::DispatchBack => {
                    let consumed = host.runner.as_mut().is_some_and(|r| r.dispatch_back());
                    consumed_back = consumed;
                    let follow = machine::step(&mut host.state, Event::BackDone { consumed });
                    for next in follow {
                        apply_view_schedule(env, &host.bridge, next);
                    }
                }
                Effect::FinishComposition => {
                    if let Some(r) = &mut host.runner {
                        r.finish_composition();
                        r.clear_ime_sync();
                    }
                }
                Effect::SetImeEnabled { enabled, restart } => {
                    if !enabled && let Some(r) = &mut host.runner { r.clear_ime_sync(); }
                    if let Some(v) = host.bridge.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        pending_ime = Some((v.obj, v.set_ime_enabled, enabled, restart));
                    }
                }
                Effect::ScheduleFrame | Effect::CancelFrame | Effect::ScheduleWakeAfter(_) => {
                    apply_view_schedule(env, &host.bridge, effect);
                }
                Effect::DrawFrame => {
                    let frame_result = host.runner.as_mut().and_then(|r| {
                        let outcome = r.frame()?;
                        Some((outcome, r.take_haptics(), r.take_file_pick(), r.ime_sync_step(host.state.resumed)))
                    });
                    if let Some((outcome, pending_haptics, pick_step, ime)) = frame_result {
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
                        let follow = machine::step(
                            &mut host.state,
                            Event::FrameDone {
                                animating: needs_redraw,
                                wake_after_secs: outcome.wake_after_secs,
                                ime_allowed: ime.set_allowed,
                                ime_restart: ime.cancel,
                            },
                        );
                        for next in follow {
                            if let Effect::SetImeEnabled { enabled, restart } = next {
                                if let Some(v) = host.bridge.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                                    pending_ime = Some((v.obj, v.set_ime_enabled, enabled, restart));
                                }
                            } else {
                                apply_view_schedule(env, &host.bridge, next);
                            }
                        }
                    }
                }
                Effect::Shutdown => shutdown = true,
            }
        }
        if shutdown {
            shutdown_bridge = host.take_bridge_for_shutdown(env);
            *borrow = None;
        }
    });

    // Run `setImeEnabled` and SAF pickers after releasing the `HOST` RefCell borrow so
    // synchronous Java callbacks cannot re-enter a borrowed `HOST`.
    if let Some((obj, method, enabled, restart)) = pending_ime {
        // SAFETY: `env` is the UI thread's `JNIEnv*` and `obj` stays live until
        // `shutdown_bridge` is deleted right below.
        unsafe { call_void_bool_bool(env, obj, method, enabled, restart) };
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
        PENDING_RUNNER.with(|pending| host.runner = pending.borrow_mut().take());
        *slot.borrow_mut() = Some(host);
    });
    if let Some(entry) = APP_ENTRY.get() { entry(); }
}

unsafe extern "system" fn native_surface_created(
    env: *mut JNIEnv, _this: jobject, surface: jobject, width: jint, height: jint, density: jfloat, dark: jboolean,
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
    env: *mut JNIEnv, _this: jobject, width: jint, height: jint, density: jfloat,
) {
    let _ = dispatch(
        env,
        Event::SurfaceChanged { width_px: width.max(0) as u32, height_px: height.max(0) as u32, scale: density },
        ptr::null_mut(),
    );
}

unsafe extern "system" fn native_surface_destroyed(env: *mut JNIEnv, _this: jobject) {
    let _ = dispatch(env, Event::SurfaceDestroyed, ptr::null_mut());
}

unsafe extern "system" fn native_frame(env: *mut JNIEnv, _this: jobject) {
    let _ = dispatch(env, Event::Frame, ptr::null_mut());
}

unsafe extern "system" fn native_insets(
    env: *mut JNIEnv, _this: jobject, top: jint, right: jint, bottom: jint, left: jint, ime_bottom: jint, density: jfloat,
) {
    let _ = dispatch(
        env,
        Event::InsetsChanged { safe_area_px: [top, right, bottom, left], keyboard_px: ime_bottom, scale: density },
        ptr::null_mut(),
    );
}

unsafe extern "system" fn native_dark_mode(env: *mut JNIEnv, _this: jobject, dark: jboolean) {
    let _ = dispatch(env, Event::DarkModeChanged(dark != JNI_FALSE), ptr::null_mut());
}

unsafe extern "system" fn native_resume(env: *mut JNIEnv, _this: jobject) {
    let _ = dispatch(env, Event::Resumed, ptr::null_mut());
}

unsafe extern "system" fn native_pause(env: *mut JNIEnv, _this: jobject) {
    HOST.with(|slot| {
        if let Some(runner) = slot.borrow_mut().as_mut().and_then(|h| h.runner.as_mut()) {
            let _ = runner.take_haptics();
        }
    });
    let _ = dispatch(env, Event::Paused, ptr::null_mut());
}

unsafe extern "system" fn native_back(env: *mut JNIEnv, _this: jobject) -> jboolean {
    if dispatch(env, Event::BackPressed, ptr::null_mut()) { JNI_TRUE } else { JNI_FALSE }
}

unsafe extern "system" fn native_touch(
    env: *mut JNIEnv, _this: jobject, action: jint, flags: jint, pointer_id: jint, x_px: jfloat, y_px: jfloat, time_ms: jlong,
) {
    let _ = dispatch(env, Event::Touch { action, flags, pointer_id, x_px, y_px, time_ms }, ptr::null_mut());
}

unsafe extern "system" fn native_long_press_timeout(env: *mut JNIEnv, _this: jobject, timeout_ms: jint) {
    let _ = dispatch(env, Event::LongPressTimeoutChanged(timeout_ms), ptr::null_mut());
}

unsafe extern "system" fn native_wake(env: *mut JNIEnv, _this: jobject) {
    HOST.with(|slot| {
        if let Some(host) = slot.borrow_mut().as_mut()
            && let Some(runner) = &mut host.runner
        {
            runner.drain_shared();
            if !host.state.resumed { let _ = runner.take_haptics(); }
        }
    });
    let _ = dispatch(env, Event::Wake, ptr::null_mut());
}

unsafe extern "system" fn native_destroy(env: *mut JNIEnv, _this: jobject) {
    let _ = dispatch(env, Event::Destroy, ptr::null_mut());
}
