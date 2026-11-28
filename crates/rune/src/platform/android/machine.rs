//! Pure lifecycle, surface, inset, dark-mode, back-navigation, and Choreographer frame-scheduling
//! state machine for the Android native host (`dev.rune.RuneView`).
//!
//! Every mode is encoded explicitly in [`State`] and [`Surface`]; [`step`] returns declarative
//! [`Effect`]s instead of performing JNI or GPU work directly.

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
    /// Outcome of [`Effect::DrawFrame`].
    FrameDone {
        animating: bool,
        wake_after_secs: Option<f64>,
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
    /// System Back button or gesture pressed; `has_focus` reports `Runtime::has_focus()`.
    BackPressed {
        has_focus: bool,
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
    /// Deliver `Key::Escape` to blur focused input or dismiss a modal; consumes the Back event.
    SendEscapeKey,
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
        } => {
            if !state.can_draw() {
                return effects;
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
        Event::BackPressed { has_focus } => {
            if has_focus {
                effects.push(Effect::SendEscapeKey);
                request_frame(state, &mut effects);
            }
        }
        Event::Destroy => {
            state.destroyed = true;
            state.resumed = false;
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

/// Whether the effects emitted for [`Event::BackPressed`] consumed the event inside Rune.
#[inline]
pub(crate) fn consumed_back(effects: &Effects<Effect>) -> bool {
    match effects {
        Effects::None => false,
        Effects::One(a) => matches!(a, Effect::SendEscapeKey),
        Effects::Two(a, b) => matches!(a, Effect::SendEscapeKey) || matches!(b, Effect::SendEscapeKey),
        Effects::Many(v) => v.iter().any(|e| matches!(e, Effect::SendEscapeKey)),
    }
}
