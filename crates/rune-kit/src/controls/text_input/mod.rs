//! TextInput and Textarea (`@opencode/ui` forms/text-input, forms/textarea), and the older
//! TextField (`components/text-field`).

use rune::{Color, El, Icon, RichStyle, Span, TextEdit, TextStyle, Tx, el, icon, row, text_input};

use crate::{Acts, Kit, State};

pub(crate) fn kit_rich_style(k: &Kit) -> RichStyle {
    RichStyle {
        link: k.c.text_accent,
        code_fg: None,
        code_bg: k.c.layer_03,
        code_block_bg: k.c.layer_02,
        quote_bar: k.c.border_strong,
        marker: Some(k.c.text_muted),
    }
}

/// Shared shell painting (text-input.css:5-75, textarea.css): the base fill, input elevation,
/// hover overlay, a 1 point focus or invalid ring just inside the box in place of the shadow
/// (or the kit's own radius and ring, `Kit::metrics.field`), disabled dimming.
pub(crate) fn shell(k: &Kit, e: El, state: State, invalid: bool, focused: bool) -> El {
    let c = &k.c;
    let m = k.field_metrics();
    let hover = state == State::Hover && !invalid && !focused;
    let e = k.hover_surface(
        e.radius(m.radius).token_motion(k.motion.control),
        c.bg_base,
        c.hover,
        hover,
        state == State::Rest && !invalid && !focused,
    );
    if state == State::Disabled {
        return k.disabled_dim(k.elevate(e, k.elevation.input()), true);
    }
    if let Some(highlight) = k.forced {
        let e = if invalid { e.border(2.0, c.text_base).border_dash(4.0, 3.0) } else { e.border(1.0, c.text_base) };
        return if focused || state == State::Focus { e.outline(2.0, 2.0, highlight) } else { e };
    }
    if invalid {
        e.outline(m.focus_width, m.focus_offset, c.danger.fg)
    } else if focused || state == State::Focus {
        e.outline(m.focus_width, m.focus_offset, c.border_focus)
    } else {
        k.elevate(e, k.elevation.input())
    }
}


/// The rich-text builders every field forwards to its inner text element.
macro_rules! rich_forwarders {
    () => {
        pub fn spans(mut self, spans: impl Into<Vec<Span>>) -> Self {
            let spans = spans.into();
            self.edits.visual(move |e| e.spans(spans));
            self
        }
        pub fn decorations(self, spans: impl Into<Vec<Span>>) -> Self {
            self.spans(spans)
        }
        pub fn rich_style(mut self, style: RichStyle) -> Self {
            self.edits.visual(move |e| e.rich_style(style));
            self
        }
        /// See [`El::line_fill`](rune::El::line_fill).
        pub fn line_fill(mut self, fill: rune::LineFill) -> Self {
            self.edits.visual(move |e| e.line_fill(fill));
            self
        }
        /// See [`El::line_fills`](rune::El::line_fills).
        pub fn line_fills(mut self, fills: impl IntoIterator<Item = rune::LineFill>) -> Self {
            let fills: Vec<rune::LineFill> = fills.into_iter().collect();
            self.edits.visual(move |e| e.line_fills(fills));
            self
        }
        /// See [`El::pill_icon`](rune::El::pill_icon).
        pub fn pill_icon(mut self, icon: impl Fn(&str) -> Option<&'static Icon> + 'static) -> Self {
            self.edits.visual(move |e| e.pill_icon(icon));
            self
        }
        /// See [`El::pill_image`](rune::El::pill_image).
        pub fn pill_image(mut self, image: impl Fn(&str) -> Option<rune::Pixels> + 'static) -> Self {
            self.edits.visual(move |e| e.pill_image(image));
            self
        }
        /// See [`El::pill_inline_image`](rune::El::pill_inline_image).
        pub fn pill_inline_image(mut self, image: impl Fn(&str) -> Option<rune::InlineImage> + 'static) -> Self {
            self.edits.visual(move |e| e.pill_inline_image(image));
            self
        }
        /// See [`El::pill_style`](rune::El::pill_style).
        pub fn pill_style(mut self, style: impl Fn(&str) -> rune::PillStyle + 'static) -> Self {
            self.edits.visual(move |e| e.pill_style(style));
            self
        }
        /// See [`El::pill_colors`](rune::El::pill_colors).
        pub fn pill_colors(mut self, bg: Color, fg: Option<Color>) -> Self {
            self.edits.visual(move |e| e.pill_colors(bg, fg));
            self
        }
        /// See [`El::pill_copy_text`](rune::El::pill_copy_text).
        pub fn pill_copy_text(mut self, f: impl Fn(&str) -> Option<String> + 'static) -> Self {
            self.edits.visual(move |e| e.pill_copy_text(f));
            self
        }
        /// Virtual keyboard layout and input mode hint ([`El::input_mode`](rune::El::input_mode)).
        pub fn input_mode(mut self, mode: rune::InputMode) -> Self {
            self.edits.visual(move |e| e.input_mode(mode));
            self
        }
        /// Browser and OS autofill hint ([`El::autofill`](rune::El::autofill), HTML `autocomplete`).
        pub fn autofill(mut self, hint: rune::Autofill) -> Self {
            self.edits.visual(move |e| e.autofill(hint));
            self
        }
    };
}

/// Secondary-press and paste hooks every field forwards to its inner editor (skipped when disabled).
macro_rules! field_hook_forwarders {
    ($ctor:literal) => {
        /// Secondary press (right-click, control-click, Menu, Shift+F10) on the inner editor in
        /// place of the default field menu (`El::on_secondary_press`). Skipped when disabled.
        /// ```
        /// # use rune::{PointerPress, TextEdit};
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit, menu: Option<PointerPress> }
        /// # let s = App { edit: TextEdit::default(), menu: None };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_secondary_press(|s: &mut App, p, _| s.menu = Some(p)).build(&k)")]
        /// # ;
        /// ```
        pub fn on_secondary_press<S: 'static>(mut self, f: impl Fn(&mut S, rune::PointerPress, &mut Tx) + 'static) -> Self {
            self.edits.secondary_press(f);
            self
        }
        /// Intercept pastes into the focused inner editor (`El::on_input_paste`): return `true` to
        /// consume or `false` for default plain-text insertion. Skipped when disabled.
        /// ```
        /// # use rune::TextEdit;
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit }
        /// # let s = App { edit: TextEdit::default() };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_input_paste(|s: &mut App, c, _| !c.files().is_empty()).build(&k)")]
        /// # ;
        /// ```
        pub fn on_input_paste<S: 'static>(mut self, f: impl Fn(&mut S, &rune::ClipboardContent, &mut Tx) -> bool + 'static) -> Self {
            self.edits.paste(f);
            self
        }
        /// Intercept Copy and Cut on the focused inner editor (`El::on_input_copy`) across both
        /// keyboard shortcuts and the context menu. Skipped when disabled.
        /// ```
        /// # use rune::TextEdit;
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit }
        /// # let s = App { edit: TextEdit::default() };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_input_copy(|s: &mut App, _cut, _| Some(rune::ClipboardContent::new(s.edit.selected()))).build(&k)")]
        /// # ;
        /// ```
        pub fn on_input_copy<S: 'static>(
            mut self,
            f: impl Fn(&mut S, bool, &mut Tx) -> Option<rune::ClipboardContent> + 'static,
        ) -> Self {
            self.edits.copy(f);
            self
        }
        /// Called after the inner editor's caret or selection moves from keys, clicks, drags, or
        /// edits ([`El::on_input_selection`](rune::El::on_input_selection)), in the same turn
        /// before the view rebuilds. Skipped when disabled.
        /// ```
        /// # use rune::TextEdit;
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit, sel: (usize, usize) }
        /// # let s = App { edit: TextEdit::default(), sel: (0, 0) };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_input_selection(|s: &mut App, _| s.sel = (s.edit.anchor, s.edit.focus)).build(&k)")]
        /// # ;
        /// ```
        pub fn on_input_selection<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
            self.edits.selection(f);
            self
        }
        /// Primary press (left-click or tap) on a [`Mark::Pill`](rune::Mark::Pill) chip inside
        /// the inner editor ([`El::on_pill_press`](rune::El::on_pill_press)): return `true` to
        /// consume before caret placement, or `false` to fall through. Skipped when disabled.
        /// ```
        /// # use rune::TextEdit;
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit, picked: Option<String> }
        /// # let s = App { edit: TextEdit::default(), picked: None };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_pill_press(|s: &mut App, id, _, _| { s.picked = Some(id.to_owned()); true }).build(&k)")]
        /// # ;
        /// ```
        pub fn on_pill_press<S: 'static>(
            mut self,
            f: impl Fn(&mut S, &str, rune::PointerPress, &mut Tx) -> bool + 'static,
        ) -> Self {
            self.edits.pill_press(f);
            self
        }
        /// Secondary press (right-click or macOS control-click) on a
        /// [`Mark::Pill`](rune::Mark::Pill) chip inside the inner editor
        /// ([`El::on_pill_secondary`](rune::El::on_pill_secondary)): return `true` to consume or
        /// `false` to fall through to the field menu. Skipped when disabled.
        /// ```
        /// # use rune::{PointerPress, TextEdit};
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit, menu: Option<(String, PointerPress)> }
        /// # let s = App { edit: TextEdit::default(), menu: None };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_pill_secondary(|s: &mut App, id, p, _| { s.menu = Some((id.to_owned(), p)); true }).build(&k)")]
        /// # ;
        /// ```
        pub fn on_pill_secondary<S: 'static>(
            mut self,
            f: impl Fn(&mut S, &str, rune::PointerPress, &mut Tx) -> bool + 'static,
        ) -> Self {
            self.edits.pill_secondary(f);
            self
        }
        /// Called when the focused inner editor loses focus: the window losing focus (another
        /// app, a hidden window), Tab, a click elsewhere or `Tx::focus` moving it off, so a form
        /// can commit a half-typed value. Use [`Self::on_focus_change`] for in-window focus gain
        /// and loss that stays silent on window deactivation and return. Return stays
        /// [`Self::on_submit`]. An open IME composition is finished first. Not called for a field
        /// removed from the view while focused. Skipped when disabled. Keys are untouched: held
        /// modifiers still release ([`El::on_focus_lost`](rune::El::on_focus_lost)).
        /// ```
        /// # use rune::TextEdit;
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit, saved: String }
        /// # impl App { fn commit(&mut self) { self.saved = self.edit.text.clone(); } }
        /// # let s = App { edit: TextEdit::default(), saved: String::new() };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_submit(|s: &mut App, _| s.commit()).on_blur(|s: &mut App, _| s.commit()).build(&k)")]
        /// # ;
        /// ```
        pub fn on_blur<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
            self.edits.blur(f);
            self
        }
        /// Called with `true` when the inner editor gains focus and `false` when focus leaves it
        /// inside the window: Tab, pointer clicks, assistive technology, programmatic
        /// [`Tx::focus`](rune::Tx::focus)/[`Tx::blur`](rune::Tx::blur), modal autofocus/return,
        /// and disabling or removing the field while focused
        /// ([`El::on_focus_change`](rune::El::on_focus_change)). Stays silent on window
        /// deactivation and return (use [`Self::on_blur`] when window deactivation should also
        /// notify). Read [`Tx::focus_visible`](rune::Tx::focus_visible) inside the callback to
        /// distinguish keyboard focus from pointer focus. Skipped when disabled.
        /// ```
        /// # use rune::TextEdit;
        /// # use rune_kit::{Kit, TextArea, TextField, TextInput};
        /// # struct App { edit: TextEdit, active: bool }
        /// # let s = App { edit: TextEdit::default(), active: false };
        /// # let k = Kit::neutral(true);
        /// # let _ =
        #[doc = concat!($ctor, ".on_edit(|s: &mut App| &mut s.edit).on_focus_change(|s: &mut App, focused, _| s.active = focused).build(&k)")]
        /// # ;
        /// ```
        pub fn on_focus_change<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
            self.edits.focus_change(f);
            self
        }
    };
}

mod area;
mod field;

pub use area::TextArea;
pub use field::TextField;

/// Edit bindings collected for the inner text field.
#[derive(Default)]
struct EditActs {
    handlers: Vec<Box<dyn FnOnce(El) -> El>>,
    visual: Vec<Box<dyn FnOnce(El) -> El>>,
}

impl EditActs {
    fn edit<S: 'static>(&mut self, f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_edit(f)));
    }
    fn submit<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_submit(f)));
    }
    fn key<S: 'static>(&mut self, f: impl Fn(&mut S, &rune::KeyEvent, &mut Tx) -> bool + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_input_key(f)));
    }
    fn change<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_input_change(f)));
    }
    fn selection<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_input_selection(f)));
    }
    fn paste<S: 'static>(&mut self, f: impl Fn(&mut S, &rune::ClipboardContent, &mut Tx) -> bool + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_input_paste(f)));
    }
    fn copy<S: 'static>(&mut self, f: impl Fn(&mut S, bool, &mut Tx) -> Option<rune::ClipboardContent> + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_input_copy(f)));
    }
    fn pill_press<S: 'static>(&mut self, f: impl Fn(&mut S, &str, rune::PointerPress, &mut Tx) -> bool + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_pill_press(f)));
    }
    fn pill_secondary<S: 'static>(&mut self, f: impl Fn(&mut S, &str, rune::PointerPress, &mut Tx) -> bool + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_pill_secondary(f)));
    }
    fn blur<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_focus_lost(f)));
    }
    fn focus_change<S: 'static>(&mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_focus_change(f)));
    }
    fn secondary_press<S: 'static>(&mut self, f: impl Fn(&mut S, rune::PointerPress, &mut Tx) + 'static) {
        self.handlers.push(Box::new(move |e: El| e.on_secondary_press(f)));
    }
    fn visual(&mut self, f: impl FnOnce(El) -> El + 'static) {
        self.visual.push(Box::new(f));
    }
    fn apply(self, mut e: El, disabled: bool) -> El {
        for f in self.visual {
            e = f(e);
        }
        if !disabled {
            for f in self.handlers {
                e = f(e);
            }
        }
        e
    }
}

/// What assistive technology reads for a field's editor: its name (the label, else the
/// placeholder), a description, and whether it can be used.
pub(crate) fn name_field(e: El, label: Option<&str>, placeholder: &str, description: Option<&str>, disabled: bool) -> El {
    let e = match label.filter(|l| !l.is_empty()).or(Some(placeholder).filter(|p| !p.is_empty())) {
        Some(name) => e.label(name),
        None => e,
    };
    e.description(description.unwrap_or_default()).disabled(disabled)
}

/// A single-line field: `TextInput::new(&s.name).placeholder("Name").on_edit(|s: &mut S| &mut s.name)`.
///
/// For a visible label and wrapped help text, wrap it in [`crate::Field`] and pass `.width(None)`
/// so it fills the field (the default width is 280 pt). [`Self::label`] and [`Self::description`]
/// are accessibility-only and are not drawn; still pass `.label(..)` on the inner `TextInput` to
/// name the editor itself for assistive technology.
pub struct TextInput {
    edit: TextEdit,
    label: Option<String>,
    description: Option<String>,
    placeholder: String,
    large: bool,
    style: Option<TextStyle>,
    height: Option<f32>,
    width: Option<f32>,
    leading: Option<&'static Icon>,
    leading_size: f32,
    plain: bool,
    trailing: Option<El>,
    copy: bool,
    clear: bool,
    invalid: bool,
    read_only: bool,
    auto_complete: Option<rune::AutoComplete>,
    active: Option<String>,
    state: State,
    edits: EditActs,
    copy_acts: Acts,
}

impl TextInput {
    pub fn secure(mut self) -> Self {
        self.edit.set_secure(true);
        self
    }
    pub fn revealed(mut self, revealed: bool) -> Self {
        self.edit.set_revealed(revealed);
        self.edits.visual(move |e| e.revealed(revealed));
        self
    }
    /// Mark misspelled words and offer their suggestions in the field's menu (`El::spellcheck`).
    pub fn spellcheck(mut self, on: bool) -> Self {
        self.edits.visual(move |e| e.spellcheck(on));
        self
    }
    /// Scroll enclosing scroll viewports to bring this field's caret into view when it gains
    /// focus ([`El::reveal_caret_on_focus`](rune::El::reveal_caret_on_focus)). Off by default
    /// (`false`).
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextInput};
    /// # struct App { name: TextEdit }
    /// # let s = App { name: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextInput::new(&s.name)
    ///     .reveal_caret_on_focus(true)
    ///     .on_edit(|s: &mut App| &mut s.name)
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn reveal_caret_on_focus(mut self, on: bool) -> Self {
        self.edits.visual(move |e| e.reveal_caret_on_focus(on));
        self
    }
    pub fn animate_edit_layout(mut self) -> Self {
        self.edits.handlers.push(Box::new(El::animate_edit_layout));
        self
    }
    /// The value's direction (`El::dir`): `Direction::Auto` takes it from the text's first strong
    /// character, as `dir="auto"` on an `input` does, so a name in Arabic starts at the right in
    /// a left-to-right window. Defaults to the window's.
    pub fn dir(mut self, dir: rune::Direction) -> Self {
        self.edits.visual(move |e| e.dir(dir));
        self
    }
    pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.edits.change(f);
        self
    }
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, &rune::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.edits.key(f);
        self
    }
    pub fn new(edit: &TextEdit) -> TextInput {
        TextInput {
            edit: edit.clone(),
            label: None,
            description: None,
            placeholder: String::new(),
            large: false,
            style: None,
            height: None,
            width: Some(280.0),
            leading: None,
            leading_size: 16.0,
            plain: false,
            trailing: None,
            copy: false,
            clear: false,
            invalid: false,
            read_only: false,
            auto_complete: None,
            active: None,
            state: State::Rest,
            edits: EditActs::default(),
            copy_acts: Acts::default(),
        }
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// The accessible name ("Search", "Email"), when no visible label names the field. Without
    /// it the placeholder names it. Accessibility-only (not drawn); wrap in [`crate::Field`] for a
    /// visible label.
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// `appearance="large"`: 32 points tall.
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }
    /// Help or a validation error for the inner editor, separate from its name.
    /// Accessibility-only (not drawn); use [`crate::Field::prefix`] / [`crate::Field::suffix`] for
    /// visible wrapped help text.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
    /// The text's type, in place of `ui_compact` (13 / 16): typed text, placeholder, caret and
    /// selection all follow it. Unless [`Self::height`] sets one, the box grows to fit, to the
    /// style's line height plus 12 points if that's taller than 28 (32 with [`Self::large`]),
    /// e.g. `.text_style(TextStyle::new(17.0, 400.0).lh(24.0)).height(56.0)` for a launcher field.
    pub fn text_style(mut self, style: TextStyle) -> Self {
        self.style = Some(style);
        self
    }
    /// The box's height in points, in place of 28 (32 with [`Self::large`]) or the fit to
    /// [`Self::text_style`]. Clamped to at least the text style's line height.
    pub fn height(mut self, points: f32) -> Self {
        self.height = Some(points.max(1.0));
        self
    }
    /// Width in points (default 280); `None` fills the parent.
    pub fn width(mut self, w: Option<f32>) -> Self {
        self.width = w;
        self
    }
    pub fn leading_icon(mut self, i: &'static Icon) -> Self {
        self.leading = Some(i);
        self
    }
    /// The leading icon's side in points (default 16). The icon stays vertically centred in the
    /// box and the text's start moves with it.
    pub fn leading_icon_size(mut self, points: f32) -> Self {
        self.leading_size = points.max(1.0);
        self
    }
    /// A borderless field for sitting inside something else, such as a header: no fill, sheen,
    /// shadow, border, hover tint or focus outline, so the surrounding view is the context.
    /// Focus shows only as the caret and selection, and an invalid field only as red text.
    /// Everything else works as in the boxed field, including press-anywhere focus, the
    /// leading icon, trailing slot, copy and clear buttons, and the same inner padding (8 points
    /// before the leading icon or 2 before the text, 8 after the trailing end; the clear button
    /// reaches the right edge). Width and height still apply; use `.width(None)` to fill the
    /// parent. A disabled plain field is dimmed.
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }
    /// Trailing accessory element (e.g. a badge, hint, or reveal/copy buttons).
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing = Some(e);
        self
    }
    pub fn copy_button(mut self) -> Self {
        self.copy = true;
        self
    }
    pub fn clear_button(mut self) -> Self {
        self.clear = true;
        self
    }
    pub fn invalid(mut self, i: bool) -> Self {
        self.invalid = i;
        self
    }
    /// Make the field read-only ([`El::read_only`](rune::El::read_only)): keeps focus, caret,
    /// keyboard and pointer navigation/selection, and Copy while refusing typing, Cut, Paste,
    /// Undo/Redo, IME, and spellcheck edits. Pair with [`Self::on_edit`] so the runtime can
    /// move the caret and selection in your [`TextEdit`].
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextInput};
    /// # struct App { endpoint: TextEdit }
    /// # let s = App { endpoint: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextInput::new(&s.endpoint).read_only(true).on_edit(|s: &mut App| &mut s.endpoint).build(&k)
    /// # ;
    /// ```
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }
    /// How typing in this field suggests or completes values
    /// ([`El::auto_complete`](rune::El::auto_complete), ARIA `aria-autocomplete`). Leave unset
    /// for ARIA's implicit `none` default.
    ///
    /// ```
    /// # use rune::{AutoComplete, TextEdit};
    /// # use rune_kit::{Kit, TextInput};
    /// # let edit = TextEdit::default();
    /// # let k = Kit::neutral(true);
    /// let _ = TextInput::new(&edit)
    ///     .placeholder("Filter…")
    ///     .auto_complete(AutoComplete::List)
    ///     .build(&k);
    /// ```
    pub fn auto_complete(mut self, mode: rune::AutoComplete) -> Self {
        self.auto_complete = Some(mode);
        self
    }
    /// The suggestion this field's arrow keys highlight, by its key (`aria-activedescendant`):
    /// focus stays in the field and a screen reader reads the highlighted row.
    pub fn active_descendant(mut self, key: Option<impl Into<String>>) -> Self {
        self.active = key.map(Into::into);
        self
    }
    pub fn state(mut self, s: State) -> Self {
        self.state = s;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        if d {
            self.state = State::Disabled;
        }
        self
    }
    pub fn on_edit<S: 'static>(mut self, f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) -> Self {
        self.edits.edit(f);
        self
    }
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.edits.submit(f);
        self
    }
    /// The trailing copy or clear button's press.
    pub fn on_action<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.copy_acts.press_tx(f);
        self
    }
    field_hook_forwarders!("TextInput::new(&s.edit)");
    rich_forwarders!();

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let red = self.invalid && !disabled && (k.invalid_text || self.plain);
        let ink = if red { c.danger.fg } else { c.text_base };
        let hint = if red { c.danger.fg } else { c.text_faint };
        let style = self.style.unwrap_or(k.ty.ui_compact);
        let m = k.field_metrics();
        let base: f32 = if self.large { m.large_height } else { m.height };
        let height = self.height.map_or_else(|| base.max(style.line_px() + 12.0), |h| h.max(style.line_px()));
        let field = text_input(&self.edit, style)
            .single_line()
            .key("input")
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .color(ink)
            .rich_style(kit_rich_style(k))
            .placeholder(self.placeholder.clone(), hint)
            .caret(ink, k.selection());
        let mut field = name_field(field, self.label.as_deref(), &self.placeholder, self.description.as_deref(), disabled)
            .invalid(self.invalid)
            .read_only(self.read_only);
        if let Some(mode) = self.auto_complete {
            field = field.auto_complete(mode);
        }
        if let Some(key) = self.active.clone() {
            field = field.active_descendant(key);
        }
        let field = self.edits.apply(field, disabled);
        // The value stops at the box's padding: a placeholder or value longer than the box is cut
        // there, as an `input` cuts its own text, rather than running on to the border.
        let mut value = row().key("value").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().clip_rounded(0.0);
        if let Some(i) = self.leading {
            value = value.child(el().key("leading").pl(m.padding_x).center().child(icon(i, self.leading_size).color(c.icon_muted)).a11y_hidden());
        } else {
            // With the 6 point gap after it, the text starts `padding_x` in (6 at least).
            value = value.child(el().w((m.padding_x - 6.0).max(0.0)));
        }
        value = value.child(field);
        // The editor inside carries the name; the box only routes presses to it.
        let mut e = row()
            .role("textbox")
            .h(height)
            .pr(m.padding_x)
            .gap(8.0)
            .child(value);
        e = match self.width {
            Some(w) => e.w(w),
            None => e.w_full(),
        };
        if let Some(t) = self.trailing {
            e = e.child(row().key("trailing").self_center().center_y().child(t));
        }
        if (self.copy && !self.edit.is_secure()) || self.clear {
            let (glyph, side, radius) =
                if self.clear { (k.glyph.clear, 28.0, k.radius.md) } else { (k.glyph.copy, 20.0, k.radius.sm) };
            let mut b = row()
                .key("action")
                .role("button")
                .label(if self.clear { "Clear" } else { "Copy" })
                .size(side, side)
                .self_center()
                .center()
                .radius(radius)
                // Inside, clear of the field's edge it sits on (`text-input.css`, opencode#64909).
                .focus_ring(2.0, -2.0, c.border_focus)
                .child(icon(glyph, 16.0).color(c.icon_muted))
                .disabled(disabled);
            if self.clear {
                b = b.mr(-m.padding_x);
            }
            if !disabled {
                b = self.copy_acts.apply(k.interactive(b.hit()), true);
            }
            e = e.child(b);
        }
        if self.plain {
            // A contrast theme shows the ring a plain field (a palette's search) otherwise leaves
            // off: its look is a fill the theme doesn't keep apart (`search-list.css`).
            let e = match k.forced {
                Some(highlight) if self.edit.focused && !disabled => e.outline(2.0, 2.0, highlight),
                _ => e,
            };
            return k.disabled_dim(e, disabled);
        }
        shell(k, e, self.state, self.invalid && !disabled, self.edit.focused && !disabled)
    }
}
