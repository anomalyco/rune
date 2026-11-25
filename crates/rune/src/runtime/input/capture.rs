//! The pointer's hold on an element: the primary (or an extra) button's press from down to up,
//! and the secondary button's beside it. A held press keeps what only it has (where and when it
//! went down, whether it's still inside its element or a scroll cancelled it, whether it
//! travelled past the click slop, whether it's the second of a double press, an armed OS drag,
//! the link it began on),
//! so none of it outlives the release. Between presses only the first click of a possible
//! double press is kept. The input routes in `pointer.rs`, `gesture.rs`, `secondary.rs` and
//! `selection/click.rs` drive it.
use rune_motion::NodeId;

use crate::PointerButton;
use crate::el::Drag;
use crate::runtime::types::Node;

/// Where a held press stands against its element: a browser's `:active` while it's inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit {
    Inside,
    /// The pointer moved off the element; back on, it's inside again.
    Outside,
    /// A scroll under the press let go of it ([`Runtime::scroll_cancels_press`]): the release
    /// presses nothing, wherever the pointer is.
    Cancelled,
}

/// The first click of a possible double press: the element, its down time and point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FirstClick {
    pub(crate) path: String,
    pub(crate) at: f64,
    pub(crate) point: (f32, f32),
}

/// A press the pointer holds on element `path`.
#[derive(Clone)]
pub(crate) struct Held {
    pub(crate) path: String,
    /// The `Phase::Down` its drags carry on from.
    pub(crate) drag: Drag,
    pub(crate) button: PointerButton,
    /// Where (window points) and when (`now_clock`) it went down.
    pub(crate) at: (f32, f32),
    pub(crate) down_at: f64,
    pub(crate) hit: Hit,
    /// It travelled past the click slop: it isn't a click of a double press, nor a press on an element that drags.
    pub(crate) moved: bool,
    /// It's the second press of a double press.
    pub(crate) second: bool,
    /// It's on a `drag_out` element and hasn't crossed this threshold (points) yet.
    pub(crate) drag_out: Option<f32>,
    /// The pointer left the window while it held; the leave lands on release.
    pub(crate) left: bool,
    /// The link of its text it began on: a release on the same link activates it.
    pub(crate) link: Option<usize>,
    /// It ran its element's `on_press` as it went down (`El::press_on_down`): its release
    /// runs nothing.
    pub(crate) pressed_on_down: bool,
    pub(crate) touch: bool,
    /// The `on_drag` handler that began this drag, kept so a cancel still reaches it if the
    /// element leaves the tree or stops dragging.
    pub(crate) drag_handler: Option<crate::el::DragFn>,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Held")
            .field("path", &self.path)
            .field("drag", &self.drag)
            .field("button", &self.button)
            .field("at", &self.at)
            .field("down_at", &self.down_at)
            .field("hit", &self.hit)
            .field("moved", &self.moved)
            .field("second", &self.second)
            .field("drag_out", &self.drag_out)
            .field("left", &self.left)
            .field("link", &self.link)
            .field("pressed_on_down", &self.pressed_on_down)
            .field("touch", &self.touch)
            .field("drag_handler", &self.drag_handler.is_some())
            .finish()
    }
}

impl Held {
    /// A press that just went down at `at`, inside its element and still.
    pub(crate) fn new(path: String, drag: Drag, button: PointerButton, at: (f32, f32), down_at: f64, second: bool, drag_out: Option<f32>) -> Self {
        Self { path, drag, button, at, down_at, hit: Hit::Inside, moved: false, second, drag_out, left: false, link: None, pressed_on_down: false, touch: false, drag_handler: None }
    }

    pub(crate) fn inside(&self) -> bool { self.hit == Hit::Inside && !self.left }

    /// Began on link `link` of its text.
    pub(crate) fn on_link(self, link: Option<usize>) -> Self { Self { link, ..self } }

    /// Ran its element's `on_press` as it went down.
    pub(crate) fn pressed_on_down(self, pressed_on_down: bool) -> Self { Self { pressed_on_down, ..self } }

    /// Started an `on_drag` gesture on `drag_handler`.
    pub(crate) fn drag_handler(self, drag_handler: Option<crate::el::DragFn>) -> Self { Self { drag_handler, ..self } }
}

#[derive(Clone, Debug)]
pub(crate) enum Press {
    Up { first: Option<FirstClick> },
    Held(Box<Held>),
}

impl Default for Press {
    fn default() -> Self { Self::Up { first: None } }
}

impl Press {
    pub(crate) fn held(&self) -> Option<&Held> {
        match self {
            Self::Held(h) => Some(h),
            Self::Up { .. } => None,
        }
    }

    pub(crate) fn held_mut(&mut self) -> Option<&mut Held> {
        match self {
            Self::Held(h) => Some(h),
            Self::Up { .. } => None,
        }
    }

    pub(crate) fn path(&self) -> Option<&str> { self.held().map(|h| h.path.as_str()) }

    pub(crate) fn is_held(&self) -> bool { matches!(self, Self::Held(_)) }

    /// Held, and inside its element.
    pub(crate) fn inside(&self) -> bool { self.held().is_some_and(Held::inside) }

    /// Lets go of a held press without a click to pair: a cancel, a drag-out taking the pointer,
    /// a modal opening. Returns it.
    pub(crate) fn drop_held(&mut self) -> Option<Box<Held>> {
        match std::mem::take(self) {
            Self::Held(h) => Some(h),
            up => {
                *self = up;
                None
            }
        }
    }

    /// The release: back up, with `first` the click a next press may pair with. Returns the
    /// press that was held.
    pub(crate) fn release(&mut self, first: Option<FirstClick>) -> Option<Box<Held>> {
        let held = self.drop_held()?;
        *self = Self::Up { first };
        Some(held)
    }

    /// A new press takes the place of whatever came before: the first click it may pair with.
    pub(crate) fn take_first(&mut self) -> Option<FirstClick> {
        match self {
            Self::Up { first } => first.take(),
            Self::Held(_) => None,
        }
    }

    pub(crate) fn hold(&mut self, held: Held) { *self = Self::Held(Box::new(held)); }
}

/// Which kind of link a secondary press began on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SecondaryLink {
    /// Explicit `El::on_link_secondary` handler: wins over ancestors and claims `"app"` on web.
    Explicit(usize),
    /// Default Open Link / Copy Link menu: yields to ancestors and leaves web mouse right-clicks
    /// to the browser's `<a href>` menu.
    Default(usize),
}

/// The secondary button's hold: the element whose menu it opens on release, the link of its
/// text it began on (`on_link_secondary` or default link menu), and whether the pointer left
/// the window meanwhile.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Secondary {
    #[default]
    Up,
    Held { path: String, link: Option<SecondaryLink>, left: bool },
    /// An outside secondary press dismissed (or was swallowed by) an active modal; release does
    /// nothing, and on web `secondary_claim()` returns `"app"` so the browser's own context menu
    /// stays closed.
    Dismissed,
    /// On Windows, an outside secondary press dismissed a `click_outside` menu modal and will
    /// repost to what lies under it once the dismissal commits and rebuilds hit-testing.
    DismissRepost { modal: String, outer: Option<String>, down: (f32, f32), left: bool },
}

/// The middle or secondary button held where the primary isn't: the element it shows pressed
/// ([`El::press`](crate::El::press)) while the pointer is over it, as a browser sets `:active`
/// for a press of any button. It's the element a primary press there would take; it hears
/// nothing, and the release lets go of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Tint {
    #[default]
    Up,
    Held { path: String, button: PointerButton },
}

impl Tint {
    pub(crate) fn path(&self) -> Option<&str> {
        match self {
            Self::Held { path, .. } => Some(path),
            Self::Up => None,
        }
    }

    /// `button` came up: it lets go if it's the one holding.
    pub(crate) fn release(&mut self, button: PointerButton) {
        if matches!(self, Self::Held { button: b, .. } if *b == button) { *self = Self::Up; }
    }
}

impl Secondary {
    pub(crate) fn path(&self) -> Option<&str> {
        match self {
            Self::Held { path, .. } => Some(path),
            Self::Up | Self::Dismissed | Self::DismissRepost { .. } => None,
        }
    }

    pub(crate) fn is_held(&self) -> bool { matches!(self, Self::Held { .. }) }

    pub(crate) fn is_captured(&self) -> bool {
        matches!(self, Self::Held { .. } | Self::DismissRepost { .. })
    }
}

/// Ownership of an active phased gesture (`!MagnifyEvent::wheel`, `RotateEvent`, or
/// handler-only touchscreen `WheelEvent`), locked from `GesturePhase::Began` until
/// `GesturePhase::Ended` (or pointer cancel / stage clear). Unphased `Moved` events (including
/// `Ctrl + wheel` with `wheel: true`) bypass this lock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum GestureOwner {
    /// No phased gesture is active on this channel.
    #[default]
    Idle,
    /// Handler at `path` accepted `Began` and receives `Moved` and `Ended` in its own local
    /// coordinates even if the midpoint or cursor leaves its bounds.
    Owned(String),
    /// `Began` had no accepting handler (or the owning handler disappeared or declined); later
    /// `Moved` and `Ended` of this gesture are ignored until `Ended` returns to `Idle`.
    Declined,
}

/// Independent per-channel gesture owner locks stored on [`PointerState`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GestureOwners {
    pub(crate) magnify: GestureOwner,
    pub(crate) rotate: GestureOwner,
    pub(crate) touch_wheel: GestureOwner,
}

impl GestureOwners {
    #[inline]
    pub(crate) fn clear(&mut self) {
        self.magnify = GestureOwner::Idle;
        self.rotate = GestureOwner::Idle;
        self.touch_wheel = GestureOwner::Idle;
    }
}

/// Currently hovered link reported to a text node's [`crate::El::on_link_hover`] handler.
#[derive(Clone)]
pub(in crate::runtime) struct HoveredLink {
    pub(in crate::runtime) path: String,
    pub(in crate::runtime) node_id: crate::NodeId,
    pub(in crate::runtime) link_index: usize,
    pub(in crate::runtime) link_id: String,
    pub(in crate::runtime) handler: crate::el::LinkHoverFn,
}

/// Pointer position, button capture, hover/tooltip tracking, touch, autoscroll, file drops and OS
/// drag-out sessions owned by [`crate::runtime::RuntimeCore`]. Ordered hot-first under
/// `#[repr(C)]` so pointer position, press capture and hover state sit together at offset 0.
#[repr(C)]
pub(in crate::runtime) struct PointerState {
    pub(in crate::runtime) pos: Option<(f32, f32)>,
    /// A press or blur stopped the waits to report an enter; the pointer's next move restarts them.
    pub(in crate::runtime) hover_paused: bool,
    /// The host starts OS drags, so `drag_out` elements start one (headless tools act as one).
    pub(in crate::runtime) os_drag: bool,
    /// Distinct bits for currently held named mouse buttons (Primary..Forward), for
    /// `PointerEvent::pressing` and `El::on_any_press` chord suppression.
    buttons_down: u8,
    pub(in crate::runtime) hovered: Option<String>,
    /// Elements with `on_hover_change` that the pointer is inside.
    pub(in crate::runtime) hover_within: Vec<String>,
    /// Hover waiting out layout motion under a still pointer.
    pub(super) hover_hold: crate::runtime::hover::HoverHold,
    /// The press the pointer holds, or the click a next press may pair with
    /// (`runtime/input/capture.rs`).
    pub(super) press: Press,
    /// The secondary button's press, held beside it.
    secondary: Secondary,
    /// The middle or secondary button's pressed look (`capture::Tint`).
    tint: Tint,
    /// Elements with `hover_delays`: which heard the enter, and which wait out a delay.
    pub(in crate::runtime) hover_delays: crate::runtime::hover_delay::HoverDelays,
    /// `on_hover_change` handlers of elements that left the tree, owed their queued leave.
    hover_gone: Vec<(String, crate::el::WindowFocusFn)>,
    /// Currently hovered link reported to a text node's `on_link_hover` handler.
    pub(in crate::runtime) link_hover: Option<Box<HoveredLink>>,
    /// `on_link_hover` handlers of elements whose leave was queued when they left or switched.
    link_hover_gone: Vec<(String, crate::el::LinkHoverFn)>,
    /// `on_drag` handlers of cancelled drags, kept so the queued cancelled `Up` still reaches them.
    drag_gone: Vec<(String, crate::el::DragFn)>,
    /// The presses counted into `Drag::clicks`.
    press_clicks: crate::runtime::press_run::PressRun,
    /// The held press's drag repeat (`Tx::drag_repeat_after`).
    pub(super) press_repeat: crate::runtime::press_repeat::PressRepeat,
    drag_data: f32,
    /// The middle button's autoscroll (`runtime/input/autoscroll.rs`).
    pub(super) autoscroll: crate::runtime::autoscroll::Autoscroll,
    /// The finger that is down and the viewport a touch holds (`runtime/input/touch.rs`).
    pub(super) touch: crate::runtime::touch_input::Touch,
    /// `El::help` tooltips.
    pub(super) help: crate::runtime::help::HelpState,
    /// Files dragged over the window from another app (`drop_dispatch`).
    pub(super) file_drag: crate::runtime::drop_dispatch::FileDrag,
    pub(in crate::runtime) drag_out_request: Option<crate::drag_out::DragOutStart>,
    /// How OS drags ended, filled by the host's session callbacks between frames.
    pub(in crate::runtime) drag_ends: crate::drag_out::DragEnds,
    /// Drag-outs whose `on_drag_start` ran and whose `on_drag_end` hasn't yet.
    pub(in crate::runtime) drags_open: usize,
    /// Currently held `PointerButton::Other(n)` ids (empty unless an extra button is held).
    other_buttons_down: Vec<u16>,
    /// Declarative pointer-lock state machine (`runtime/input/pointer_lock.rs`).
    pub(super) pointer_lock: super::pointer_lock::PointerLock,
    /// Locked owners for phased magnify, rotate, and touch-wheel gestures (`runtime/input/gesture.rs`).
    pub(super) gesture_owners: GestureOwners,
}

impl PointerState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            pos: None,
            press: Default::default(),
            secondary: Default::default(),
            tint: Default::default(),
            press_clicks: Default::default(),
            press_repeat: Default::default(),
            drag_data: 0.0,
            autoscroll: Default::default(),
            touch: Default::default(),
            hovered: None,
            hover_within: Vec::new(),
            hover_gone: Vec::new(),
            link_hover: None,
            link_hover_gone: Vec::new(),
            drag_gone: Vec::new(),
            hover_delays: Default::default(),
            hover_paused: false,
            hover_hold: Default::default(),
            help: Default::default(),
            file_drag: Default::default(),
            os_drag: true,
            buttons_down: 0,
            drag_out_request: None,
            drag_ends: Default::default(),
            drags_open: 0,
            other_buttons_down: Vec::new(),
            pointer_lock: super::pointer_lock::PointerLock::new(),
            gesture_owners: GestureOwners::default(),
        }
    }

    const BTN_PRIMARY: u8 = 1 << 0;
    const BTN_SECONDARY: u8 = 1 << 1;
    const BTN_MIDDLE: u8 = 1 << 2;
    const BTN_BACK: u8 = 1 << 3;
    const BTN_FORWARD: u8 = 1 << 4;

    #[inline]
    const fn button_bit(button: PointerButton) -> u8 {
        match button {
            PointerButton::Primary => Self::BTN_PRIMARY,
            PointerButton::Secondary => Self::BTN_SECONDARY,
            PointerButton::Middle => Self::BTN_MIDDLE,
            PointerButton::Back => Self::BTN_BACK,
            PointerButton::Forward => Self::BTN_FORWARD,
            PointerButton::Other(_) => 0,
        }
    }

    /// Whether any mouse button is currently held down anywhere in the window.
    #[inline]
    pub(in crate::runtime) fn any_button_down(&self) -> bool {
        self.buttons_down != 0 || !self.other_buttons_down.is_empty()
    }

    #[inline]
    pub(in crate::runtime) fn note_button_down(&mut self, button: PointerButton) {
        if let PointerButton::Other(id) = button {
            if !self.other_buttons_down.contains(&id) {
                self.other_buttons_down.push(id);
            }
        } else {
            self.buttons_down |= Self::button_bit(button);
        }
    }

    #[inline]
    pub(in crate::runtime) fn note_button_up(&mut self, button: PointerButton) {
        if let PointerButton::Other(id) = button {
            self.other_buttons_down.retain(|&b| b != id);
        } else {
            self.buttons_down &= !Self::button_bit(button);
        }
    }

    #[inline]
    pub(in crate::runtime) fn clear_buttons_down(&mut self) {
        self.buttons_down = 0;
        self.other_buttons_down.clear();
    }

    #[inline]
    pub(in crate::runtime) fn pos(&self) -> Option<(f32, f32)> { self.pos }

    /// A press on an element is held (the hand drives whatever follows it).
    #[inline]
    pub(in crate::runtime) fn pressing(&self) -> bool { self.press.is_held() }

    /// A button holds the pointer: the OS keeps sending its moves to this window, and a leave
    /// waits for the release.
    #[inline]
    pub(in crate::runtime) fn captured(&self) -> bool {
        self.press.is_held() || self.secondary.is_captured() || self.tint.path().is_some()
    }

    #[inline]
    pub(super) fn captured_path(&self) -> Option<&str> {
        self.press.path().or(self.secondary.path())
    }

    #[inline]
    pub(in crate::runtime) fn press_inside(&self) -> bool { self.press.inside() }

    #[inline]
    pub(in crate::runtime) fn press_held(&self) -> Option<&Held> { self.press.held() }

    #[inline]
    pub(in crate::runtime) fn hold_press(&mut self, held: Held) { self.press.hold(held); }

    /// Whether the pointer left the window while a press holds capture.
    #[inline]
    pub(in crate::runtime) fn left(&self) -> bool {
        self.press.held().is_some_and(|h| h.left)
            || matches!(self.secondary, Secondary::Held { left: true, .. } | Secondary::DismissRepost { left: true, .. })
    }

    /// The pointer left the window (`true`) or came back (`false`) while a press holds it.
    #[inline]
    pub(super) fn set_left(&mut self, out: bool) {
        if let Some(held) = self.press.held_mut() { held.left = out; }
        match &mut self.secondary {
            Secondary::Held { left, .. } | Secondary::DismissRepost { left, .. } => *left = out,
            Secondary::Up | Secondary::Dismissed => {}
        }
    }

    /// The host moved its window by `(dx, dy)` points under a still pointer.
    #[inline]
    pub(in crate::runtime) fn window_moved(&mut self, dx: f32, dy: f32) {
        if let Some((x, y)) = self.pos { self.pos = Some((x - dx, y - dy)); }
        if let Some(held) = self.press.held_mut() { held.at = (held.at.0 - dx, held.at.1 - dy); }
        if let Secondary::DismissRepost { down, .. } = &mut self.secondary { *down = (down.0 - dx, down.1 - dy); }
    }

    /// The button is up, so the held press can no longer start a drag-out; it stays a press.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[inline]
    pub(in crate::runtime) fn disarm_drag_out(&mut self) {
        if let Some(held) = self.press.held_mut() { held.drag_out = None; }
    }

    #[cfg(all(test, target_os = "macos"))]
    #[inline]
    pub(in crate::runtime) fn end_click_run(&mut self) {
        self.press_clicks.clear();
        self.press.take_first();
    }

    #[inline]
    pub(super) fn drag_data(&self) -> f32 { self.drag_data }

    #[inline]
    pub(super) fn set_drag_data(&mut self, data: f32) { self.drag_data = data; }

    #[inline]
    pub(super) fn tint_path(&self) -> Option<&str> { self.tint.path() }

    #[inline]
    pub(super) fn set_tint(&mut self, tint: Tint) { self.tint = tint; }

    #[inline]
    pub(super) fn release_tint(&mut self, button: PointerButton) { self.tint.release(button); }

    #[inline]
    pub(super) fn clear_tint(&mut self) { self.tint = Tint::Up; }

    #[inline]
    pub(super) fn press_click(&mut self, path: &str, now: f64, at: (f32, f32), interval: f64) -> u32 {
        self.press_clicks.press(path, now, at, interval)
    }

    #[inline]
    pub(super) fn clear_press_clicks(&mut self) { self.press_clicks.clear(); }

    #[inline]
    pub(super) fn push_hover_gone(&mut self, path: String, f: crate::el::WindowFocusFn) {
        self.hover_gone.push((path, f));
    }

    #[inline]
    pub(super) fn take_hover_gone(&mut self, path: &str) -> Option<crate::el::WindowFocusFn> {
        self.hover_gone.iter().position(|g| g.0 == path).map(|i| self.hover_gone.remove(i).1)
    }

    #[inline]
    pub(in crate::runtime) fn push_link_hover_gone(&mut self, path: String, f: crate::el::LinkHoverFn) {
        self.link_hover_gone.push((path, f));
    }

    #[inline]
    pub(in crate::runtime) fn take_link_hover_gone(&mut self, path: &str) -> Option<crate::el::LinkHoverFn> {
        self.link_hover_gone.iter().position(|g| g.0 == path).map(|i| self.link_hover_gone.remove(i).1)
    }

    #[inline]
    pub(super) fn push_drag_gone(&mut self, path: String, f: crate::el::DragFn) {
        self.drag_gone.push((path, f));
    }

    #[inline]
    pub(super) fn take_drag_gone(&mut self, path: &str) -> Option<crate::el::DragFn> {
        self.drag_gone.iter().position(|g| g.0 == path).map(|i| self.drag_gone.remove(i).1)
    }

    #[cfg(all(test, target_os = "macos"))]
    #[inline]
    pub(super) fn clear_drag_gone(&mut self) {
        self.drag_gone.clear();
    }

    /// Follows a held press whose path shifted across an unkeyed ancestor's sibling change.
    pub(in crate::runtime) fn rebind_press_path(&mut self, next: &str) {
        let Some(held) = self.press.held_mut() else { return };
        let old = std::mem::replace(&mut held.path, next.to_owned());
        self.press_repeat.rebind(&old, next);
    }

    #[inline]
    pub(super) fn secondary_held(&self) -> bool { self.secondary.is_held() }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[inline]
    pub(super) fn secondary(&self) -> &Secondary { &self.secondary }

    #[inline]
    pub(super) fn set_secondary(&mut self, secondary: Secondary) { self.secondary = secondary; }

    #[inline]
    pub(super) fn clear_secondary(&mut self) { self.secondary = Secondary::Up; }

    #[inline]
    pub(super) fn take_secondary(&mut self) -> Secondary { std::mem::take(&mut self.secondary) }

    /// Drops transient pointer, hover, click-run and drop-target state when a playground stage
    /// swaps views.
    pub(in crate::runtime) fn clear_for_stage(&mut self) {
        self.press = Press::default();
        self.secondary = Secondary::Up;
        self.clear_buttons_down();
        self.hovered = None;
        self.hover_gone.clear();
        self.drag_gone.clear();
        self.hover_within.clear();
        self.hover_delays.clear();
        self.forget_drop_targets();
        self.press_clicks.clear();
        self.pointer_lock.step(super::pointer_lock::LockEvent::Released);
        self.gesture_owners.clear();
        self.touch.clear_edge_state();
    }

    /// Scrolling viewport `id` under a held press inside it cancels the press, as a UIScrollView
    /// cancels its content's touches: the row lets go and the release doesn't press. A drag
    /// keeps its capture.
    pub(in crate::runtime) fn scroll_cancels_press(&mut self, nodes: &[Node], id: NodeId) {
        let Some(path) = self.press.path() else { return };
        let Some(i) = nodes.iter().position(|n| n.path == path) else { return };
        let inside = std::iter::successors(nodes[i].parent, |&p| nodes[p].parent).any(|p| nodes[p].id == id);
        if inside && nodes[i].handlers.on_drag.is_none() {
            if let Some(held) = self.press.held_mut() {
                held.hit = Hit::Cancelled;
                held.second = false;
            }
        }
    }

    #[inline]
    pub(in crate::runtime) fn touch_scroll_mut(&mut self) -> &mut Option<crate::runtime::scroll_touch::TouchScroll> {
        &mut self.touch.scroll
    }

    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    #[inline]
    pub(in crate::runtime) fn touch_scroll(&self) -> Option<crate::runtime::scroll_touch::TouchScroll> {
        self.touch.scroll
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn hover_hold(&self) -> &crate::runtime::hover::HoverHold {
        &self.hover_hold
    }
}
