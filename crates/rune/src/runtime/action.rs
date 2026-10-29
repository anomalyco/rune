//! The actions input and layout queue for the next frame's handlers, and how a frame runs them.

use super::*;
use crate::time::Instant;

pub(super) enum Action {
    WindowFocus(bool),
    Physical(crate::PhysicalInput, Option<physical_dispatch::Fallback>),
    FocusControl(String),
    /// The pointer entered a `focus_highlight` row (`Some`) or left every such row.
    HoverFocus(Option<String>),
    Dismiss(String),
    PointerMove(String, PointerEvent),
    Hover(String),
    HoverChange(String, bool),
    /// `El::on_selection_change`.
    SelectionChange(String, crate::SelectionChange),
    AtBottom(String, bool),
    FarFromBottom(String, bool),
    VisibleRows(String, std::ops::Range<usize>),
    Layout(String, [f32; 4]),
    ScreenLayout(String, [f32; 4]),
    /// `El::on_drawn_layout`; true when only a live value, scroll or animation moved it.
    DrawnLayout(String, [f32; 4], bool),
    Press(String, crate::PointerButton),
    DoublePress(String),
    Link(String, usize),
    /// `El::on_link_secondary`: the text, its link and the press.
    LinkSecondary(String, usize, PointerEvent),
    SecondaryPress(String, PointerEvent),
    Drag(String, Drag, crate::PointerButton),
    DragStart(String),
    DragEnd(String, crate::DragOutcome),
    /// A file drag from another app moved, left, or dropped (`drop_dispatch`).
    Files(drop_dispatch::FileEvent),
    Key { ev: KeyEvent, repeat: bool },
    KeyUp(KeyEvent),
    /// A wheel delta at window point (x, y) for the `on_wheel` handlers at the pointer,
    /// innermost first (`gesture_dispatch.rs`).
    Wheel(u64, Vec<String>, crate::WheelEvent),
    Gesture(Vec<String>, gesture_dispatch::Gesture),
    Ime(Ime),
    Paste(crate::ClipboardContent, Option<Option<String>>),
    /// A `Tx::read_clipboard` callback's id and what the clipboard held.
    ClipboardRead(u64, crate::ClipboardContent),
    /// A `Tx::open_file` or `Tx::save_file` callback's id and the dialog's answer.
    FileDialog(u64, crate::file_dialog::FileDialogResult),
    Focus(String, f32, f32),
    Blur,
}

/// A queued action and when the input that queued it arrived (`Tx::input_time`): the arrival
/// time its host dispatched the input under (`input_time::scope`), none for what layout,
/// timers, updates and polls queue.
pub(super) struct Queued {
    pub(super) action: Action,
    pub(super) at: Option<Instant>,
}

/// The actions waiting for the next frame, in order, each with its input's arrival time.
#[derive(Default)]
pub(super) struct ActionQueue(Vec<Queued>);

impl ActionQueue {
    /// Queues `action` stamped with the input being dispatched now, if any.
    pub(super) fn push(&mut self, action: Action) { self.0.push(Queued { action, at: crate::input_time::current() }); }
    /// Queues an action again with its own stamp.
    pub(super) fn push_queued(&mut self, queued: Queued) { self.0.push(queued); }
    /// Puts `queued` ahead of everything queued.
    pub(super) fn prepend(&mut self, queued: Vec<Queued>) { self.0.splice(0..0, queued); }
    pub(super) fn iter(&self) -> impl Iterator<Item = &Action> { self.0.iter().map(|q| &q.action) }
    pub(super) fn len(&self) -> usize { self.0.len() }
    pub(super) fn is_empty(&self) -> bool { self.0.is_empty() }
    pub(super) fn retain(&mut self, mut keep: impl FnMut(&Action) -> bool) { self.0.retain(|q| keep(&q.action)); }
    pub(super) fn remove(&mut self, index: usize) -> Action { self.0.remove(index).action }
}

impl Extend<Action> for ActionQueue {
    fn extend<I: IntoIterator<Item = Action>>(&mut self, actions: I) {
        for action in actions { self.push(action); }
    }
}

impl Extend<Queued> for ActionQueue {
    fn extend<I: IntoIterator<Item = Queued>>(&mut self, queued: I) { self.0.extend(queued); }
}

impl FromIterator<Queued> for ActionQueue {
    fn from_iter<I: IntoIterator<Item = Queued>>(queued: I) -> Self { Self(queued.into_iter().collect()) }
}

impl IntoIterator for ActionQueue {
    type Item = Queued;
    type IntoIter = std::vec::IntoIter<Queued>;
    fn into_iter(self) -> Self::IntoIter { self.0.into_iter() }
}

/// The handler an event goes to, when its `Tx::live_only` can keep the event from committing.
/// Events Rune does its own work for (focus, editing, pastes, file drops) always commit.
pub(super) fn live_only_handler(action: &Action) -> Option<&'static str> {
    Some(match action {
        Action::WindowFocus(_) => "on_window_focus",
        Action::DrawnLayout(..) => "on_drawn_layout_tx",
        Action::PointerMove(..) => "on_pointer_move_local",
        Action::Hover(_) => "on_hover",
        Action::HoverChange(..) => "on_hover_change",
        Action::SelectionChange(..) => "on_selection_change",
        Action::Press(..) => "on_press",
        Action::DoublePress(_) => "on_double_press",
        Action::Link(..) => "on_link",
        Action::LinkSecondary(..) => "on_link_secondary",
        Action::SecondaryPress(..) => "on_secondary_press",
        Action::Drag(..) => "on_drag",
        Action::DragStart(_) => "on_drag_start",
        Action::DragEnd(..) => "on_drag_end",
        Action::Key { .. } => "on_key",
        Action::KeyUp(_) => "on_key_up",
        Action::Wheel(..) => "on_wheel",
        Action::Gesture(_, gesture_dispatch::Gesture::Magnify(_)) => "on_magnify",
        Action::Gesture(_, gesture_dispatch::Gesture::Rotate(_)) => "on_rotate",
        Action::Gesture(_, gesture_dispatch::Gesture::SmartMagnify(..)) => "on_smart_magnify",
        _ => return None,
    })
}

#[cfg(debug_assertions)]
pub(super) fn action_path(action: &Action) -> Option<&str> {
    match action {
        Action::PointerMove(path, _) | Action::DrawnLayout(path, ..) | Action::Hover(path) | Action::HoverChange(path, _) | Action::SelectionChange(path, _) | Action::Press(path, _) | Action::DoublePress(path)
        | Action::Link(path, _) | Action::LinkSecondary(path, ..) | Action::SecondaryPress(path, _) | Action::Drag(path, ..) | Action::DragStart(path)
        | Action::DragEnd(path, _) => Some(path),
        Action::Wheel(_, chain, _) | Action::Gesture(chain, _) => chain.first().map(String::as_str),
        _ => None,
    }
}

pub(super) fn merge(tx: &mut Tx, one: Tx) {
    tx.pick_reads.extend(one.pick_reads);
    tx.paste |= one.paste;
    tx.clipboard_reads.extend(one.clipboard_reads);
    if let Some(one) = one.trace {
        let trace = tx.trace.get_or_insert_with(Default::default);
        trace.events.extend(one.events);
        trace.channels.extend(one.channels);
    }
    tx.scroll_to.extend(one.scroll_to);
    tx.scroll_bottom.extend(one.scroll_bottom);
    tx.scroll_pages.extend(one.scroll_pages);
    tx.scroll_offsets.extend(one.scroll_offsets);
    tx.keep_on_screen.extend(one.keep_on_screen);
    tx.wakes.extend(one.wakes);
    tx.text_flights.extend(one.text_flights);
    tx.launches.extend(one.launches);
    tx.popups.extend(one.popups);
    tx.file_dialogs.extend(one.file_dialogs);
    if let Some(request) = one.window_frame { tx.window_frame = Some(tx.window_frame.unwrap_or_default().merge(request)); }
    if one.clear_secret && tx.copy.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { tx.copy = None; }
    tx.clear_secret |= one.clear_secret;
    if one.copy.is_some() { tx.copy = one.copy; }
    tx.shell = tx.shell.max(one.shell);
    tx.minimize |= one.minimize;
    if one.title.is_some() { tx.title = one.title; }
    tx.playground.extend(one.playground);
    tx.open_urls.extend(one.open_urls);
    tx.srgb_blend = one.srgb_blend.or(tx.srgb_blend);
    tx.font_smoothing = one.font_smoothing.or(tx.font_smoothing);
    tx.blur |= one.blur;
    if one.select.is_some() {
        tx.select = one.select;
        tx.select_from_change = one.select_from_change;
    }
    tx.instant |= one.instant;
    tx.settle |= one.settle;
    tx.follow_edge |= one.follow_edge;
    if one.motion.is_some() {
        tx.motion = one.motion;
        tx.motion_curve = one.motion_curve;
    }
    tx.color_motion = one.color_motion.or(tx.color_motion);
}

/// Slots the ends hosts reported since the last frame into this frame's actions. One OS drag
/// runs at a time and hosts report ends in order, so each belongs to the oldest open drag: it
/// runs before any later drag's start, and after its own start when both are queued.
pub(super) fn order_drag_ends(queue: ActionQueue, ended: Vec<(String, crate::DragOutcome)>, open: &mut usize) -> ActionQueue {
    let mut ended = ended.into_iter().map(|(path, outcome)| Queued { action: Action::DragEnd(path, outcome), at: None });
    let mut out = Vec::with_capacity(queue.len() + ended.len());
    for queued in queue {
        match &queued.action {
            Action::DragStart(_) => {
                while *open > 0 && let Some(end) = ended.next() {
                    out.push(end);
                    *open -= 1;
                }
                *open += 1;
            }
            Action::DragEnd(..) => *open = open.saturating_sub(1),
            _ => {}
        }
        out.push(queued);
    }
    for end in ended {
        *open = open.saturating_sub(1);
        out.push(end);
    }
    ActionQueue(out)
}
