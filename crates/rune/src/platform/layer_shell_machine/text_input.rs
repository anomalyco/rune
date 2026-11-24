//! Wayland `zwp_text_input_v3` state machine for layer-shell overlays, kept cross-platform so
//! its tests run headless on every host. Matches winit's Wayland text-input path and Chromium's
//! Ozone Wayland `ZwpTextInputV3Impl` (independent pending preedit/commit buffers, batched
//! enable/content-type/cursor-rectangle commits, cursor-rect replay on `enter`, and silent idle
//! `done` events so resting overlays draw nothing).

use crate::input::host_events::ImeStep;
use crate::machine::Effects;

/// Surface-local logical cursor rectangle for `zwp_text_input_v3.set_cursor_rectangle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

impl CursorRect {
    /// Converts logical window-point `[x, y, width, height]` into surface-local integers.
    pub(crate) fn from_logical(area: [f32; 4]) -> Self {
        let round = |v: f32| if v.is_finite() { v.round() as i32 } else { 0 };
        let nonneg = |v: f32| if v.is_finite() { v.round().max(0.0) as i32 } else { 0 };
        Self {
            x: round(area[0]),
            y: round(area[1]),
            width: nonneg(area[2]),
            height: nonneg(area[3]),
        }
    }
}

/// Host IME allowance requested while the surface is not yet entered by `zwp_text_input_v3`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Desired {
    #[default]
    Disabled,
    Enabled { cursor_rect: Option<CursorRect> },
}

/// Pending double-buffered preedit received between `done` events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Preedit {
    pub(crate) text: String,
    pub(crate) cursor: Option<(usize, usize)>,
}

/// Per-surface `zwp_text_input_v3` mode: surface focus, wire enablement, and double-buffered
/// IME composition state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Text-input focus is not on this surface. Remembers desired host state so `enter` can
    /// immediately enable and replay the cursor rectangle if IME was already allowed.
    Outside { desired: Desired },
    /// Text-input focus is on this surface, and IME is currently disabled on the wire.
    EnteredDisabled,
    /// Text-input focus is on this surface, and IME is currently enabled on the wire.
    EnteredEnabled {
        /// Last logical cursor rectangle sent since the latest `enable`, so duplicate updates
        /// (for example after `Ime::Enabled` invalidates `ImeSync`) emit no extra `commit`.
        sent_rect: Option<CursorRect>,
        /// Double-buffered `preedit_string` received since the previous `done`.
        pending_preedit: Option<Preedit>,
        /// Double-buffered `commit_string` received since the previous `done`.
        pending_commit: Option<String>,
        /// Whether the overlay currently displays a non-empty preedit from this text input.
        composing: bool,
    },
}

impl Default for State {
    fn default() -> Self {
        Self::Outside { desired: Desired::Disabled }
    }
}

/// Events stepping the layer-shell `zwp_text_input_v3` state machine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// `zwp_text_input_v3.enter` for this layer surface (`ours = true`) or another surface.
    Enter { ours: bool },
    /// `zwp_text_input_v3.leave` for this layer surface (`ours = true`) or another surface.
    Leave { ours: bool },
    /// `zwp_text_input_v3.preedit_string`.
    Preedit { text: Option<String>, cursor_begin: i32, cursor_end: i32 },
    /// `zwp_text_input_v3.commit_string`.
    Commit { text: Option<String> },
    /// `zwp_text_input_v3.done`.
    Done,
    /// Host IME sync produced by `host_events::ime_sync_step`.
    Sync(ImeStep),
    /// Surface is hidden and draining its queue: disable on the wire without losing enter state.
    Drain,
    /// Display connection failed or proxies are tearing down: drop all state with no wire writes.
    Gone,
}

/// Wire requests and overlay input events requested by one transition, in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    Enable,
    Disable,
    SetContentTypeNormal,
    SetCursorRectangle(CursorRect),
    Commit,
    Ime(crate::Ime),
    ResetCompose,
}

/// Validates `zwp_text_input_v3.preedit_string` cursor byte offsets against UTF-8 char
/// boundaries in `text`. Negative begin (including `(-1, -1)` hidden cursor) or invalid byte
/// indices yield `None`; an invalid end with a valid begin collapses to a caret at `begin`.
pub(crate) fn validate_preedit_cursor(text: &str, cursor_begin: i32, cursor_end: i32) -> Option<(usize, usize)> {
    if text.is_empty() {
        return None;
    }
    let begin = usize::try_from(cursor_begin).ok().filter(|&i| text.is_char_boundary(i))?;
    let end = usize::try_from(cursor_end).ok().filter(|&i| text.is_char_boundary(i)).unwrap_or(begin);
    Some((begin, end))
}

fn push_enable(effects: &mut Effects<Effect>, rect: Option<CursorRect>, emit_ime: bool) {
    effects.push(Effect::Enable);
    effects.push(Effect::SetContentTypeNormal);
    if let Some(rect) = rect {
        effects.push(Effect::SetCursorRectangle(rect));
    }
    effects.push(Effect::Commit);
    if emit_ime {
        effects.push(Effect::Ime(crate::Ime::Enabled));
    }
}

/// Advances `state` by `event` and returns the ordered wire requests and IME events to emit.
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::Enter { ours: true } => match std::mem::take(state) {
            State::Outside { desired: Desired::Disabled } | State::EnteredDisabled => {
                *state = State::EnteredDisabled;
                Effects::none()
            }
            State::Outside { desired: Desired::Enabled { cursor_rect } } => {
                *state = State::EnteredEnabled {
                    sent_rect: cursor_rect,
                    pending_preedit: None,
                    pending_commit: None,
                    composing: false,
                };
                let mut effects = Effects::none();
                push_enable(&mut effects, cursor_rect, true);
                effects
            }
            State::EnteredEnabled { sent_rect, .. } => {
                // Per zwp_text_input_v3, enter invalidates all previous state.
                *state = State::EnteredEnabled {
                    sent_rect,
                    pending_preedit: None,
                    pending_commit: None,
                    composing: false,
                };
                let mut effects = Effects::none();
                push_enable(&mut effects, sent_rect, true);
                effects
            }
        },
        Event::Enter { ours: false } | Event::Leave { ours: true } => match std::mem::take(state) {
            State::Outside { desired } => {
                *state = State::Outside { desired };
                Effects::none()
            }
            State::EnteredDisabled => {
                *state = State::Outside { desired: Desired::Disabled };
                Effects::none()
            }
            State::EnteredEnabled { sent_rect, .. } => {
                *state = State::Outside { desired: Desired::Enabled { cursor_rect: sent_rect } };
                let mut effects = Effects::two(Effect::Disable, Effect::Commit);
                effects.push(Effect::Ime(crate::Ime::Disabled));
                effects
            }
        },
        Event::Leave { ours: false } => Effects::none(),
        Event::Preedit { text, cursor_begin, cursor_end } => {
            if let State::EnteredEnabled { pending_preedit, .. } = state {
                let text = text.unwrap_or_default();
                let cursor = validate_preedit_cursor(&text, cursor_begin, cursor_end);
                *pending_preedit = Some(Preedit { text, cursor });
            }
            Effects::none()
        }
        Event::Commit { text } => {
            if let State::EnteredEnabled { pending_commit, .. } = state {
                *pending_commit = text.filter(|s| !s.is_empty());
            }
            Effects::none()
        }
        Event::Done => {
            let State::EnteredEnabled { pending_preedit, pending_commit, composing, .. } = state else {
                return Effects::none();
            };
            let commit = pending_commit.take();
            let preedit = pending_preedit.take().filter(|p| !p.text.is_empty());
            let was_composing = *composing;
            let had_commit = commit.is_some();
            let mut effects = Effects::none();
            if had_commit || preedit.is_some() {
                effects.push(Effect::ResetCompose);
            }
            if let Some(text) = commit {
                effects.push(Effect::Ime(crate::Ime::Commit(text)));
            }
            match preedit {
                Some(p) => {
                    *composing = true;
                    effects.push(Effect::Ime(crate::Ime::Preedit(p.text, p.cursor, Vec::new())));
                }
                None => {
                    *composing = false;
                    if was_composing && !had_commit {
                        effects.push(Effect::Ime(crate::Ime::Preedit(String::new(), None, Vec::new())));
                    }
                }
            }
            effects
        }
        Event::Sync(sync) => {
            let rect = sync.cursor_area.map(CursorRect::from_logical);
            match state {
                State::Outside { desired } => {
                    match sync.set_allowed {
                        Some(false) => *desired = Desired::Disabled,
                        Some(true) => *desired = Desired::Enabled { cursor_rect: rect },
                        None => {
                            if let Desired::Enabled { cursor_rect } = desired
                                && let Some(r) = rect
                            {
                                *cursor_rect = Some(r);
                            }
                        }
                    }
                    Effects::none()
                }
                State::EnteredDisabled => {
                    if sync.set_allowed == Some(true) {
                        *state = State::EnteredEnabled {
                            sent_rect: rect,
                            pending_preedit: None,
                            pending_commit: None,
                            composing: false,
                        };
                        let mut effects = Effects::none();
                        push_enable(&mut effects, rect, true);
                        effects
                    } else {
                        Effects::none()
                    }
                }
                State::EnteredEnabled { sent_rect, pending_preedit, pending_commit, composing } => {
                    if sync.set_allowed == Some(false) {
                        *state = State::EnteredDisabled;
                        let mut effects = Effects::two(Effect::Disable, Effect::Commit);
                        effects.push(Effect::Ime(crate::Ime::Disabled));
                        effects
                    } else if sync.cancel || sync.set_allowed == Some(true) {
                        // Commit disable first so the compositor deactivates the input method and
                        // drops its open preedit, then re-enable with content type and cursor rect
                        // in a single commit.
                        let next_rect = rect.or(*sent_rect);
                        *sent_rect = next_rect;
                        *pending_preedit = None;
                        *pending_commit = None;
                        *composing = false;
                        let mut effects = Effects::two(Effect::Disable, Effect::Commit);
                        push_enable(&mut effects, next_rect, false);
                        effects
                    } else if let Some(r) = rect && *sent_rect != Some(r) {
                        *sent_rect = Some(r);
                        Effects::two(Effect::SetCursorRectangle(r), Effect::Commit)
                    } else {
                        Effects::none()
                    }
                }
            }
        }
        Event::Drain => match state {
            State::Outside { desired } => {
                *desired = Desired::Disabled;
                Effects::none()
            }
            State::EnteredDisabled => Effects::none(),
            State::EnteredEnabled { .. } => {
                *state = State::EnteredDisabled;
                Effects::two(Effect::Disable, Effect::Commit)
            }
        },
        Event::Gone => {
            *state = State::Outside { desired: Desired::Disabled };
            Effects::none()
        }
    }
}

#[cfg(test)]
#[path = "text_input_tests.rs"]
mod tests;
