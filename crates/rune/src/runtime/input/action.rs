//! The actions input and layout queue for the next frame's handlers, and how a frame runs them.

use super::*;
use crate::time::Instant;

pub(in crate::runtime) enum Action {
    WindowFocus(bool),
    Physical(crate::PhysicalInput, Option<physical_dispatch::Fallback>),
    FocusControl(String),
    /// The pointer entered a `focus_highlight` row (`Some`) or left every such row.
    HoverFocus(Option<String>),
    Dismiss(String),
    PointerMove(String, PointerEvent),
    /// `El::pointer_lock`: coalesced relative mouse delta `[dx, dy]` in points.
    PointerLockMotion(String, [f32; 2]),
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
    /// `El::on_visible`: the element came on screen or left it.
    Visible(String, bool),
    Press(String, crate::PointerButton, bool, Option<PointerEvent>), // true for a touch tap
    DoublePress(String, Option<PointerEvent>),
    /// `El::on_middle_press`.
    MiddlePress(String, Option<PointerEvent>),
    /// `El::on_any_press`: passive window-wide mouse button-down observer.
    AnyPress(String, crate::PointerButton, PointerEvent),
    /// A text's link and the button that clicked it (the primary or the middle).
    Link(String, usize, crate::PointerButton),
    /// `El::on_link_secondary`: the text, its link and the press.
    LinkSecondary(String, usize, PointerEvent),
    /// `El::on_link_hover`: the text, hovered link id (or `None` on leave), hit line box
    /// `[left, top, width, height]` in window points, and whether any mouse button was held.
    LinkHover(String, Option<String>, Option<[f32; 4]>, bool),
    SecondaryPress(String, PointerEvent),
    /// A same-frame outside secondary press that dismissed a menu modal on Windows and waits for
    /// the dismissal to commit and rebuild hit-testing before resolving what lies under it.
    SecondaryRepost { modal: String, outer: Option<String>, down: (f32, f32), up: (f32, f32) },
    Drag(String, Drag, crate::PointerButton),
    /// A held press's drag hearing the pointer again (`Tx::drag_repeat_after`).
    DragRepeat(String, Drag, crate::PointerButton),
    DragStart(String),
    DragEnd(String, crate::DragOutcome),
    /// A phase of an `El::on_edge_drag` swipe (`runtime/input/edge_drag.rs`).
    EdgeDrag(String, Drag),
    /// A file drag from another app moved, left, or dropped (`drop_dispatch`).
    Files(drop_dispatch::FileEvent),
    /// `stage`: a playground step pressed it, for the stage (`Play::press`).
    Key { ev: KeyEvent, repeat: bool, stage: bool },
    KeyUp(KeyEvent),
    /// A wheel delta at window point (x, y) for the `on_wheel` handlers at the pointer,
    /// innermost first, with the gesture's own axis policy: whether the platform reported its
    /// phases (`runtime/input/gesture.rs`).
    Wheel { gesture: crate::machine::Attempt, chain: Option<Vec<String>>, ev: crate::WheelEvent, phased: bool },
    /// An outside wheel that dismissed a modal with `El::modal_wheel_outside` and waits for
    /// the dismissal to commit and rebuild hit-testing before routing to what lies under it.
    WheelRepost(WheelRepost),
    Gesture(Vec<String>, gesture_dispatch::Gesture),
    Ime(Ime),
    /// The window went inactive: an open composition becomes text (`Runtime::finish_composition`).
    FinishIme,
    Paste(crate::ClipboardContent, Option<Option<String>>),
    /// A `Tx::read_clipboard` callback's id and what the clipboard held.
    ClipboardRead(u64, crate::ClipboardContent),
    /// A `Tx::copy_image_then` callback's id and what came of the copy.
    CopyImageDone(u64, Result<(), crate::CopyImageError>),
    /// A `Tx::open_file`, `Tx::open_files`, `Tx::save_file` or `Tx::open_folder` callback's id
    /// and the dialog's answer.
    FileDialog(u64, crate::file_dialog::DialogOutcome),
    /// A `Tx::open_url_then` callback and what came of the open.
    UrlOpened(crate::open_url::OpenedFn, Result<(), crate::open_url::OpenUrlError>),
    Focus(String, f32, f32),
    /// A text field's menu: the field, the window point it opens at, and the window point where a
    /// press asked for it (none from the keyboard).
    FieldMenu(String, (f32, f32), Option<(f32, f32)>),
    Blur,
    /// What assistive technology asked of an element (`a11y/actions.rs`).
    A11y(String, crate::a11y::actions::A11yAct),
}

/// An outside wheel waiting for a modal's dismissal to commit before routing to what lies under it.
pub(in crate::runtime) struct WheelRepost {
    pub(in crate::runtime) modal: String,
    pub(in crate::runtime) outer: Option<String>,
    pub(in crate::runtime) gesture: crate::machine::Attempt,
    pub(in crate::runtime) ev: crate::WheelEvent,
    pub(in crate::runtime) phased: bool,
}

/// A queued action and when the input that queued it arrived (`Tx::input_time`): the arrival
/// time its host dispatched the input under (`input_time::scope`), none for what layout,
/// timers, updates and polls queue.
pub(in crate::runtime) struct Queued {
    pub(in crate::runtime) action: Action,
    pub(in crate::runtime) at: Option<Instant>,
    pub(in crate::runtime) input: bool,
}

/// The actions waiting for the next frame, in order, each with its input's arrival time.
#[derive(Default)]
pub(in crate::runtime) struct ActionQueue(Vec<Queued>);

impl ActionQueue {
    /// Queues `action` stamped with the input being dispatched now, if any.
    pub(in crate::runtime) fn push(&mut self, action: Action) {
        // A host can dispatch input without a timestamp. Layout and state observers, on the
        // other hand, don't become input just because they have a queued callback.
        let at = crate::input::time::current();
        let input = !matches!(action, Action::Layout(..) | Action::ScreenLayout(..) | Action::DrawnLayout(..)
            | Action::Visible(..) | Action::VisibleRows(..) | Action::AtBottom(..) | Action::FarFromBottom(..)
            | Action::WindowFocus(_) | Action::Physical(crate::PhysicalInput::FocusLost, _) | Action::FinishIme
            | Action::ClipboardRead(..) | Action::CopyImageDone(..) | Action::UrlOpened(..) | Action::SelectionChange(..)) || at.is_some();
        self.0.push(Queued { action, at, input });
    }
    /// A stationary pointer was re-hit after layout; no pointer event asked for that hover.
    pub(in crate::runtime) fn observe_since(&mut self, start: usize) {
        for q in &mut self.0[start..] { q.input = false; }
    }
    /// Queues an action again with its own stamp.
    pub(in crate::runtime) fn push_queued(&mut self, queued: Queued) { self.0.push(queued); }
    /// Puts `queued` ahead of everything queued.
    pub(in crate::runtime) fn prepend(&mut self, queued: Vec<Queued>) { self.0.splice(0..0, queued); }
    pub(in crate::runtime) fn iter(&self) -> impl Iterator<Item = &Action> { self.0.iter().map(|q| &q.action) }
    pub(in crate::runtime) fn last_mut(&mut self) -> Option<&mut Action> { self.0.last_mut().map(|q| &mut q.action) }
    /// Whether any of them is the user's input, as the commit's `INPUT` cause counts it: a host
    /// (or headless `Runtime`) may dispatch input without an arrival timestamp.
    pub(in crate::runtime) fn has_input(&self) -> bool { self.0.iter().any(|q| q.input) }
    pub(in crate::runtime) fn len(&self) -> usize { self.0.len() }
    pub(in crate::runtime) fn is_empty(&self) -> bool { self.0.is_empty() }
    pub(in crate::runtime) fn retain(&mut self, mut keep: impl FnMut(&Action) -> bool) { self.0.retain(|q| keep(&q.action)); }
    pub(in crate::runtime) fn remove(&mut self, index: usize) -> Action { self.0.remove(index).action }
    pub(in crate::runtime) fn split_off(&mut self, at: usize) -> Vec<Queued> { self.0.split_off(at) }

    /// Records a same-frame middle-button release on the latest pending `Gesture::Button` so
    /// deferred autoscroll fallback sees the release point if every `on_button` handler declines.
    pub(in crate::runtime) fn note_middle_button_up(&mut self, x: f32, y: f32) {
        for q in self.0.iter_mut().rev() {
            if let Action::Gesture(_, gesture_dispatch::Gesture::Button { button: crate::PointerButton::Middle, released, .. }) = &mut q.action
                && released.is_none()
            {
                *released = Some((x, y));
                return;
            }
        }
    }
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
pub(in crate::runtime) fn live_only_handler(action: &Action) -> Option<&'static str> {
    Some(match action {
        Action::WindowFocus(_) => "on_window_focus",
        Action::DrawnLayout(..) => "on_drawn_layout_tx",
        Action::Visible(..) => "on_visible",
        Action::Layout(..) => "on_layout_tx",
        Action::PointerMove(..) => "on_pointer_move_local",
        Action::PointerLockMotion(..) => "pointer_lock",
        Action::Hover(_) => "on_hover",
        Action::HoverChange(..) => "on_hover_change",
        Action::SelectionChange(..) => "on_selection_change",
        Action::Press(..) => "on_press",
        Action::DoublePress(..) => "on_double_press",
        Action::MiddlePress(..) => "on_middle_press",
        Action::AnyPress(..) => "on_any_press",
        Action::Link(..) => "on_link",
        Action::LinkSecondary(..) => "on_link_secondary",
        Action::LinkHover(..) => "on_link_hover",
        Action::SecondaryPress(..) => "on_secondary_press",
        Action::Drag(..) | Action::DragRepeat(..) => "on_drag",
        Action::DragStart(_) => "on_drag_start",
        Action::DragEnd(..) => "on_drag_end",
        Action::EdgeDrag(..) => "on_edge_drag",
        Action::Key { .. } => "on_key",
        Action::KeyUp(_) => "on_key_up",
        Action::Wheel { .. } | Action::WheelRepost(_)
        | Action::Gesture(_, gesture_dispatch::Gesture::TouchWheel(_)) => "on_wheel",
        Action::Gesture(_, gesture_dispatch::Gesture::Magnify(_)) => "on_magnify",
        Action::Gesture(_, gesture_dispatch::Gesture::Rotate(_)) => "on_rotate",
        Action::Gesture(_, gesture_dispatch::Gesture::SmartMagnify(..)) => "on_smart_magnify",
        Action::Gesture(_, gesture_dispatch::Gesture::Button { .. }) => "on_button",
        Action::A11y(_, crate::a11y::actions::A11yAct::Delegate { .. }) => "on_a11y_action",
        _ => return None,
    })
}

#[cfg(debug_assertions)]
pub(in crate::runtime) fn action_path(action: &Action) -> Option<&str> {
    match action {
        Action::PointerMove(path, _) | Action::PointerLockMotion(path, _) | Action::DrawnLayout(path, ..) | Action::Layout(path, _) | Action::Hover(path) | Action::HoverChange(path, _) | Action::SelectionChange(path, _) | Action::Press(path, ..) | Action::DoublePress(path, ..) | Action::MiddlePress(path, ..) | Action::AnyPress(path, ..)
        | Action::Link(path, ..) | Action::LinkSecondary(path, ..) | Action::LinkHover(path, ..) | Action::SecondaryPress(path, _) | Action::Drag(path, ..) | Action::DragRepeat(path, ..) | Action::DragStart(path)
        | Action::DragEnd(path, _) | Action::EdgeDrag(path, _) | Action::Visible(path, _) => Some(path),
        Action::Wheel { chain, .. } => chain.as_deref().and_then(|c| c.first()).map(String::as_str),
        Action::Gesture(chain, _) => chain.first().map(String::as_str),
        _ => None,
    }
}

/// The handler an action runs, for the stall warning.
#[cfg(debug_assertions)]
fn handler_name(action: &Action) -> &'static str {
    live_only_handler(action).unwrap_or(match action {
        Action::Physical(..) => "on_key",
        Action::FocusControl(_) | Action::HoverFocus(_) | Action::Focus(..) | Action::Blur => "a focus change",
        Action::Dismiss(_) => "a modal's dismiss",
        Action::SecondaryRepost { .. } => "a modal's secondary repost",
        Action::AtBottom(..) => "on_at_bottom",
        Action::FarFromBottom(..) => "on_far_from_bottom",
        Action::VisibleRows(..) => "on_visible_rows",
        Action::ScreenLayout(..) => "on_screen_layout",
        Action::Files(_) => "on_drop_files",
        Action::Ime(_) | Action::FinishIme => "a text input's edit",
        Action::Paste(..) => "on_paste",
        Action::ClipboardRead(..) => "a `read_clipboard` callback",
        Action::CopyImageDone(..) => "a `copy_image_then` callback",
        Action::FileDialog(..) => "a file dialog callback",
        Action::UrlOpened(..) => "an `open_url_then` callback",
        _ => "a handler",
    })
}

#[cfg(debug_assertions)]
fn stall_path(action: &Action) -> Option<&str> {
    match action {
        Action::Dismiss(path) | Action::AtBottom(path, _) | Action::FarFromBottom(path, _) | Action::VisibleRows(path, _)
        | Action::Layout(path, _) | Action::ScreenLayout(path, _) | Action::Focus(path, ..) | Action::FocusControl(path) => Some(path),
        _ => action_path(action),
    }
}

pub(crate) fn merge(tx: &mut Tx, one: Tx) {
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
    tx.scroll_stops.extend(one.scroll_stops);
    tx.keep_on_screen.extend(one.keep_on_screen);
    tx.evict_memos.extend(one.evict_memos);
    tx.wakes.extend(one.wakes);
    tx.text_flights.extend(one.text_flights);
    tx.launches.extend(one.launches);
    tx.unclipped_flights |= one.unclipped_flights;
    tx.popups.extend(one.popups);
    if one.field_menu_opened {
        tx.field_menu_opened = true;
        tx.field_menu_resolution = one.field_menu_resolution;
    } else if one.field_menu_resolution.is_some() {
        tx.field_menu_resolution = one.field_menu_resolution;
    }
    tx.file_dialogs.extend(one.file_dialogs);
    if let Some(request) = one.window_frame { tx.window_frame = Some(tx.window_frame.unwrap_or_default().merge(request)); }
    if one.clear_secret && tx.copy.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { tx.copy = None; }
    tx.clear_secret |= one.clear_secret;
    tx.superseded_copy_images.extend(one.superseded_copy_images);
    if one.copy.is_some() {
        if let Some(old) = tx.copy_image_callback.take() { tx.superseded_copy_images.push(old); }
        tx.copy = one.copy;
        tx.copy_image_callback = one.copy_image_callback;
    }
    tx.shell = tx.shell.max(one.shell);
    tx.user_close_request |= one.user_close_request;
    tx.minimize_order = tx.minimize_order.max(one.minimize_order);
    tx.toggle_maximized |= one.toggle_maximized;
    if one.full_screen.is_some() { tx.full_screen = one.full_screen; }
    tx.keys.extend(one.keys);
    if one.title.is_some() { tx.title = one.title; }
    if one.color_scheme.is_some() { tx.color_scheme = one.color_scheme; }
    if one.window_backdrop.is_some() { tx.window_backdrop = one.window_backdrop; }
    if one.represented_file.is_some() { tx.represented_file = one.represented_file; }
    if one.document_edited.is_some() { tx.document_edited = one.document_edited; }
    if one.announce.is_some() { tx.announce = one.announce; }
    tx.playground.extend(one.playground);
    tx.open_urls.extend(one.open_urls);
    tx.haptics.extend(one.haptics);
    tx.open_paths.extend(one.open_paths);
    tx.url_schemes.extend(one.url_schemes);
    tx.srgb_blend = one.srgb_blend.or(tx.srgb_blend);
    tx.font_smoothing = one.font_smoothing.or(tx.font_smoothing);
    tx.fonts_added |= one.fonts_added;
    if one.font_families.is_some() { tx.font_families = one.font_families; }
    if one.emoji_family.is_some() { tx.emoji_family = one.emoji_family; }
    if one.rewind.is_some() { tx.rewind = one.rewind; }
    tx.blur |= one.blur;
    tx.cancel_pointer |= one.cancel_pointer;
    if one.focus_to.is_some() { tx.focus_to = one.focus_to; }
    if one.select.is_some() {
        tx.select = one.select;
        tx.select_from_change = one.select_from_change;
    }
    tx.commit = tx.commit.merge(one.commit);
    tx.follow_edge |= one.follow_edge;
    tx.asked_motion |= one.asked_motion;
    for (stack, ink) in one.live_growth { tx.request_live_growth(stack, ink); }
    tx.tail_rooms.extend(one.tail_rooms);
    tx.motion = one.motion.or(tx.motion);
    tx.color_motion = one.color_motion.or(tx.color_motion);
}

/// Slots the ends hosts reported since the last frame into this frame's actions. One OS drag
/// runs at a time and hosts report ends in order, so each belongs to the oldest open drag: it
/// runs before any later drag's start, and after its own start when both are queued.
pub(in crate::runtime) fn order_drag_ends(queue: ActionQueue, ended: Vec<(String, crate::DragOutcome)>, open: &mut usize) -> ActionQueue {
    let mut ended = ended.into_iter().map(|(path, outcome)| Queued { action: Action::DragEnd(path, outcome), at: None, input: true });
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

impl RuntimeCore {
    /// Runs `action`'s handlers; debug builds warn when they block the main thread.
    pub(in crate::runtime) fn run_action(&mut self, app: &mut dyn AppDriver, action: Action, tx: &mut Tx) -> bool {
        #[cfg(debug_assertions)]
        let (name, path, at) = (handler_name(&action), stall_path(&action).map(str::to_owned), self.handler_location(&action));
        let stall = crate::tooling::perf_warn::Stall::start();
        let timed = self.interaction_note(&action).map(|note| (note, crate::time::now()));
        let ran = self.dispatch_action(app, action, tx);
        if ran && let Some((note, started)) = timed { self.interaction_ran(note, started); }
        #[cfg(debug_assertions)]
        if ran && name.starts_with("on_") { self.commit.watch.meter.handler(name, path.as_deref(), at); }
        #[cfg(debug_assertions)]
        stall.end(|| match path {
            Some(path) => (crate::tooling::perf_warn::handler_site(name, &path), format!("`{name}` on {path:?}")),
            None => (name.to_owned(), format!("`{name}`")),
        });
        #[cfg(not(debug_assertions))]
        stall.end(|| unreachable!());
        ran
    }

    /// Where the app set the handler `action` runs, for the commit warnings.
    #[cfg(debug_assertions)]
    fn handler_location(&self, action: &Action) -> Option<&'static std::panic::Location<'static>> {
        let sites = |path: &str| self.node(path).map(|n| n.handlers.sites);
        match action {
            Action::Hover(p) => sites(p)?.hover,
            Action::HoverChange(p, _) => sites(p)?.hover_change,
            Action::PointerMove(p, _) => sites(p)?.pointer_move,
            Action::PointerLockMotion(p, _) => sites(p)?.pointer_lock,
            Action::Layout(p, _) => sites(p)?.layout,
            Action::DrawnLayout(p, ..) => sites(p)?.drawn_layout,
            Action::VisibleRows(p, _) => sites(p)?.visible_rows,
            _ => None,
        }
    }

    fn dispatch_action(&mut self, app: &mut dyn AppDriver, action: Action, tx: &mut Tx) -> bool {
        // A caret key or a press reads the field's layout where it lands: a long paste's
        // paragraphs still waiting there are shaped first. Typing at the caret needs none.
        if !self.paint.text_rows.is_empty() && reads_text_layout(&action) { self.fill_text_near(); }
        let mut one = self.transaction(app);
        // Debug builds name the element too, for the stale-view warning.
        #[cfg(debug_assertions)]
        let live_label = live_only_handler(&action).map(|h| match action_path(&action) {
            Some(path) => format!("{h} on {path:?}"),
            None => h.to_owned(),
        });
        #[cfg(not(debug_assertions))]
        let live_label = live_only_handler(&action);
        let repeat = matches!(action, Action::DragRepeat(..));
        let ran = match action {
            Action::WindowFocus(focused) => {
                if !focused { self.kbd.clear_held(); }
                let Some(f) = self.tree.nodes.get(self.app_root()).and_then(|n| n.handlers.on_window_focus.clone()) else { return false };
                f(app.states(), focused, &mut one);
                true
            }
            Action::Physical(event, fallback) => {
                // The window lost key focus (an internal focus move also notifies handlers).
                if matches!(event, crate::PhysicalInput::FocusLost) { self.kbd.clear_held(); }
                let ran = self.dispatch_physical(app, event, fallback, tx);
                let live_only = std::mem::take(&mut tx.live_only);
                return if ran && live_only { self.skip_view("a key handler") } else { ran };
            }
            Action::FocusControl(path) => {
                self.set_focus(app, Some(path), false);
                true
            }
            Action::HoverFocus(path) => match path {
                Some(path) => {
                    self.set_focus(app, Some(path), false);
                    self.kbd.focus.step(focus_machine::FocusEvent::Hover(true));
                    one.commit = one.commit.instant();
                    true
                }
                None => {
                    let lit = self.kbd.focus.hovered();
                    self.kbd.focus.step(focus_machine::FocusEvent::Hover(false));
                    lit
                }
            },
            Action::Dismiss(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.modal.as_ref().and_then(|m| m.dismiss.clone())) else { return false };
                self.set_focus(app, None, self.kbd.focus.keyboard());
                f(app.states(), &mut one);
                true
            }
            Action::PointerMove(path, point) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_pointer_move.clone()) else { return false };
                f(app.states(), point, &mut one);
                true
            }
            Action::PointerLockMotion(path, delta) => {
                let Some(i) = self.live_index(&path).filter(|&i| self.allowed(i) && !self.node_disabled(i)) else { return false };
                let Some(f) = self.tree.nodes[i].handlers.pointer_lock.clone() else { return false };
                f(app.states(), delta, &mut one);
                true
            }
            Action::AtBottom(path, at) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_at_bottom.clone()) else { return false };
                f(app.states(), at)
            }
            Action::FarFromBottom(path, far) => {
                let Some((_, _, f)) = self.node(&path).and_then(|n| n.handlers.on_far_from_bottom.clone()) else { return false };
                f(app.states(), far)
            }
            Action::VisibleRows(path, range) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_visible_rows.clone()) else { return false };
                f(app.states(), range)
            }
            Action::Layout(path, rect) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_layout.clone()) else { return false };
                f(app.states(), rect, &mut one);
                true
            }
            Action::ScreenLayout(path, rect) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_screen_layout.clone()) else { return false };
                f(app.states(), rect);
                true
            }
            Action::DrawnLayout(path, rect, _) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_drawn_layout.clone()) else { return false };
                f(app.states(), rect, &mut one);
                true
            }
            Action::Visible(path, on) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_visible.clone()) else { return false };
                f(app.states(), on, &mut one);
                true
            }
            Action::Hover(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_hover.clone()) else { return false };
                f(app.states(), &mut one);
                true
            }
            Action::HoverChange(path, inside) => {
                let gone = self.pointer.take_hover_gone(&path);
                // A leave still reaches an element on its way out.
                let exiting = || self.tree.nodes.iter().find(|n| !inside && n.path == path).and_then(|n| n.handlers.on_hover_change.clone());
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_hover_change.clone()).or(gone).or_else(exiting) else { return false };
                f(app.states(), inside, &mut one);
                true
            }
            Action::SelectionChange(path, change) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_selection_change.clone()) else { return false };
                f(app.states(), &change, &mut one);
                one.select_from_change = one.select.is_some();
                true
            }
            Action::Press(path, button, touch, point) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_press.clone()) else { return false };
                log_event!(self, "press {path}");
                one.button = button;
                one.touch_tap = touch;
                one.press_point = point;
                f(app.states(), &mut one);
                true
            }
            Action::DoublePress(path, point) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_double_press.clone()) else { return false };
                log_event!(self, "double press {path}");
                one.press_point = point;
                f(app.states(), &mut one);
                true
            }
            Action::MiddlePress(path, point) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_middle_press.clone()) else { return false };
                log_event!(self, "middle press {path}");
                one.button = crate::PointerButton::Middle;
                one.press_point = point;
                f(app.states(), &mut one);
                true
            }
            Action::AnyPress(path, button, point) => {
                let Some(i) = self.live_index(&path).filter(|&i| self.allowed(i)) else { return false };
                let Some(f) = self.tree.nodes[i].handlers.on_any_press.clone() else { return false };
                log_event!(self, "any press {path}");
                one.button = button;
                f(app.states(), button, point, &mut one);
                true
            }
            Action::Link(path, k, button) => {
                one.button = button;
                self.press_link(app, &path, k, &mut one)
            }
            Action::LinkSecondary(path, k, point) => self.secondary_press_link_or_menu(app, &path, k, point, &mut one),
            Action::LinkHover(path, url, rect, pressing) => {
                one.pressing = pressing;
                self.dispatch_link_hover(app, &path, url.as_deref(), rect, &mut one)
            }
            Action::SecondaryPress(path, point) => {
                let Some(n) = self.node(&path).filter(|n| !self.is_exiting(n.id)) else { return false };
                let Some(f) = n.handlers.on_secondary_press.clone() else { return false };
                log_event!(self, "secondary press {path}");
                f(app.states(), point, &mut one);
                true
            }
            Action::SecondaryRepost { modal, outer, down, up } => {
                self.run_secondary_repost(app, &modal, outer.as_deref(), down, up, &mut one)
            }
            Action::Drag(path, mut d, button) | Action::DragRepeat(path, mut d, button) => {
                let gone = (d.phase == Phase::Up && d.cancelled).then(|| self.pointer.take_drag_gone(&path)).flatten();
                let Some(f) = gone.or_else(|| self.node(&path).and_then(|n| n.handlers.on_drag.clone())) else { return false };
                if d.phase == Phase::Move {
                    one.commit = one.commit.instant();
                }
                one.button = button;
                one.drag_repeat = repeat;
                d.data = if d.phase == Phase::Down { 0.0 } else { self.pointer.drag_data() };
                f(app.states(), &mut d, &mut one);
                self.pointer.set_drag_data(d.data);
                if let Some(after) = one.drag_repeat_after.take() {
                    self.arm_press_repeat(&path, after);
                }
                if d.phase != Phase::Move {
                    log_event!(self, "drag {:?} {path}", d.phase);
                }
                true
            }
            Action::EdgeDrag(path, mut d) => {
                let gone = (d.phase == Phase::Up && d.cancelled)
                    .then(|| self.pointer.touch.take_edge_gone(&path))
                    .flatten();
                let Some(f) = gone.or_else(|| {
                    let edge = d.edge?;
                    self.node(&path).and_then(|n| {
                        n.handlers.edge_drags.as_deref().and_then(|e| e.get(edge)).map(|(_, f)| f.clone())
                    })
                }) else {
                    return false;
                };
                if d.phase == Phase::Move {
                    one.commit = one.commit.instant();
                }
                d.data = if d.phase == Phase::Down { 0.0 } else { self.pointer.touch.edge_data };
                f(app.states(), &mut d, &mut one);
                self.pointer.touch.edge_data = d.data;
                if d.phase != Phase::Move {
                    log_event!(self, "edge drag {:?} {path}", d.phase);
                }
                true
            }
            Action::DragStart(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_drag_start.clone()) else { return false };
                f(app.states(), &mut one);
                true
            }
            Action::DragEnd(path, outcome) => {
                log_event!(self, "drag-end {outcome:?} {path}");
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_drag_end.clone()) else { return false };
                f(app.states(), outcome, &mut one);
                true
            }
            Action::Files(event) => self.run_files(app, event, &mut one),
            Action::Key { ev, repeat, stage } => {
                if self.kbd.held_gate(None, Some(&ev.key), repeat) { return false; }
                if ! matches!(ev.key, crate::Key::Modifier { .. }) { self.note_key_down(app, repeat); }
                if stage { self.route_stage_key(app, &ev, &mut one) } else { self.route_key(app, &ev, repeat, &mut one) }
            }
            Action::KeyUp(ev) => self.route_key_up(app, &ev, &mut one),
            Action::Wheel { gesture, chain, ev, phased } => self.run_wheel(app, gesture, chain, ev, phased, &mut one),
            Action::WheelRepost(repost) => self.run_wheel_repost(app, repost, &mut one),
            Action::Gesture(chain, gesture) => self.run_gesture(app, chain, gesture, &mut one),
            Action::Ime(ev) => self.run_ime(app, ev, &mut one),
            Action::FinishIme => self.finish_ime(app, &mut one),
            Action::Paste(mut content, target) => {
                if target.is_some_and(|target| target != self.paste_target()) { return false; }
                let hook = self.focused_input().and_then(|i| i.paste.clone());
                if cfg!(target_arch = "wasm32")
                    && !self.edit.has_clipboard()
                    && content.html().is_some_and(|h| self.edit.has_pill_copy_matching(content.text(), h))
                {
                    content.mark_own_pills();
                }
                let own_pills = content.has_own_pills();
                let into_field = |mut state: crate::states::States<'_>, access: &crate::el::EditFn, tx: &mut Tx| {
                    if hook.is_some_and(|hook| hook(state.reborrow(), &content, tx)) { return EditOutcome::default(); }
                    let edit = access(state);
                    // A rich field keeps the paste's formatting. The HTML was sanitised when it
                    // was read, and `from_html` keeps only its own safe subset anyway.
                    if let Some(html) = content.html() && edit.is_rich() && !edit.is_secure() {
                        return if own_pills { edit.paste_html_with_pills(html, content.text()) } else { edit.paste_html(html, content.text()) };
                    }
                    if !content.has_plain() { return EditOutcome::default(); }
                    edit.paste(content.text())
                };
                if !self.focused_input_read_only() && self.with_focused_state(app, into_field, &mut one) {
                    log_event!(self, "paste field");
                    true
                } else if let Some(f) = self.app_paste_handler() && (f.call)(app.states(), &content, &mut one) {
                    log_event!(self, "paste window");
                    true
                } else { false }
            }
            Action::ClipboardRead(id, mut content) => {
                if cfg!(target_arch = "wasm32")
                    && !self.edit.has_clipboard()
                    && content.html().is_some_and(|h| self.edit.has_pill_copy_matching(content.text(), h))
                {
                    content.mark_own_pills();
                }
                let Some(f) = self.edit.take_clipboard_read(id) else { return false };
                f(app.states(), &content, &mut one);
                true
            }
            Action::FileDialog(id, result) => self.run_file_dialog(app, id, result, &mut one),
            Action::CopyImageDone(id, result) => {
                if let Some(f) = self.edit.take_copy_image_callback(id) {
                    f(app.states(), result, &mut one);
                    true
                } else {
                    false
                }
            }
            Action::UrlOpened(f, result) => {
                f(app.states(), result, &mut one);
                true
            }
            Action::Focus(path, x, y) => self.focus_input(app, path, x, y, &mut one),
            Action::FieldMenu(path, at, local) => self.field_menu(app, path, at, local, &mut one),
            Action::Blur => {
                if self.kbd.focus.path().is_none() { return false; }
                self.set_focus(app, None, self.kbd.focus.keyboard());
                true
            }
            Action::A11y(path, act) => self.run_a11y(app, path, act, &mut one),
        };
        let live_only = std::mem::take(&mut one.live_only);
        if live_only && live_label.is_some() {
            // A handler that asks for no commit cannot change another handler's animation.
            one.commit = Commit::Animate;
            one.motion = None;
            one.follow_edge = false;
            one.color_motion = None;
        }
        merge(tx, one);
        if ran && live_only && let Some(label) = live_label {
            return self.skip_view(label);
        }
        ran
    }

    pub(crate) fn queue_a11y(&mut self, path: String, act: crate::a11y::actions::A11yAct) {
        self.frame.queue.push(Action::A11y(path, act));
    }
    pub(crate) fn clear_a11y_steps(&mut self, path: &str) -> bool {
        let before = self.frame.queue.len();
        self.frame.queue.retain(|a| !matches!(a, Action::A11y(p, crate::a11y::actions::A11yAct::Step(_)) if p == path));
        self.frame.queue.len() != before
    }
    pub(crate) fn queue_a11y_press(&mut self, path: String) {
        self.frame.queue.push(Action::Press(path, crate::PointerButton::Primary, false, None));
    }
    pub(crate) fn queue_a11y_secondary_press(&mut self, path: String, ev: PointerEvent) {
        self.frame.queue.push(Action::SecondaryPress(path, ev));
    }
    pub(crate) fn queue_a11y_link(&mut self, path: String, link: usize) {
        self.frame.queue.push(Action::Link(path, link, crate::PointerButton::Primary));
    }
    pub(crate) fn queue_a11y_link_secondary(&mut self, path: String, link: usize, ev: PointerEvent) {
        self.frame.queue.push(Action::LinkSecondary(path, link, ev));
    }
}

/// Whether `action` reads a text's layout away from its caret: caret movement, a press or a
/// drag. Text typed, deleted or composed at the caret doesn't.
fn reads_text_layout(action: &Action) -> bool {
    use crate::Key;
    match action {
        Action::Key { ev, .. } => match &ev.key {
            Key::Text(_) | Key::Backspace | Key::Delete => ev.mods.cmd || ev.mods.ctrl || ev.mods.alt,
            Key::Modifier { .. } => false,
            _ => true,
        },
        Action::Press(..) | Action::DoublePress(..) | Action::MiddlePress(..) | Action::SecondaryPress(..) | Action::SecondaryRepost { .. } | Action::Drag(..) | Action::DragStart(..) => true,
        _ => false,
    }
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
