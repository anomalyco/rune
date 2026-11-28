//! Type-erased boundary between the Android UI-thread host (`host.rs`) and a concrete `App<S>` /
//! `LayerHost<S>`.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::app::{RuntimeConfig, Shared};
use crate::compose::Compositor;
use crate::input::host_events::{ImeStep, ImeSync, ime_sync_step};
use crate::platform::embed::{EmbedFrame, LayerHost};
use crate::runtime::Waker;
use crate::tooling::AppSpec;
use crate::{Haptic, KeyEvent, TouchPhase};

use super::document::Document;
use super::host::ViewBridge;
use super::input::{self, TextInputAction};
use super::picker::{self, PickAnswer, PickStep};

pub(super) trait AndroidRunner {
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
    #[cfg(feature = "accessibility")]
    fn a11y_receive(&mut self, a11y: &mut crate::a11y::android::Adapter) -> bool;
    #[cfg(feature = "accessibility")]
    fn a11y_sync(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome>;
    #[cfg(feature = "accessibility")]
    fn a11y_wake(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        attempt: crate::machine::Attempt,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome>;
}

pub(super) struct PendingAndroidApp<S: 'static> {
    pub(super) spec: AppSpec<S>,
    pub(super) state: S,
    pub(super) runtime_config: RuntimeConfig<S>,
    pub(super) compositor: Option<Box<dyn Compositor<S>>>,
}

pub(super) struct TypedRunner<S: 'static> {
    pub(super) pending: Option<PendingAndroidApp<S>>,
    pub(super) host: Option<LayerHost<S>>,
    pub(super) shared: Arc<Shared<S>>,
    pub(super) clipboard: crate::clipboard::Client,
    pub(super) ime: ImeSync,
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
        if let Some(host) = &mut self.host {
            host.resize(viewport, scale);
        }
    }

    fn detach(&mut self) {
        if let Some(host) = &mut self.host {
            host.detach_surface();
        }
    }

    fn set_safe_area(&mut self, insets: [f32; 4]) {
        if let Some(host) = &mut self.host {
            host.runtime_mut().set_safe_area(insets);
        }
    }

    fn set_keyboard_inset(&mut self, points: f32) {
        if let Some(host) = &mut self.host {
            host.runtime_mut().set_keyboard_inset(points);
        }
    }

    fn set_dark(&mut self, dark: bool) {
        if let Some(host) = &mut self.host {
            host.runtime_mut().set_system_dark(dark);
        }
    }

    fn set_touch_long_press(&mut self, secs: Option<f64>) {
        if let Some(host) = &mut self.host {
            host.runtime_mut().set_touch_long_press(secs.unwrap_or(0.0));
        }
    }

    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time_secs: f64) {
        if let Some(host) = &mut self.host {
            host.runtime_mut().touch(id, phase, x, y, time_secs);
        }
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

    #[cfg(feature = "accessibility")]
    fn a11y_receive(&mut self, a11y: &mut crate::a11y::android::Adapter) -> bool {
        self.host.as_mut().is_some_and(|h| a11y.receive(h.runtime_mut().core_mut(), ""))
    }

    #[cfg(feature = "accessibility")]
    fn a11y_sync(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome> {
        Some(a11y.sync_on_frame(self.host.as_mut()?.runtime_mut().core_mut(), now))
    }

    #[cfg(feature = "accessibility")]
    fn a11y_wake(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        attempt: crate::machine::Attempt,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome> {
        Some(a11y.sync_on_wake(self.host.as_mut()?.runtime_mut().core_mut(), attempt, now))
    }
}
