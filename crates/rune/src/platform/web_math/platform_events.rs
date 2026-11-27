//! Parsing and state-change checks for browser platform events (`init`, `viewport`, `motion`,
//! `languages`, `primary_pointer`, `contrast`, `focus`, `visibility`). Kept out of
//! `platform/web/input.rs`, which only compiles on `wasm32`, so the change-detection and startup
//! snapshot rules run in headless Rust tests.
#![cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]

use crate::app::SharedCore;
use crate::runtime::{AppDriver, RuntimeCore};

/// The page's primary pointer capability (`(hover: none)` and `(pointer: coarse)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrimaryPointer {
    Hover,
    NoHover,
    Touch,
}

pub(crate) fn parse_primary_pointer(event: &serde_json::Value) -> PrimaryPointer {
    match (event["hover"].as_bool(), event["coarse"].as_bool()) {
        (Some(false), Some(true)) => PrimaryPointer::Touch,
        (Some(false), _) => PrimaryPointer::NoHover,
        _ => PrimaryPointer::Hover,
    }
}

pub(crate) fn parse_visual_viewport(value: &serde_json::Value) -> Option<[f32; 3]> {
    let v = value.as_array()?;
    let (top, bottom, layout_h) = (
        v.first()?.as_f64()? as f32,
        v.get(1)?.as_f64()? as f32,
        v.get(2)?.as_f64()? as f32,
    );
    (layout_h > 0.0 && bottom > top).then_some([top, bottom, layout_h])
}

/// Parses the 8 browser `forced-colors` system colour RGB triples:
/// Canvas, CanvasText, Highlight, HighlightText, GrayText, ButtonFace, ButtonText, LinkText.
pub(crate) fn parse_contrast(value: &serde_json::Value) -> Option<crate::SystemContrast> {
    let list = value.as_array()?;
    if list.len() != 8 {
        return None;
    }
    let color = |c: &serde_json::Value| {
        let d = |i: usize| crate::color::srgb_decode(c[i].as_f64().unwrap_or(0.0) as f32 / 255.0);
        crate::Color { r: d(0), g: d(1), b: d(2), a: 1.0 }
    };
    Some(crate::SystemContrast {
        window: color(&list[0]),
        text: color(&list[1]),
        highlight: color(&list[2]),
        highlight_text: color(&list[3]),
        disabled: color(&list[4]),
        button: color(&list[5]),
        button_text: color(&list[6]),
        link: color(&list[7]),
    })
}

pub(crate) fn parse_languages(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| list.iter().filter_map(|v| v.as_str()).map(str::to_owned).collect())
        .unwrap_or_default()
}

pub(crate) fn apply_motion(rt: &mut RuntimeCore, event: &serde_json::Value) -> bool {
    rt.set_system_reduced_motion(event["reduced"].as_bool().unwrap_or(false))
}

pub(crate) fn apply_contrast(rt: &mut RuntimeCore, event: &serde_json::Value) -> bool {
    rt.set_system_contrast(parse_contrast(&event["colors"]))
}

pub(crate) fn apply_languages(rt: &mut RuntimeCore, event: &serde_json::Value) -> bool {
    rt.set_languages(parse_languages(&event["languages"]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FocusChange {
    pub focused: bool,
    pub changed: bool,
}

pub(crate) fn apply_focus(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    event: &serde_json::Value,
) -> FocusChange {
    let focused = event["focused"].as_bool().unwrap_or(false);
    let changed = rt.window_focus(app, focused);
    FocusChange { focused, changed }
}

/// What the web host must do after applying a `visibility` (or `init`) visibility payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VisibilityAction {
    /// Visibility and focus already matched runtime state: skip frame and redraw.
    NoOp,
    /// Window stayed visible, and focus transitioned to true: request redraw.
    Redraw,
    /// Window transitioned from visible to hidden: reset input, commit one frame, and rest.
    Hide,
    /// Window transitioned from hidden to visible: settle animations and request redraw.
    Show,
}

pub(crate) fn apply_visibility(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    shared: &SharedCore,
    event: &serde_json::Value,
) -> VisibilityAction {
    let visible = event["visible"].as_bool().unwrap_or(false);
    let focused = event["focused"].as_bool().unwrap_or(false);
    let was_visible = shared.is_visible();
    if visible == was_visible {
        return if visible && focused && rt.window_focus(app, true) {
            VisibilityAction::Redraw
        } else {
            VisibilityAction::NoOp
        };
    }
    shared.set_visible(visible);
    rt.set_live_visible(app, visible);
    if !visible {
        return VisibilityAction::Hide;
    }
    rt.settle_all();
    if focused {
        rt.window_focus(app, true);
    }
    VisibilityAction::Show
}

/// What the web host must do after applying a combined startup `init` snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InitAction {
    /// No startup field changed and the tab stayed visible: skip frame and redraw.
    NoOp,
    /// At least one startup field changed while visible: commit one frame and request a redraw.
    CommitFrame,
    /// The tab started hidden: reset input, commit one frame, and rest.
    CommitHidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InitOutcome {
    pub action: InitAction,
    pub focus: FocusChange,
    pub clear_presented: bool,
}

/// Applies the runtime-side fields of a startup `init` snapshot (`motion`, `contrast`, `focus`,
/// `visibility`) alongside the host's `viewport_changed` and `pointer_changed` results, returning
/// the single host action to take.
pub(crate) fn apply_init(
    rt: &mut RuntimeCore,
    app: &mut dyn AppDriver,
    shared: &SharedCore,
    event: &serde_json::Value,
    viewport_changed: bool,
    pointer_changed: bool,
) -> InitOutcome {
    let mut needs_frame = viewport_changed || pointer_changed;
    needs_frame |= apply_motion(rt, event);
    needs_frame |= apply_contrast(rt, event);
    let focus = apply_focus(rt, app, event);
    needs_frame |= focus.changed;
    let (action, clear_presented) = match apply_visibility(rt, app, shared, event) {
        VisibilityAction::Hide => (InitAction::CommitHidden, true),
        VisibilityAction::Show => (InitAction::CommitFrame, true),
        VisibilityAction::Redraw => (InitAction::CommitFrame, false),
        VisibilityAction::NoOp => (
            if needs_frame { InitAction::CommitFrame } else { InitAction::NoOp },
            false,
        ),
    };
    InitOutcome { action, focus, clear_presented }
}

#[cfg(test)]
#[path = "platform_events_tests.rs"]
mod tests;
