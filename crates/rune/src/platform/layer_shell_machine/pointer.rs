//! Wayland `wl_pointer` button mapping and per-frame axis accumulation, matching winit's
//! Wayland backend (`vendor/winit/src/platform_impl/linux/wayland/seat/pointer/mod.rs`) and
//! Rune's Linux wheel path (`input/host_events.rs`, `runtime/scroll/x.rs`).

use crate::input::host_events::WheelInput;
use crate::machine::Effects;

/// Window points per logical wheel detent on Linux (matches `runtime::scroll::x::LINE_POINTS`
/// on Linux, Chromium `kWheelDelta`).
pub(crate) const WHEEL_LINE_POINTS: f32 = 120.0;
/// Window points per Wayland `wl_pointer.axis` surface unit for finger/continuous scrolling
/// (matches `vendor/winit/src/platform_impl/linux/wayland/seat/pointer/mod.rs`).
pub(crate) const FINGER_POINTS_PER_AXIS_UNIT: f64 = 120.0 / 10.0;

/// Maps a Linux `input-event-codes.h` pointer button code to Rune's [`crate::PointerButton`],
/// matching winit's `wayland_button_to_winit` and `window::pointer_button`.
pub(crate) fn button_from_linux(code: u32) -> crate::PointerButton {
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;
    const BTN_MIDDLE: u32 = 0x112;
    const BTN_SIDE: u32 = 0x113;
    const BTN_EXTRA: u32 = 0x114;
    const BTN_FORWARD: u32 = 0x115;
    const BTN_BACK: u32 = 0x116;

    match code {
        BTN_LEFT => crate::PointerButton::Primary,
        BTN_RIGHT => crate::PointerButton::Secondary,
        BTN_MIDDLE => crate::PointerButton::Middle,
        BTN_SIDE | BTN_BACK => crate::PointerButton::Back,
        BTN_EXTRA | BTN_FORWARD => crate::PointerButton::Forward,
        other => crate::PointerButton::Other(other as u16),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis { Vertical, Horizontal }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AxisSource { Wheel, Finger, Continuous, WheelTilt }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct AxisAccum { absolute: f64, value120: i32, discrete: i32, stop: bool }

impl AxisAccum {
    fn has_motion(self) -> bool { self.value120 != 0 || self.discrete != 0 || self.absolute != 0.0 }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PendingFrame { horizontal: AxisAccum, vertical: AxisAccum, source: Option<AxisSource> }

impl PendingFrame {
    fn axis_mut(&mut self, axis: Axis) -> &mut AxisAccum {
        match axis { Axis::Horizontal => &mut self.horizontal, Axis::Vertical => &mut self.vertical }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum State {
    /// No continuous scroll gesture is active.
    Idle { pending: Option<PendingFrame> },
    /// A continuous scroll gesture (touchpad / continuous) has begun and not yet stopped.
    Active { pending: Option<PendingFrame> },
}

impl Default for State {
    fn default() -> Self { Self::Idle { pending: None } }
}

impl State {
    fn pending_mut(&mut self) -> &mut PendingFrame {
        match self { Self::Idle { pending } | Self::Active { pending } => pending.get_or_insert_with(PendingFrame::default) }
    }
    fn take_pending(&mut self) -> Option<PendingFrame> {
        match self { Self::Idle { pending } | Self::Active { pending } => pending.take() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Event {
    Axis { axis: Axis, value: f64 },
    AxisSource(AxisSource),
    AxisDiscrete { axis: Axis, discrete: i32 },
    AxisValue120 { axis: Axis, value120: i32 },
    AxisStop { axis: Axis },
    /// End of a `wl_pointer` frame (`wl_pointer.frame` for v5+, or synthetic per-axis for < v5).
    Frame { mods: crate::Mods, framed: bool },
    /// Pointer entered/left, seat lost pointer capability, or surface drained while hidden.
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Effect {
    Wheel(WheelInput),
}

pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::Reset => {
            *state = State::Idle { pending: None };
            return Effects::none();
        }
        Event::Axis { axis, value } => {
            state.pending_mut().axis_mut(axis).absolute += value;
            return Effects::none();
        }
        Event::AxisSource(source) => {
            state.pending_mut().source = Some(source);
            return Effects::none();
        }
        Event::AxisDiscrete { axis, discrete } => {
            let slot = state.pending_mut().axis_mut(axis);
            slot.discrete = slot.discrete.saturating_add(discrete);
            return Effects::none();
        }
        Event::AxisValue120 { axis, value120 } => {
            let slot = state.pending_mut().axis_mut(axis);
            slot.value120 = slot.value120.saturating_add(value120);
            return Effects::none();
        }
        Event::AxisStop { axis } => {
            state.pending_mut().axis_mut(axis).stop = true;
            return Effects::none();
        }
        Event::Frame { .. } => {}
    }

    let Event::Frame { mods, framed } = event else { unreachable!() };
    let Some(frame) = state.take_pending() else {
        return Effects::none();
    };
    let active = matches!(*state, State::Active { .. });
    let (h, v) = (frame.horizontal, frame.vertical);
    let has_value120 = h.value120 != 0 || v.value120 != 0;
    let has_discrete = h.discrete != 0 || v.discrete != 0;
    let any_stop = h.stop || v.stop;
    let all_moving_stopped = any_stop && (!h.has_motion() || h.stop) && (!v.has_motion() || v.stop);

    // Negate Wayland signs: positive Wayland axis moves the viewport down/right, while Rune
    // (matching winit) uses positive deltas to move content down/right.
    let (raw_dx, raw_dy, raw_ticks, precise, notch) = if has_value120 {
        (
            -(h.value120 as f32) * (WHEEL_LINE_POINTS / 120.0),
            -(v.value120 as f32) * (WHEEL_LINE_POINTS / 120.0),
            Some(-(v.value120 as f32) / 120.0),
            false,
            h.value120 % 120 == 0 && v.value120 % 120 == 0,
        )
    } else if has_discrete {
        (
            -(h.discrete as f32) * WHEEL_LINE_POINTS,
            -(v.discrete as f32) * WHEEL_LINE_POINTS,
            Some(-(v.discrete as f32)),
            false,
            true,
        )
    } else {
        let gain = match frame.source {
            Some(AxisSource::Finger | AxisSource::Continuous) => FINGER_POINTS_PER_AXIS_UNIT,
            _ => 1.0,
        };
        ((-h.absolute * gain) as f32, (-v.absolute * gain) as f32, None, true, false)
    };

    let moved = raw_dx != 0.0 || raw_dy != 0.0;
    if !moved && (!all_moving_stopped || !active) {
        if all_moving_stopped {
            *state = State::Idle { pending: None };
        }
        return Effects::none();
    }

    let phased = framed
        && !has_value120
        && !has_discrete
        && !matches!(frame.source, Some(AxisSource::Wheel | AxisSource::WheelTilt));

    let phase = if all_moving_stopped {
        *state = State::Idle { pending: None };
        crate::GesturePhase::Ended
    } else if !phased {
        *state = State::Idle { pending: None };
        crate::GesturePhase::Moved
    } else if active {
        crate::GesturePhase::Moved
    } else {
        *state = State::Active { pending: None };
        crate::GesturePhase::Began
    };

    let clean = |v: f32| if v == 0.0 { 0.0 } else { v };
    let shift_turn = mods.shift && !mods.ctrl && raw_dx == 0.0;
    let (dx, dy) = if shift_turn {
        (clean(raw_dy), 0.0)
    } else {
        (clean(raw_dx), clean(raw_dy))
    };
    let ticks = if shift_turn { None } else { raw_ticks.map(clean) };

    Effects::one(Effect::Wheel(WheelInput {
        dx,
        dy,
        ticks,
        mods,
        phase,
        precise,
        notch,
        pinch: true,
    }))
}

#[cfg(test)]
#[path = "pointer_tests.rs"]
mod tests;
