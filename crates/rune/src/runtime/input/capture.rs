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
#[derive(Clone, Debug)]
pub(crate) struct Held {
    pub(crate) path: String,
    /// The `Phase::Down` its drags carry on from.
    pub(crate) drag: Drag,
    pub(crate) button: PointerButton,
    /// Where (window points) and when (`now_clock`) it went down.
    pub(crate) at: (f32, f32),
    pub(crate) down_at: f64,
    pub(crate) hit: Hit,
    /// It travelled past the click slop: it isn't a click of a double press.
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
}

impl Held {
    /// A press that just went down at `at`, inside its element and still.
    pub(crate) fn new(path: String, drag: Drag, button: PointerButton, at: (f32, f32), down_at: f64, second: bool, drag_out: Option<f32>) -> Self {
        Self { path, drag, button, at, down_at, hit: Hit::Inside, moved: false, second, drag_out, left: false, link: None, pressed_on_down: false, touch: false }
    }

    pub(crate) fn inside(&self) -> bool { self.hit == Hit::Inside }

    /// Began on link `link` of its text.
    pub(crate) fn on_link(self, link: Option<usize>) -> Self { Self { link, ..self } }

    /// Ran its element's `on_press` as it went down.
    pub(crate) fn pressed_on_down(self, pressed_on_down: bool) -> Self { Self { pressed_on_down, ..self } }
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

/// The secondary button's hold: the element whose menu it opens on release, the link of its
/// text it began on (`on_link_secondary`), and whether the pointer left the window meanwhile.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Secondary {
    #[default]
    Up,
    Held { path: String, link: Option<usize>, left: bool },
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
            Self::Up => None,
        }
    }

    pub(crate) fn is_held(&self) -> bool { matches!(self, Self::Held { .. }) }
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
            hover_delays: Default::default(),
            hover_paused: false,
            hover_hold: Default::default(),
            help: Default::default(),
            file_drag: Default::default(),
            os_drag: true,
            drag_out_request: None,
            drag_ends: Default::default(),
            drags_open: 0,
        }
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
        self.press.is_held() || self.secondary.is_held() || self.tint.path().is_some()
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

    /// The pointer left the window (`true`) or came back (`false`) while a press holds it.
    #[inline]
    pub(super) fn set_left(&mut self, out: bool) {
        if let Some(held) = self.press.held_mut() { held.left = out; }
        if let Secondary::Held { left, .. } = &mut self.secondary { *left = out; }
    }

    /// The host moved its window by `(dx, dy)` points under a still pointer.
    #[inline]
    pub(in crate::runtime) fn window_moved(&mut self, dx: f32, dy: f32) {
        if let Some((x, y)) = self.pos { self.pos = Some((x - dx, y - dy)); }
        if let Some(held) = self.press.held_mut() { held.at = (held.at.0 - dx, held.at.1 - dy); }
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
        self.hovered = None;
        self.hover_gone.clear();
        self.hover_within.clear();
        self.hover_delays.clear();
        self.forget_drop_targets();
        self.press_clicks.clear();
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

    #[cfg(test)]
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
