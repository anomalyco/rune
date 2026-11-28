//! Pure state machine for Android's native text-selection overlay (handles, floating
//! `ActionMode` toolbar, `Magnifier`, and 4-second caret-handle timeout).
//!
//! Compiles on every target so handle visibility, toolbar avoidance geometry, drag/magnifier
//! transitions, 4-second caret expiry, and Home/rotation suspend-restore are host-tested.

use crate::PopupMenu;
use crate::machine::{Attempt, Effects, Seq};
use crate::runtime::selection::touch_ui::{
    Handle, HandleAnchor, TouchSelectionKind, TouchSelectionSnapshot,
};

/// Minimum touch target size (in logical dp) of each handle `PopupWindow` (48dp x 48dp), and
/// how far below a visible handle's caret foot the floating toolbar's content rect extends so
/// the toolbar never covers the handles.
pub(crate) const HANDLE_TARGET_DP: f32 = 48.0;

/// Duration (in seconds) the collapsed field caret handle stays visible after a tap.
pub(crate) const CARET_HANDLE_TIMEOUT_SECS: f64 = 4.0;

/// JNI action code passed by `RuneSelectionOverlay` when `ActionMode` is dismissed.
pub(crate) const TOOLBAR_DISMISSED: i32 = 0;
/// JNI action code for `android.R.id.cut`.
pub(crate) const TOOLBAR_CUT: i32 = 1;
/// JNI action code for `android.R.id.copy`.
pub(crate) const TOOLBAR_COPY: i32 = 2;
/// JNI action code for `android.R.id.paste`.
pub(crate) const TOOLBAR_PASTE: i32 = 3;
/// JNI action code for `android.R.id.selectAll`.
pub(crate) const TOOLBAR_SELECT_ALL: i32 = 4;

/// Bitmask of enabled standard actions in Android's floating `ActionMode` toolbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToolbarActions(pub(crate) u32);

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
impl ToolbarActions {
    pub(crate) const NONE: Self = Self(0);
    pub(crate) const CUT: u32 = 1 << 0;
    pub(crate) const COPY: u32 = 1 << 1;
    pub(crate) const PASTE: u32 = 1 << 2;
    pub(crate) const SELECT_ALL: u32 = 1 << 3;

    #[inline]
    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub(crate) const fn bits(self) -> i32 {
        self.0 as i32
    }

    #[inline]
    pub(crate) const fn contains(self, bit: u32) -> bool {
        (self.0 & bit) != 0
    }

    /// Extracts standard Android text toolbar actions from a Rune text `PopupMenu`.
    /// `can_select_all` suppresses Select All when the target field is empty.
    pub(crate) fn from_popup_menu(menu: &PopupMenu, can_select_all: bool) -> Self {
        let mut bits = 0u32;
        if menu.find("cut").is_some_and(|i| i.is_enabled()) {
            bits |= Self::CUT;
        }
        if menu.find("copy").is_some_and(|i| i.is_enabled()) {
            bits |= Self::COPY;
        }
        if menu.find("paste").is_some_and(|i| i.is_enabled()) {
            bits |= Self::PASTE;
        }
        if can_select_all && menu.find("select-all").is_some_and(|i| i.is_enabled()) {
            bits |= Self::SELECT_ALL;
        }
        Self(bits)
    }

    /// Maps a JNI `TOOLBAR_*` action code back to Rune's `PopupItem` id.
    pub(crate) const fn action_id(code: i32) -> Option<&'static str> {
        match code {
            TOOLBAR_CUT => Some("cut"),
            TOOLBAR_COPY => Some("copy"),
            TOOLBAR_PASTE => Some("paste"),
            TOOLBAR_SELECT_ALL => Some("select-all"),
            _ => None,
        }
    }
}

/// Which native selection handle `PopupWindow` is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandleKind {
    Start = 0,
    End = 1,
    Caret = 2,
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
impl HandleKind {
    pub(crate) const fn from_jni(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Start),
            1 => Some(Self::End),
            2 => Some(Self::Caret),
            _ => None,
        }
    }

    pub(crate) const fn to_runtime(self) -> Handle {
        match self {
            Self::Start => Handle::Start,
            Self::End => Handle::End,
            Self::Caret => Handle::Caret,
        }
    }
}

/// Visible handle anchor segment in window points (logical dp).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HandlePose {
    pub x: f32,
    pub top: f32,
    pub bottom: f32,
}

impl HandlePose {
    #[inline]
    pub(crate) fn from_anchor(anchor: HandleAnchor) -> Option<Self> {
        anchor.visible.then_some(Self { x: anchor.x, top: anchor.top, bottom: anchor.bottom })
    }

    #[inline]
    pub(crate) fn mid_y(self) -> f32 {
        (self.top + self.bottom) * 0.5
    }
}

/// Complete desired state of Android's native selection overlay in logical dp.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Presentation {
    pub start: Option<HandlePose>,
    pub end: Option<HandlePose>,
    pub caret: Option<HandlePose>,
    pub magnifier: Option<(f32, f32)>,
    pub toolbar_actions: ToolbarActions,
    /// Content rect `[left, top, right, bottom]` in logical dp that the floating `ActionMode`
    /// avoids covering, extended down to the foot of any visible 48dp handle target.
    pub toolbar_rect: [f32; 4],
}

fn rect_non_empty(r: [f32; 4]) -> bool {
    r[2] > r[0] || r[3] > r[1]
}

/// Builds the floating `ActionMode` avoidance rect: starts with the visible selection box and
/// extends downwards to `bottom + HANDLE_TARGET_DP` for every visible handle so the toolbar never
/// overlaps the 48dp handle targets.
pub(crate) fn toolbar_avoidance_rect(base: [f32; 4], handles: [Option<HandlePose>; 3]) -> [f32; 4] {
    let mut acc = rect_non_empty(base).then_some(base);
    for h in handles.into_iter().flatten() {
        let foot = h.bottom + HANDLE_TARGET_DP;
        acc = Some(match acc {
            None => [h.x, h.top, h.x, foot],
            Some(a) => [a[0].min(h.x), a[1].min(h.top), a[2].max(h.x), a[3].max(foot)],
        });
    }
    acc.unwrap_or([0.0; 4])
}

/// Explicit mode of the Android text-selection overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) enum Mode {
    /// No handles, no magnifier, no toolbar.
    #[default]
    Hidden,
    /// Showing the collapsed field caret handle, optionally with the floating toolbar.
    Caret {
        revision: u64,
        caret: Option<HandlePose>,
        /// Auto-hide deadline in seconds (`None` while the toolbar is open).
        hide_at: Option<f64>,
        attempt: Attempt,
        toolbar: ToolbarActions,
        content_rect: [f32; 4],
    },
    /// Showing a range selection's start/end handles, optionally with the floating toolbar.
    Range {
        start: Option<HandlePose>,
        end: Option<HandlePose>,
        toolbar: ToolbarActions,
        content_rect: [f32; 4],
    },
    /// Actively dragging a handle: toolbar hidden, magnifier tracking the active line.
    Dragging {
        handle: HandleKind,
        reopen_toolbar: bool,
        start: Option<HandlePose>,
        end: Option<HandlePose>,
        caret: Option<HandlePose>,
        magnifier: Option<(f32, f32)>,
        last_pos: (f32, f32),
    },
}

/// Complete host-side selection overlay state machine.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) struct State {
    pub(crate) mode: Mode,
    seq: Seq,
    dismissed_caret_rev: Option<u64>,
    pub(crate) suspended: bool,
    restore_toolbar: bool,
    last_applied: Presentation,
}

impl Default for State {
    fn default() -> Self {
        Self {
            mode: Mode::Hidden,
            seq: Seq::default(),
            // Revision 0 is pre-dismissed so an auto-focused field on launch shows no handle until
            // the user taps it.
            dismissed_caret_rev: Some(0),
            suspended: false,
            restore_toolbar: false,
            last_applied: Presentation::default(),
        }
    }
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
impl State {
    /// Seconds until the caret handle auto-hide timer comes due, if armed and unsuspended.
    pub(crate) fn wake_after_secs(&self, now: f64) -> Option<f64> {
        if self.suspended {
            return None;
        }
        match self.mode {
            Mode::Caret { hide_at: Some(due), .. } => Some((due - now).max(0.0)),
            _ => None,
        }
    }

    /// Computes the desired native overlay presentation from the current mode.
    pub(crate) fn presentation(&self) -> Presentation {
        if self.suspended {
            return Presentation::default();
        }
        match self.mode {
            Mode::Hidden => Presentation::default(),
            Mode::Caret { caret, toolbar, content_rect, .. } => {
                let any_visible = caret.is_some() || rect_non_empty(content_rect);
                let toolbar_actions = if any_visible { toolbar } else { ToolbarActions::NONE };
                let toolbar_rect = if toolbar_actions.is_empty() {
                    [0.0; 4]
                } else {
                    toolbar_avoidance_rect(content_rect, [None, None, caret])
                };
                Presentation {
                    start: None,
                    end: None,
                    caret,
                    magnifier: None,
                    toolbar_actions,
                    toolbar_rect,
                }
            }
            Mode::Range { start, end, toolbar, content_rect } => {
                let any_visible = start.is_some() || end.is_some() || rect_non_empty(content_rect);
                let toolbar_actions = if any_visible { toolbar } else { ToolbarActions::NONE };
                let toolbar_rect = if toolbar_actions.is_empty() {
                    [0.0; 4]
                } else {
                    toolbar_avoidance_rect(content_rect, [start, end, None])
                };
                Presentation {
                    start,
                    end,
                    caret: None,
                    magnifier: None,
                    toolbar_actions,
                    toolbar_rect,
                }
            }
            Mode::Dragging { start, end, caret, magnifier, .. } => Presentation {
                start,
                end,
                caret,
                magnifier,
                toolbar_actions: ToolbarActions::NONE,
                toolbar_rect: [0.0; 4],
            },
        }
    }
}

/// Inputs driving [`step`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) enum Event {
    /// Post-frame synchronization with Rune's live selection geometry and any newly taken text menu.
    Sync {
        now: f64,
        snapshot: Option<TouchSelectionSnapshot>,
        menu: Option<ToolbarActions>,
    },
    /// Clock tick checking whether the 4-second caret handle timeout has elapsed.
    Tick { now: f64 },
    /// Finger grabbed a native handle `PopupWindow` at `(x, y)` in logical dp.
    HandleGrab { handle: HandleKind, x: f32, y: f32 },
    /// Finger dragged the grabbed handle to `(x, y)` in logical dp.
    HandleMove { x: f32, y: f32 },
    /// Finger released (or cancelled) the grabbed handle.
    HandleRelease { now: f64 },
    /// Finger tapped the collapsed caret handle without dragging.
    CaretHandleTapped { now: f64 },
    /// User picked a floating `ActionMode` item (`TOOLBAR_CUT..=TOOLBAR_SELECT_ALL`).
    ToolbarAction { code: i32, now: f64 },
    /// Android dismissed the floating `ActionMode` externally.
    ToolbarDismissed { now: f64 },
    /// Activity paused or surface destroyed.
    Suspend,
    /// Activity resumed with a live surface.
    Resume,
}

/// Declarative side effects returned by [`step`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) enum Effect {
    GrabHandle { handle: HandleKind, x: f32, y: f32 },
    MoveHandle { x: f32, y: f32 },
    ReleaseHandle,
    ResolveTextMenu(Option<&'static str>),
    RequestTextMenu,
    Apply(Presentation),
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn emit_presentation_if_changed(state: &mut State, effects: &mut Effects<Effect>) {
    let next = state.presentation();
    if next != state.last_applied {
        state.last_applied = next;
        effects.push(Effect::Apply(next));
    }
}

/// Pure transition function for the Android selection overlay state machine.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let mut effects = Effects::none();
    match event {
        Event::Suspend => {
            if matches!(
                state.mode,
                Mode::Caret { toolbar, .. } | Mode::Range { toolbar, .. } if !toolbar.is_empty()
            ) {
                state.restore_toolbar = true;
            }
            if let Mode::Dragging { reopen_toolbar, .. } = state.mode {
                if reopen_toolbar {
                    state.restore_toolbar = true;
                }
                state.mode = Mode::Hidden;
                effects.push(Effect::ReleaseHandle);
            }
            state.suspended = true;
            emit_presentation_if_changed(state, &mut effects);
        }
        Event::Resume => {
            state.suspended = false;
            emit_presentation_if_changed(state, &mut effects);
        }
        Event::Tick { now } => {
            if let Mode::Caret { revision, hide_at: Some(due), toolbar, .. } = state.mode
                && toolbar.is_empty()
                && now >= due
            {
                state.dismissed_caret_rev = Some(revision);
                state.mode = Mode::Hidden;
                emit_presentation_if_changed(state, &mut effects);
            }
        }
        Event::HandleGrab { handle, x, y } => {
            let had_toolbar = match state.mode {
                Mode::Caret { toolbar, .. } | Mode::Range { toolbar, .. } => !toolbar.is_empty(),
                Mode::Hidden | Mode::Dragging { .. } => false,
            };
            if had_toolbar {
                effects.push(Effect::ResolveTextMenu(None));
            }
            let (start, end, caret, mag) = match state.mode {
                Mode::Range { start, end, .. } => {
                    let active = match handle {
                        HandleKind::Start => start,
                        HandleKind::End => end,
                        HandleKind::Caret => None,
                    };
                    (start, end, None, active.map(|p| (p.x, p.mid_y())).or(Some((x, y))))
                }
                Mode::Caret { caret, .. } => {
                    (None, None, caret, caret.map(|p| (p.x, p.mid_y())).or(Some((x, y))))
                }
                Mode::Dragging { start, end, caret, magnifier, .. } => {
                    (start, end, caret, magnifier.or(Some((x, y))))
                }
                Mode::Hidden => (None, None, None, Some((x, y))),
            };
            state.restore_toolbar = false;
            state.mode = Mode::Dragging {
                handle,
                reopen_toolbar: handle != HandleKind::Caret,
                start,
                end,
                caret,
                magnifier: mag,
                last_pos: (x, y),
            };
            effects.push(Effect::GrabHandle { handle, x, y });
            emit_presentation_if_changed(state, &mut effects);
        }
        Event::HandleMove { x, y } => {
            if let Mode::Dragging { last_pos, .. } = &mut state.mode {
                *last_pos = (x, y);
                effects.push(Effect::MoveHandle { x, y });
            }
        }
        Event::HandleRelease { now } => {
            let Mode::Dragging { handle, reopen_toolbar, start, end, caret, .. } = state.mode else {
                return effects;
            };
            effects.push(Effect::ReleaseHandle);
            match handle {
                HandleKind::Caret => {
                    let attempt = state.seq.next();
                    let rect = caret.map_or([0.0; 4], |p| [p.x, p.top, p.x, p.bottom]);
                    state.mode = Mode::Caret {
                        revision: state.dismissed_caret_rev.map_or(1, |r| r.wrapping_add(1)),
                        caret,
                        hide_at: Some(now + CARET_HANDLE_TIMEOUT_SECS),
                        attempt,
                        toolbar: ToolbarActions::NONE,
                        content_rect: rect,
                    };
                }
                HandleKind::Start | HandleKind::End => {
                    let rect = toolbar_avoidance_rect([0.0; 4], [start, end, None]);
                    state.mode = Mode::Range {
                        start,
                        end,
                        toolbar: ToolbarActions::NONE,
                        content_rect: rect,
                    };
                    if reopen_toolbar {
                        state.restore_toolbar = true;
                        effects.push(Effect::RequestTextMenu);
                    }
                }
            }
            emit_presentation_if_changed(state, &mut effects);
        }
        Event::CaretHandleTapped { now } => {
            if let Mode::Caret { toolbar, hide_at, attempt, .. } = &mut state.mode {
                if toolbar.is_empty() {
                    effects.push(Effect::RequestTextMenu);
                } else {
                    *toolbar = ToolbarActions::NONE;
                    *hide_at = Some(now + CARET_HANDLE_TIMEOUT_SECS);
                    *attempt = state.seq.next();
                    effects.push(Effect::ResolveTextMenu(None));
                    emit_presentation_if_changed(state, &mut effects);
                }
            }
        }
        Event::ToolbarAction { code, now } => {
            if let Some(id) = ToolbarActions::action_id(code) {
                match &mut state.mode {
                    Mode::Caret { toolbar, hide_at, attempt, .. } => {
                        *toolbar = ToolbarActions::NONE;
                        *hide_at = Some(now + CARET_HANDLE_TIMEOUT_SECS);
                        *attempt = state.seq.next();
                    }
                    Mode::Range { toolbar, .. } => {
                        *toolbar = ToolbarActions::NONE;
                    }
                    Mode::Hidden | Mode::Dragging { .. } => {}
                }
                if code == TOOLBAR_SELECT_ALL {
                    state.restore_toolbar = true;
                }
                effects.push(Effect::ResolveTextMenu(Some(id)));
                emit_presentation_if_changed(state, &mut effects);
            } else {
                return step(state, Event::ToolbarDismissed { now });
            }
        }
        Event::ToolbarDismissed { now } => {
            let had_toolbar = match &mut state.mode {
                Mode::Caret { toolbar, hide_at, attempt, .. } => {
                    let was = !toolbar.is_empty();
                    *toolbar = ToolbarActions::NONE;
                    if was {
                        *hide_at = Some(now + CARET_HANDLE_TIMEOUT_SECS);
                        *attempt = state.seq.next();
                    }
                    was
                }
                Mode::Range { toolbar, .. } => {
                    let was = !toolbar.is_empty();
                    *toolbar = ToolbarActions::NONE;
                    was
                }
                Mode::Hidden | Mode::Dragging { .. } => false,
            };
            state.restore_toolbar = false;
            if had_toolbar {
                effects.push(Effect::ResolveTextMenu(None));
            }
            emit_presentation_if_changed(state, &mut effects);
        }
        Event::Sync { now, snapshot, menu } => {
            let Some(snap) = snapshot else {
                let had_toolbar = matches!(
                    state.mode,
                    Mode::Caret { toolbar, .. } | Mode::Range { toolbar, .. } if !toolbar.is_empty()
                );
                if matches!(state.mode, Mode::Dragging { .. }) {
                    effects.push(Effect::ReleaseHandle);
                }
                if had_toolbar || menu.is_some() {
                    effects.push(Effect::ResolveTextMenu(None));
                }
                state.restore_toolbar = false;
                state.mode = Mode::Hidden;
                emit_presentation_if_changed(state, &mut effects);
                return effects;
            };

            let menu_actions = menu.filter(|m| !m.is_empty());

            // While dragging a handle, update handle poses and magnifier position; keep toolbar hidden.
            if let Mode::Dragging { handle, start, end, caret, magnifier, last_pos, .. } = &mut state.mode {
                match snap.kind {
                    TouchSelectionKind::FieldCaret => {
                        *start = None;
                        *end = None;
                        *caret = HandlePose::from_anchor(snap.start);
                        *magnifier = caret.map(|p| (p.x, p.mid_y())).or(Some(*last_pos));
                    }
                    TouchSelectionKind::FieldRange | TouchSelectionKind::SelectableRange => {
                        *start = HandlePose::from_anchor(snap.start);
                        *end = snap.end.and_then(HandlePose::from_anchor);
                        *caret = None;
                        let active = match *handle {
                            HandleKind::Start => *start,
                            HandleKind::End => *end,
                            HandleKind::Caret => *start,
                        };
                        *magnifier = active.map(|p| (p.x, p.mid_y())).or(Some(*last_pos));
                    }
                }
                emit_presentation_if_changed(state, &mut effects);
                return effects;
            }

            match snap.kind {
                TouchSelectionKind::FieldCaret => {
                    let caret = HandlePose::from_anchor(snap.start);
                    if let Some(actions) = menu_actions {
                        state.restore_toolbar = false;
                        state.dismissed_caret_rev = None;
                        let attempt = state.seq.next();
                        state.mode = Mode::Caret {
                            revision: snap.revision,
                            caret,
                            hide_at: None,
                            attempt,
                            toolbar: actions,
                            content_rect: snap.content_rect,
                        };
                    } else if let Mode::Caret { revision, hide_at, attempt, toolbar, .. } = state.mode {
                        if revision != snap.revision {
                            state.dismissed_caret_rev = None;
                            let next_attempt = state.seq.next();
                            state.mode = Mode::Caret {
                                revision: snap.revision,
                                caret,
                                hide_at: Some(now + CARET_HANDLE_TIMEOUT_SECS),
                                attempt: next_attempt,
                                toolbar: ToolbarActions::NONE,
                                content_rect: snap.content_rect,
                            };
                        } else if toolbar.is_empty() && hide_at.is_some_and(|due| now >= due) {
                            state.dismissed_caret_rev = Some(revision);
                            state.mode = Mode::Hidden;
                        } else {
                            state.mode = Mode::Caret {
                                revision,
                                caret,
                                hide_at,
                                attempt,
                                toolbar,
                                content_rect: snap.content_rect,
                            };
                        }
                    } else if state.dismissed_caret_rev != Some(snap.revision) {
                        let attempt = state.seq.next();
                        state.mode = Mode::Caret {
                            revision: snap.revision,
                            caret,
                            hide_at: Some(now + CARET_HANDLE_TIMEOUT_SECS),
                            attempt,
                            toolbar: ToolbarActions::NONE,
                            content_rect: snap.content_rect,
                        };
                    } else {
                        state.mode = Mode::Hidden;
                    }
                    if state.restore_toolbar && !state.suspended {
                        state.restore_toolbar = false;
                        effects.push(Effect::RequestTextMenu);
                    }
                }
                TouchSelectionKind::FieldRange | TouchSelectionKind::SelectableRange => {
                    let start = HandlePose::from_anchor(snap.start);
                    let end = snap.end.and_then(HandlePose::from_anchor);
                    let prev_toolbar = match state.mode {
                        Mode::Range { toolbar, .. } => toolbar,
                        _ => ToolbarActions::NONE,
                    };
                    let toolbar = menu_actions.unwrap_or(prev_toolbar);
                    state.mode = Mode::Range {
                        start,
                        end,
                        toolbar,
                        content_rect: snap.content_rect,
                    };
                    if menu_actions.is_some() {
                        state.restore_toolbar = false;
                    } else if state.restore_toolbar && !state.suspended {
                        state.restore_toolbar = false;
                        effects.push(Effect::RequestTextMenu);
                    }
                }
            }
            emit_presentation_if_changed(state, &mut effects);
        }
    }
    effects
}
