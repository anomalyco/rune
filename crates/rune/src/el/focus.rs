//! What `El` holds for focus and keys: `FocusSpec`, kept behind `Cold` because most elements set none of it.

use std::rc::Rc;

use crate::color::Color;
use crate::el::{El, InputKeyFn, Outline, PressFn, RoleTag, StepFn, StepLimitFn, Tx};
use crate::states::downcast;

#[derive(Clone)]
pub(crate) struct FocusSpec {
    pub physical: Option<crate::input::physical_input::Handler>,
    pub enabled: bool,
    /// `focusable(false)`: not a Tab stop even where [`crate::Cx::web_focus`] would make it one.
    pub opt_out: bool,
    /// [`crate::El::autofocus`].
    pub autofocus: bool,
    pub order: i32,
    pub group: bool,
    pub selected: bool,
    pub reveal_selected: bool,
    pub preserve_on_press: crate::runtime::selectable::OnPress,
    pub ring: Option<Outline>,
    /// `focus_within_ring`: `ring` shows while focus is on this element or inside it, from
    /// the pointer or the keyboard.
    pub within: bool,
    /// `no_focus_ring`: no default ring on this control, and none from this element.
    pub ring_off: bool,
    /// `focus_within_flat`: no outer or inset shadows and no hover fill while focus is on this
    /// element or inside it.
    pub flat_within: bool,
    /// `default_button`: Return presses it.
    pub default: bool,
    pub highlight: Option<Color>,
    /// `focus_fill`: painted over the element while it shows keyboard focus.
    pub fill: Option<Color>,
    pub follows: bool,
    pub return_to: Option<String>,
    /// [`crate::El::press_focuses_below`].
    pub below: bool,
    /// `on_step`: a slider's arrow and page keys.
    pub step: Option<StepFn>,
    /// `on_step_limit`: a slider's Home and End.
    pub step_limit: Option<StepLimitFn>,
    /// `on_key_up`.
    pub key_up: Option<InputKeyFn>,
    /// `El::on_key`: keys while this element or one inside it has focus.
    pub key: Option<InputKeyFn>,
}

impl crate::cold::Empty for FocusSpec {
    const EMPTY: &'static Self = &FocusSpec {
        physical: None,
        enabled: false,
        opt_out: false,
        autofocus: false,
        order: 0,
        group: false,
        selected: false,
        reveal_selected: false,
        preserve_on_press: crate::runtime::selectable::OnPress { focus: false, selection: false },
        ring: None,
        within: false,
        ring_off: false,
        flat_within: false,
        default: false,
        highlight: None,
        fill: None,
        follows: false,
        return_to: None,
        below: false,
        step: None,
        step_limit: None,
        key_up: None,
        key: None,
    };
}

#[derive(Clone)]
pub(crate) struct ModalSpec {
    pub dismiss: PressFn,
    pub outside: bool,
}

impl El {
    /// Keys while this element, or a control inside it, has focus: `f` sees each key before
    /// focus navigation (arrows in a group, Return and Space presses), the default button and
    /// the window's `on_key`, and returns true to consume it. Keys typed into a focused text
    /// field stay the field's. Nested handlers run innermost first.
    ///
    /// With `focusable(true)`, this is a control that owns its keyboard, like a virtualised
    /// list whose rows aren't all built: it keeps focus while its rows scroll in and out, and a
    /// click anywhere inside it focuses it (an NSTableView).
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, &crate::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.cold_mut().focus.key = Some(Rc::new(move |s, key, tx| f(downcast::<S>(s), key, tx)));
        self
    }
    /// Opt into keyboard and pointer focus. Disabled controls must set this false, which also
    /// keeps a pressable control out of the Tab order under [`Cx::web_focus`](crate::Cx::web_focus).
    pub fn focusable(mut self, enabled: bool) -> Self {
        // Recorded even on a bare element: under `Cx::web_focus` it keeps a pressable control
        // out of the Tab order.
        let focus = &mut self.cold_mut().focus;
        focus.enabled = enabled;
        focus.opt_out = !enabled;
        self
    }
    /// Positive overrides precede document order (0); negative values skip Tab only.
    pub fn tab_index(mut self, order: i32) -> Self {
        if order != 0 || !self.cold.focus.is_empty() {
            self.cold_mut().focus.order = order;
        }
        self
    }
    /// Arrow keys rove among focusable descendants; Tab visits only the current/selected item.
    /// Groups with role `menu`, `listbox`, `list` or `tree` move on Up and Down only; other
    /// groups (segments, tabs, radios, toolbars) move on Left and Right as well. Home and End
    /// jump to the ends. `list`, `tree` and `grid` stop at their ends, as NSTableView does;
    /// menus, popups and segments wrap.
    pub fn focus_group(mut self) -> Self {
        self.cold_mut().focus.group = true;
        self
    }
    pub fn focus_selected(mut self, selected: bool) -> Self {
        if selected || !self.cold.focus.is_empty() {
            self.cold_mut().focus.selected = selected;
        }
        self
    }
    /// On a virtualized stack, reveal its selected row when the selected key changes,
    /// even while another control owns keyboard focus. Frames dispatching `on_hover`
    /// do not scroll, so hover-driven selection does not move the row under the pointer.
    /// Off by default; useful for suggestions controlled by an outside text field.
    pub fn reveal_selected(mut self, enabled: bool) -> Self {
        if enabled || !self.cold.focus.is_empty() {
            self.cold_mut().focus.reveal_selected = enabled;
        }
        self
    }
    /// A press on this element or its unfocusable descendants leaves the current field
    /// focused instead of blurring it. Works on any element, not only popups: a list row, a
    /// button, a toolbar. The press still reaches `on_press` and the other handlers, and a
    /// scroll still cancels it as usual. Focusable descendants still take focus normally.
    pub fn preserve_focus_on_press(mut self, enabled: bool) -> Self {
        if enabled || !self.cold.focus.is_empty() {
            self.cold_mut().focus.preserve_on_press.focus = enabled;
        }
        self
    }
    /// On a focus group: arrow keys and type-select also press the item they focus, so
    /// selection follows focus (a source list). Radio and tab items always do.
    pub fn focus_follows(mut self) -> Self {
        self.cold_mut().focus.follows = true;
        self
    }
    /// A menu or list row's highlight: painted while focused from the keyboard or pointer,
    /// and moving the pointer over the row focuses it, so keys and mouse share one
    /// highlight (NSMenu). Use instead of `hover` and `focus_ring`.
    pub fn focus_highlight(mut self, color: Color) -> Self {
        self.cold_mut().focus.highlight = Some(color);
        self
    }
    /// A fill laid over this element while it shows keyboard focus, as CSS
    /// `:focus-visible { background }` paints one: a list row that Tab reaches. Unlike
    /// [`El::focus_highlight`] the pointer neither paints it nor moves focus. A row that shows
    /// focus by its fill alone also sets [`El::no_focus_ring`].
    pub fn focus_fill(mut self, color: Color) -> Self {
        self.cold_mut().focus.fill = Some(color);
        self
    }
    /// The window's default button, which Return presses (AppKit's `keyEquivalent = "\r"`).
    /// Return goes to it from anywhere in the window, or the open modal, unless the focused
    /// element takes Return itself: a text field with `on_submit`, a link, or a row of a
    /// `menu`, `listbox`, `list`, `tree` or `grid` group or a `focus_highlight` row, which
    /// Return presses. With several, the last one shown wins (a
    /// sheet's over its window's). It needs an `on_press`, and stays out of reach while hidden
    /// or behind a modal. A focused button presses on Space, and passes Return on to this.
    pub fn default_button(mut self) -> Self {
        self.cold_mut().focus.default = true;
        self
    }
    /// A slider's keyboard, as NSSlider's: while this element is focused, Left and Down call
    /// `f` with -1, Right and Up with +1, Page Down with -10 and Page Up with +10, repeating
    /// while held. `f` moves the value by that many of its steps and clamps it. The element
    /// becomes focusable, and its role `slider` unless it has another. Home and End go to the
    /// ends with [`El::on_step_limit`].
    pub fn on_step<S: 'static>(mut self, f: impl Fn(&mut S, i32, &mut Tx) + 'static) -> Self {
        self.cold_mut().focus.step = Some(Rc::new(move |s, delta, tx| f(downcast::<S>(s), delta, tx)));
        self.cold_mut().focus.enabled = true;
        if self.role == RoleTag::GROUP {
            self.role = RoleTag::SLIDER;
        }
        self
    }
    /// A slider's Home and End (see [`El::on_step`]): `f` gets `false` for Home, which goes to
    /// the minimum, and `true` for End, the maximum. Without it they scroll, as elsewhere.
    pub fn on_step_limit<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.cold_mut().focus.step_limit = Some(Rc::new(move |s, max, tx| f(downcast::<S>(s), max, tx)));
        self
    }
    /// Return focus to this keyed trigger (or its first focusable child) when removed.
    pub fn focus_return(mut self, trigger: impl Into<String>) -> Self {
        self.cold_mut().focus.return_to = Some(trigger.into());
        self
    }
    /// A press on this layer (the scrim under a context menu) focuses what a press there would
    /// focus without it: the focusable scroll region under the pointer, as a web page's press
    /// outside a menu focuses the `tabIndex` scroller it lands on, else nothing.
    pub fn press_focuses_below(mut self) -> Self {
        self.cold_mut().focus.below = true;
        self
    }
    /// The control a [`El::modal`] focuses when it opens, as HTML's `autofocus` attribute in a
    /// dialog: a Cancel button, say. Without one the modal focuses its first text field, else
    /// its first Tab stop, or itself under [`Cx::web_focus`](crate::Cx::web_focus).
    pub fn autofocus(mut self) -> Self {
        self.cold_mut().focus.autofocus = true;
        self
    }
    /// Keyboard-only ring, on this element or a visual child of the focused control. A
    /// control with no ring of its own draws the default one (see [`El::no_focus_ring`]).
    pub fn focus_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.cold_mut().focus.ring = Some(Outline { width, offset, color });
        self.cold_mut().focus.within = false;
        self.cold_mut().focus.ring_off = false;
        self
    }
    /// A ring drawn while focus is on this element or anywhere inside it, whether the pointer or
    /// the keyboard put it there, as CSS `:focus-within` draws a text field's box: e.g.
    /// `focus_within_ring(1.0, -1.0, c)` for `outline: 1px solid; outline-offset: -1px`, a ring
    /// just inside the border box. It isn't a Tab stop by itself.
    pub fn focus_within_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.cold_mut().focus.ring = Some(Outline { width, offset, color });
        self.cold_mut().focus.within = true;
        self.cold_mut().focus.ring_off = false;
        self
    }
    /// While focus is on this element or anywhere inside it, from the pointer or the keyboard,
    /// it draws none of its [`El::box_shadow`]s and no [`El::hover`] fill, as a CSS field whose
    /// `:focus-within` rule sets `box-shadow: none` and whose hover rule is
    /// `:not(:focus-within)`: the [`El::focus_within_ring`] stands in for its raised edge.
    pub fn focus_within_flat(mut self) -> Self {
        self.cold_mut().focus.flat_within = true;
        self
    }
    /// No focus ring: not the default one, and not one set with [`El::focus_ring`]. A control
    /// that shows focus its own way (a fill, an outline it animates) sets it on itself or on
    /// the child that would draw the ring.
    ///
    /// Without it, a `focusable` element focused from the keyboard draws the default ring when
    /// neither it nor a child (short of another focusable) sets `focus_ring`: the view's
    /// [`Cx::focus_ring`](crate::Cx::focus_ring), else AppKit's, 3 points wide in
    /// [`Cx::focus_color`](crate::Cx::focus_color), just outside its border box, following
    /// its corner radii. Text fields, focused links, `focus_highlight` rows and the items of a `menu`,
    /// `listbox`, `list`, `tree` or `grid` group draw none, as in AppKit: their caret,
    /// highlight or selection shows focus.
    pub fn no_focus_ring(mut self) -> Self {
        self.cold_mut().focus.ring = None;
        self.cold_mut().focus.within = false;
        self.cold_mut().focus.ring_off = true;
        self
    }
    /// Trap focus and make the background inert while mounted. Escape requests dismissal.
    pub fn modal<S: 'static>(mut self, click_outside: bool, dismiss: impl Fn(&mut S) + 'static) -> Self {
        self.handlers.modal = Some(ModalSpec { outside: click_outside,
            dismiss: Rc::new(move |s, _| dismiss(downcast::<S>(s))) });
        self
    }
    /// [`El::modal`] whose dismissal gets the change's [`Tx`]: a dialog that closes at once
    /// (`tx.instant()`), say.
    pub fn modal_tx<S: 'static>(mut self, click_outside: bool, dismiss: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.modal = Some(ModalSpec { outside: click_outside,
            dismiss: Rc::new(move |s, tx| dismiss(downcast::<S>(s), tx)) });
        self
    }
    pub fn modal_click_outside(mut self, enabled: bool) -> Self {
        if let Some(h) = self.handlers.get_mut_if_some() && let Some(m) = &mut h.modal { m.outside = enabled; }
        self
    }
}
