//! What `El` holds for focus and keys: `FocusSpec`, kept behind `Cold` because most elements set none of it.

use std::rc::Rc;

use crate::color::Color;
use crate::el::{El, InputKeyFn, Outline, PressFn, RoleTag, SetValueFn, StepFn, StepLimitFn, Tx};
use crate::states::downcast;

#[derive(Clone)]
pub(crate) struct FocusSpec {
    pub physical: Option<crate::input::physical_input::Handler>,
    pub enabled: bool,
    /// `focusable(false)`: not a Tab stop even where [`crate::Cx::web_focus`] would make it one.
    pub opt_out: bool,
    /// [`crate::El::keyboard_focus_only`]: Tab, modals and assistive tech can focus it, not pointer clicks.
    pub keyboard_only: bool,
    /// [`crate::El::autofocus`].
    pub autofocus: bool,
    pub order: i32,
    pub group: bool,
    pub selected: bool,
    pub reveal_selected: bool,
    pub preserve_on_press: crate::runtime::selectable::OnPress,
    pub ring: Option<Outline>,
    /// [`crate::El::focus_ring_radius`]: explicit outer corner radius in points for this element's focus ring.
    pub ring_radius: Option<f32>,
    /// `focus_within_ring`: `ring` shows while focus is on this element or inside it, from
    /// the pointer or the keyboard.
    pub within: bool,
    /// `no_focus_ring`: no default ring on this control, and none from this element.
    pub ring_off: bool,
    /// `focus_within_flat`: no outer or inset shadows and no hover fill while focus is on this
    /// element or inside it.
    pub flat_within: bool,
    /// `focus_visible_within_ring`: the within ring shows only while the keyboard put focus there.
    pub within_keyboard: bool,
    /// [`crate::El::focus_ring_for`]: `ring` shows while the keyboard has the element with this key focused.
    pub ring_for: Option<Rc<str>>,
    /// `default_button`: Return presses it.
    pub default: bool,
    /// [`crate::El::press_on_return`]: Return presses this element while it has focus.
    pub press_on_return: bool,
    pub highlight: Option<Color>,
    /// `focus_fill`: painted over the element while it shows keyboard focus.
    pub fill: Option<Color>,
    pub follows: bool,
    pub return_to: Option<String>,
    /// [`crate::El::press_focuses_below`].
    pub below: bool,
    /// [`crate::El::hover_passes_below`].
    pub hover_below: bool,
    /// `on_set_value`: a range control's absolute value from assistive technology.
    pub set_value: Option<SetValueFn>,
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
        keyboard_only: false,
        autofocus: false,
        order: 0,
        group: false,
        selected: false,
        reveal_selected: false,
        preserve_on_press: crate::runtime::selectable::OnPress { focus: false, selection: false },
        ring: None,
        ring_radius: None,
        within: false,
        ring_off: false,
        flat_within: false,
        within_keyboard: false,
        ring_for: None,
        default: false,
        press_on_return: false,
        highlight: None,
        fill: None,
        follows: false,
        return_to: None,
        below: false,
        hover_below: false,
        set_value: None,
        step: None,
        step_limit: None,
        key_up: None,
        key: None,
    };
}

#[derive(Clone)]
pub(crate) struct ModalSpec {
    pub dismiss: Option<PressFn>,
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
    /// Reachable via Tab, modal initial focus, [`Tx::focus`](crate::Tx::focus) and assistive
    /// technology (`Focus`), while pointer clicks run `on_press` without focusing this element
    /// (as AppKit's `NSButton` and Safari's buttons do).
    ///
    /// For focus, a click on this element acts like a click on plain content: it blurs a focused
    /// control or focuses a qualifying ancestor (`on_key` or focusable scroll region), unless
    /// [`El::preserve_focus_on_press`] is also set.
    ///
    /// ```
    /// use rune::{El, el};
    ///
    /// struct App { runs: usize }
    /// let _btn: El = el()
    ///     .role("button")
    ///     .label("Run")
    ///     .focusable(true)
    ///     .keyboard_focus_only()
    ///     .on_press(|s: &mut App| s.runs += 1);
    /// ```
    pub fn keyboard_focus_only(mut self) -> Self {
        self.cold_mut().focus.keyboard_only = true;
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
    /// On an [`El::virtualize`] stack with eager child rows, reveal its [`El::focus_selected`]
    /// child row when the selected key changes, even while another control owns keyboard focus.
    /// Frames dispatching `on_hover` do not scroll, so hover-driven selection does not move the
    /// row under the pointer. Off by default; useful for short lists such as suggestions
    /// controlled by an outside text field.
    ///
    /// Only inspects eager child rows of [`El::virtualize`]: on an [`El::virtual_rows`] list it
    /// does nothing (debug builds warn once per list). For [`El::virtual_rows`], reveal rows from
    /// selection-moving handlers via [`Tx::scroll_into_view`](crate::Tx::scroll_into_view) or
    /// [`Tx::scroll_into_view_in`](crate::Tx::scroll_into_view_in) (or
    /// [`Tx::scroll_center`](crate::Tx::scroll_center) /
    /// [`Tx::scroll_key_to`](crate::Tx::scroll_key_to)).
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
    /// Return presses this element while it has focus (as Space does), without enabling
    /// [`Cx::web_focus`](crate::Cx::web_focus) window-wide.
    ///
    /// Rune follows AppKit by default: Return goes to the window's [`El::default_button`], while
    /// Space presses the focused control. Set this on a focusable control with `on_press` that
    /// should also activate on Return (matching the WAI-ARIA Button pattern, GTK and Win32).
    /// While this element has focus, Return activates it ahead of any [`El::default_button`] and
    /// ignores key repeats.
    ///
    /// ```
    /// use rune::{El, el};
    ///
    /// struct App { count: usize }
    /// let _btn: El = el()
    ///     .role("button")
    ///     .label("Apply")
    ///     .focusable(true)
    ///     .press_on_return()
    ///     .on_press(|s: &mut App| s.count += 1);
    /// ```
    pub fn press_on_return(mut self) -> Self {
        self.cold_mut().focus.press_on_return = true;
        self
    }
    /// A slider's or spin button's keyboard, as NSSlider's: while this element is focused, Left
    /// and Down call `f` with -1, Right and Up with +1, Page Down with -10 and Page Up with +10,
    /// repeating while held. On a text field, Left and Right stay with the caret while Up, Down,
    /// Page Up and Page Down step. `f` moves the value by that many of its steps and clamps it.
    /// The element becomes focusable, and its role `slider` unless it has another. Home and End
    /// go to the ends with [`El::on_step_limit`].
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
    /// Sets a range control's numeric value when assistive technology invokes AccessKit's numeric
    /// `SetValue` action, as a browser sets `<input type="range">` directly to the requested
    /// number.
    ///
    /// When [`El::range_value`] sets finite ordered bounds (`min <= max`), the requested value is
    /// clamped to `[min, max]` before `f` runs, so `f` receives the target and snaps it to its
    /// own grid (if any); non-finite targets and unusable range bounds are ignored. Works with or
    /// without [`El::range_step`]. When both `on_set_value` and [`El::on_step`] are set,
    /// `SetValue` calls `on_set_value` while `Increment`, `Decrement` and keyboard stepping still
    /// call `on_step`.
    pub fn on_set_value<S: 'static>(mut self, f: impl Fn(&mut S, f64, &mut Tx) + 'static) -> Self {
        self.cold_mut().focus.set_value = Some(Rc::new(move |s, value, tx| f(downcast::<S>(s), value, tx)));
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
    /// Hover passes through this layer to what lies under it (the dismiss layer under a menu
    /// that isn't modal): the layer still takes its presses, and the elements under the pointer
    /// keep their hover styles, hover groups and `on_hover_change`, as a web page's elements
    /// keep `:hover` beside an open context menu, which puts nothing over them.
    pub fn hover_passes_below(mut self) -> Self {
        self.cold_mut().focus.hover_below = true;
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
        self.cold_mut().focus.within_keyboard = false;
        self.cold_mut().focus.ring_off = false;
        self.cold_mut().focus.ring_for = None;
        self
    }
    /// Explicit outer corner radius in points for this element's focus ring, independent of its
    /// own [`El::radius`] or [`El::corners`].
    ///
    /// When unset, a focus ring derives concentric radii from the element's own corner radii plus
    /// its offset and width. Setting `focus_ring_radius` overrides that for any focus ring painted
    /// on this element (the default ring, [`El::focus_ring`], [`El::focus_within_ring`],
    /// [`El::focus_visible_within_ring`] and [`El::focus_ring_for`]), while leaving
    /// [`El::outline`] unaffected. Negative and non-finite values clamp to `0.0`, and
    /// [`El::continuous_corners`] applies to positive radii.
    ///
    /// ```
    /// use rune::{Color, El, el};
    ///
    /// let _row: El = el()
    ///     .size(160.0, 32.0)
    ///     .radius(4.0)
    ///     .focusable(true)
    ///     .focus_ring(2.0, 2.0, Color { r: 0.0, g: 0.62, b: 1.0, a: 1.0 })
    ///     .focus_ring_radius(8.0);
    /// ```
    pub fn focus_ring_radius(mut self, radius: f32) -> Self {
        self.cold_mut().focus.ring_radius = Some(if radius.is_finite() { radius.max(0.0) } else { 0.0 });
        self
    }
    /// A ring drawn while focus is on this element or anywhere inside it, whether the pointer or
    /// the keyboard put it there, as CSS `:focus-within` draws a text field's box: e.g.
    /// `focus_within_ring(1.0, -1.0, c)` for `outline: 1px solid; outline-offset: -1px`, a ring
    /// just inside the border box. It isn't a Tab stop by itself.
    pub fn focus_within_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.cold_mut().focus.ring = Some(Outline { width, offset, color });
        self.cold_mut().focus.within = true;
        self.cold_mut().focus.within_keyboard = false;
        self.cold_mut().focus.ring_off = false;
        self.cold_mut().focus.ring_for = None;
        self
    }
    /// [`El::focus_within_ring`] that shows only while the keyboard put focus on this element or
    /// inside it (a Tab, an arrow, `Tx::focus` with a ring), as a web field's own keyboard-focus
    /// ring does: not for a click, a focus handed over without a ring, or the window coming back
    /// to the front with focus where it was. A text field's frame (a prompt's) takes it.
    pub fn focus_visible_within_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self = self.focus_within_ring(width, offset, color);
        self.cold_mut().focus.within_keyboard = true;
        self
    }
    /// A keyboard-only ring drawn on this element while the element keyed `key` (the last part of
    /// its path) has focus, as a CSS sibling's `:has(:focus-visible) ~ ::after` ring: a scroll
    /// region's ring drawn by an overlay painted after the header and the dock that cover the
    /// region's own edges. Give the focused element [`El::no_focus_ring`]. This element isn't a
    /// Tab stop by itself.
    pub fn focus_ring_for(mut self, key: impl Into<String>, width: f32, offset: f32, color: Color) -> Self {
        self = self.focus_ring(width, offset, color);
        self.cold_mut().focus.ring_for = Some(Rc::from(key.into()));
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
            dismiss: Some(Rc::new(move |s, _| dismiss(downcast::<S>(s)))) });
        self
    }
    /// [`El::modal`] whose dismissal gets the change's [`Tx`]: a dialog that closes at once
    /// (`tx.instant()`), say.
    pub fn modal_tx<S: 'static>(mut self, click_outside: bool, dismiss: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.modal = Some(ModalSpec { outside: click_outside,
            dismiss: Some(Rc::new(move |s, tx| dismiss(downcast::<S>(s), tx))) });
        self
    }
    /// Trap focus and make the background inert while mounted, with no Escape or outside-click
    /// dismissal (as HTML's `<dialog closedby="none">`): the modal's own controls close it.
    pub fn modal_trap(mut self) -> Self {
        self.handlers.modal = Some(ModalSpec { outside: false, dismiss: None });
        self
    }
    pub fn modal_click_outside(mut self, enabled: bool) -> Self {
        if let Some(h) = self.handlers.get_mut_if_some() && let Some(m) = &mut h.modal { m.outside = enabled; }
        self
    }
}
