//! Pure lifecycle, surface, inset, dark-mode, back-navigation, and Choreographer frame-scheduling
//! state machine for the Android native host (`dev.rune.RuneView`).
//!
//! Every mode is encoded explicitly in [`State`] and [`Surface`]; [`step`] returns declarative
//! [`Effect`]s instead of performing JNI or GPU work directly.

use super::touch;
use crate::TouchPhase;
use crate::machine::Effects;

/// Current `ANativeWindow` attachment state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Surface {
    /// No `ANativeWindow` is attached (before `surfaceCreated` or after `surfaceDestroyed`).
    #[default]
    Detached,
    /// An `ANativeWindow` is attached with physical pixel dimensions and display density.
    Attached {
        width_px: u32,
        height_px: u32,
        scale: f32,
        frame_scheduled: bool,
    },
}

/// Complete host lifecycle state for one `RuneView` instance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct State {
    pub(crate) surface: Surface,
    pub(crate) resumed: bool,
    pub(crate) pending_wake: bool,
    pub(crate) dark: bool,
    /// Hardware system bars and display cutout insets `[top, right, bottom, left]` in logical dp.
    /// Never includes transient software keyboard height.
    pub(crate) safe_area: [f32; 4],
    /// Software keyboard bottom overlap in logical dp, kept separate from `safe_area`.
    pub(crate) keyboard_inset: f32,
    /// System long-press delay in seconds from `ViewConfiguration.getLongPressTimeout()`,
    /// when positive (`None` keeps Rune's 0.5 s default).
    pub(crate) long_press_secs: Option<f64>,
    /// Whether `RuneView` currently advertises an active `InputConnection` / soft keyboard.
    pub(crate) ime_enabled: bool,
    pub(crate) destroyed: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            surface: Surface::Detached,
            resumed: false,
            pending_wake: false,
            dark: true,
            safe_area: [0.0; 4],
            keyboard_inset: 0.0,
            long_press_secs: None,
            ime_enabled: false,
            destroyed: false,
        }
    }
}

impl State {
    #[cfg(test)]
    #[inline]
    pub(crate) fn is_attached(&self) -> bool {
        matches!(self.surface, Surface::Attached { .. })
    }

    #[inline]
    fn can_draw(&self) -> bool {
        !self.destroyed
            && self.resumed
            && matches!(self.surface, Surface::Attached { width_px, height_px, .. } if width_px > 0 && height_px > 0)
    }
}

/// Input events delivered by `dev.rune.RuneView`, `dev.rune.RuneActivity`, Choreographer, and
/// background wake handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Event {
    Resumed,
    Paused,
    SurfaceCreated {
        width_px: u32,
        height_px: u32,
        scale: f32,
        dark: bool,
    },
    SurfaceChanged {
        width_px: u32,
        height_px: u32,
        scale: f32,
    },
    SurfaceDestroyed,
    /// Choreographer vsync callback fired.
    Frame,
    /// Outcome of [`Effect::DrawFrame`], including post-frame `ImeSync` decisions.
    FrameDone {
        animating: bool,
        wake_after_secs: Option<f64>,
        ime_allowed: Option<bool>,
        ime_restart: bool,
    },
    /// Background thread or timer woke the runtime.
    Wake,
    /// `WindowInsets` changed: system bars + cutout in pixels, and IME bottom overlap in pixels.
    /// Does not resize the layout viewport (`adjustNothing`).
    InsetsChanged {
        safe_area_px: [i32; 4],
        keyboard_px: i32,
        scale: f32,
    },
    DarkModeChanged(bool),
    /// System Back button or gesture pressed.
    BackPressed,
    /// Outcome of [`Effect::DispatchBack`]: whether Rune consumed `Key::Escape`.
    BackDone {
        consumed: bool,
    },
    /// System touch long-press timeout in milliseconds (`ViewConfiguration.getLongPressTimeout`).
    LongPressTimeoutChanged(i32),
    /// One Android `MotionEvent` touch sample (including batched historical samples).
    Touch {
        action: i32,
        flags: i32,
        pointer_id: i32,
        x_px: f32,
        y_px: f32,
        time_ms: i64,
    },
    /// Activity destroyed.
    Destroy,
}

/// Side effects the JNI host performs in response to [`step`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Effect {
    AttachSurface {
        viewport: (f32, f32),
        scale: f32,
    },
    ResizeSurface {
        viewport: (f32, f32),
        scale: f32,
    },
    DetachSurface,
    SetSafeArea([f32; 4]),
    SetKeyboardInset(f32),
    SetDark(bool),
    /// Update `Runtime::set_touch_long_press` (`None` restores Rune's 0.5 s default).
    SetTouchLongPress(Option<f64>),
    /// Deliver one touch sample in logical dp and seconds to `Runtime::touch`.
    Touch {
        id: u64,
        phase: TouchPhase,
        x: f32,
        y: f32,
        time_secs: f64,
    },
    /// Queue `Key::Escape`, run one synchronous zero-dt frame to route it, and feed
    /// [`Event::BackDone`] with whether Rune consumed the key (`Runtime::take_key_consumed`).
    DispatchBack,
    /// Enable or disable `RuneView`'s `InputConnection` and show or hide the soft keyboard.
    SetImeEnabled {
        enabled: bool,
        restart: bool,
    },
    /// Commit any open composition and clear host `ImeSync` when pausing while an editor is active.
    FinishComposition,
    ScheduleFrame,
    CancelFrame,
    ScheduleWakeAfter(f64),
    DrawFrame,
    Shutdown,
}

#[inline]
fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

#[inline]
fn px_to_viewport(width_px: u32, height_px: u32, scale: f32) -> (f32, f32) {
    (width_px as f32 / scale, height_px as f32 / scale)
}

#[inline]
fn px_to_dp(px: i32, scale: f32) -> f32 {
    (px.max(0) as f32) / scale
}

/// Schedules a Choreographer frame when drawing is possible and no frame is already scheduled;
/// otherwise remembers `pending_wake` so drawing starts once resumed with a live surface.
fn request_frame(state: &mut State, effects: &mut Effects<Effect>) {
    if state.can_draw() {
        if let Surface::Attached { frame_scheduled, .. } = &mut state.surface
            && !*frame_scheduled
        {
            *frame_scheduled = true;
            state.pending_wake = false;
            effects.push(Effect::ScheduleFrame);
        }
    } else if !state.destroyed {
        state.pending_wake = true;
    }
}

/// Advances the Android host state machine by one [`Event`], returning the ordered [`Effect`]s
/// the native bridge should execute.
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let mut effects = Effects::none();
    if state.destroyed {
        return effects;
    }

    match event {
        Event::SurfaceCreated {
            width_px,
            height_px,
            scale,
            dark,
        } => {
            if let Surface::Attached { frame_scheduled, .. } = state.surface {
                if frame_scheduled {
                    effects.push(Effect::CancelFrame);
                }
                effects.push(Effect::DetachSurface);
            }
            let scale = sanitize_scale(scale);
            let viewport = px_to_viewport(width_px, height_px, scale);
            state.dark = dark;
            state.surface = Surface::Attached {
                width_px,
                height_px,
                scale,
                frame_scheduled: false,
            };
            effects.push(Effect::AttachSurface { viewport, scale });
            effects.push(Effect::SetDark(state.dark));
            effects.push(Effect::SetSafeArea(state.safe_area));
            effects.push(Effect::SetKeyboardInset(state.keyboard_inset));
            if state.long_press_secs.is_some() {
                effects.push(Effect::SetTouchLongPress(state.long_press_secs));
            }
            request_frame(state, &mut effects);
        }
        Event::SurfaceChanged {
            width_px,
            height_px,
            scale,
        } => {
            let next_scale = sanitize_scale(scale);
            let mut zero_size = false;
            let mut resized = None;
            if let Surface::Attached {
                width_px: w,
                height_px: h,
                scale: s,
                frame_scheduled,
            } = &mut state.surface
            {
                if width_px == 0 || height_px == 0 {
                    if *frame_scheduled {
                        *frame_scheduled = false;
                        effects.push(Effect::CancelFrame);
                    }
                    *w = width_px;
                    *h = height_px;
                    *s = next_scale;
                    zero_size = true;
                } else {
                    *w = width_px;
                    *h = height_px;
                    *s = next_scale;
                    resized = Some((px_to_viewport(width_px, height_px, next_scale), next_scale));
                }
            }
            if zero_size {
                state.pending_wake = true;
            } else if let Some((viewport, scale)) = resized {
                effects.push(Effect::ResizeSurface { viewport, scale });
                request_frame(state, &mut effects);
            }
        }
        Event::SurfaceDestroyed => {
            if let Surface::Attached { frame_scheduled, .. } = state.surface {
                if frame_scheduled {
                    effects.push(Effect::CancelFrame);
                }
                state.surface = Surface::Detached;
                effects.push(Effect::DetachSurface);
            }
        }
        Event::Resumed => {
            let was_resumed = state.resumed;
            state.resumed = true;
            if !was_resumed || state.pending_wake {
                request_frame(state, &mut effects);
            }
        }
        Event::Paused => {
            state.resumed = false;
            if let Surface::Attached { frame_scheduled, .. } = &mut state.surface
                && *frame_scheduled
            {
                *frame_scheduled = false;
                state.pending_wake = true;
                effects.push(Effect::CancelFrame);
            }
            if state.ime_enabled {
                state.ime_enabled = false;
                effects.push(Effect::FinishComposition);
                effects.push(Effect::SetImeEnabled {
                    enabled: false,
                    restart: true,
                });
            }
        }
        Event::Frame => {
            let can_draw = state.can_draw();
            if let Surface::Attached { frame_scheduled, .. } = &mut state.surface {
                *frame_scheduled = false;
            }
            if can_draw {
                effects.push(Effect::DrawFrame);
            }
        }
        Event::FrameDone {
            animating,
            wake_after_secs,
            ime_allowed,
            ime_restart,
        } => {
            if !state.can_draw() {
                return effects;
            }
            match ime_allowed {
                Some(true) => {
                    let was = state.ime_enabled;
                    if !was || ime_restart {
                        state.ime_enabled = true;
                        effects.push(Effect::SetImeEnabled {
                            enabled: true,
                            restart: !was || ime_restart,
                        });
                    }
                }
                Some(false) => {
                    if state.ime_enabled {
                        state.ime_enabled = false;
                        effects.push(Effect::SetImeEnabled {
                            enabled: false,
                            restart: true,
                        });
                    }
                }
                None => {
                    if ime_restart && state.ime_enabled {
                        effects.push(Effect::SetImeEnabled {
                            enabled: true,
                            restart: true,
                        });
                    }
                }
            }
            if animating || state.pending_wake {
                request_frame(state, &mut effects);
            } else if let Some(secs) = wake_after_secs
                && secs.is_finite()
            {
                if secs <= 0.0 {
                    request_frame(state, &mut effects);
                } else {
                    effects.push(Effect::ScheduleWakeAfter(secs));
                }
            }
        }
        Event::Wake => {
            request_frame(state, &mut effects);
        }
        Event::InsetsChanged {
            safe_area_px,
            keyboard_px,
            scale,
        } => {
            let scale = sanitize_scale(scale);
            let safe = safe_area_px.map(|px| px_to_dp(px, scale));
            let kb = px_to_dp(keyboard_px, scale);
            let mut changed = false;
            if safe != state.safe_area {
                state.safe_area = safe;
                effects.push(Effect::SetSafeArea(safe));
                changed = true;
            }
            if kb != state.keyboard_inset {
                state.keyboard_inset = kb;
                effects.push(Effect::SetKeyboardInset(kb));
                changed = true;
            }
            if changed {
                request_frame(state, &mut effects);
            }
        }
        Event::DarkModeChanged(dark) => {
            if dark != state.dark {
                state.dark = dark;
                effects.push(Effect::SetDark(dark));
                request_frame(state, &mut effects);
            }
        }
        Event::BackPressed => {
            effects.push(Effect::DispatchBack);
        }
        Event::BackDone { consumed } => {
            if consumed {
                request_frame(state, &mut effects);
            }
        }
        Event::LongPressTimeoutChanged(timeout_ms) => {
            let next = touch::long_press_ms_to_secs(timeout_ms);
            if next != state.long_press_secs {
                state.long_press_secs = next;
                if matches!(state.surface, Surface::Attached { .. }) {
                    effects.push(Effect::SetTouchLongPress(next));
                }
            }
        }
        Event::Touch {
            action,
            flags,
            pointer_id,
            x_px,
            y_px,
            time_ms,
        } => {
            let Surface::Attached { scale, .. } = state.surface else {
                return effects;
            };
            let Some(phase) = touch::action_to_phase(action, flags) else {
                return effects;
            };
            let Some((x, y)) = touch::px_to_dp_point(x_px, y_px, scale) else {
                return effects;
            };
            effects.push(Effect::Touch {
                id: touch::pointer_id_to_u64(pointer_id),
                phase,
                x,
                y,
                time_secs: touch::event_time_ms_to_secs(time_ms),
            });
            request_frame(state, &mut effects);
        }
        Event::Destroy => {
            state.destroyed = true;
            state.resumed = false;
            if state.ime_enabled {
                state.ime_enabled = false;
                effects.push(Effect::SetImeEnabled {
                    enabled: false,
                    restart: false,
                });
            }
            if let Surface::Attached { frame_scheduled, .. } = state.surface {
                if frame_scheduled {
                    effects.push(Effect::CancelFrame);
                }
                state.surface = Surface::Detached;
                effects.push(Effect::DetachSurface);
            }
            effects.push(Effect::Shutdown);
        }
    }

    effects
}

/// Routes an Android Back press through `rt` as `Key::Escape` on a synchronous zero-dt frame and
/// returns whether Rune consumed the key (`RuntimeCore::take_key_consumed`).
///
/// Mirrors the web host's key-consumption contract: active modals, app `on_key` panels, focused
/// text fields (blur or IME composition), and active drags consume Escape so the Activity stays;
/// focused controls that ignore Escape (such as a tapped button or switch) return `false` so
/// Android navigates back.
pub(crate) fn dispatch_back_escape_core(
    rt: &mut crate::runtime::RuntimeCore,
    app: &mut dyn crate::runtime::AppDriver,
) -> bool {
    rt.take_key_consumed();
    rt.key(crate::KeyEvent {
        key: crate::Key::Escape,
        mods: crate::Mods::default(),
    });
    let _ = rt.frame(app, 0.0);
    rt.take_key_consumed()
}

#[cfg(test)]
pub(crate) fn dispatch_back_escape<S: 'static>(rt: &mut crate::Runtime<S>) -> bool {
    let (core, mut app) = rt.split_mut();
    dispatch_back_escape_core(core, &mut app)
}
