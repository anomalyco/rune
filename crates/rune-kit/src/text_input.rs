//! TextInput and Textarea (`@opencode/ui` forms/text-input, forms/textarea), and the older
//! TextField (`components/text-field`).

use rune::{Color, El, Icon, RichStyle, Span, TextEdit, TextStyle, Tx, col, el, icon, row, text, text_input};

use crate::{Acts, Kit, State, over, shadows};

fn kit_rich_style(k: &Kit) -> RichStyle {
    RichStyle {
        link: k.c.text_accent,
        code_fg: None,
        code_bg: k.c.layer_03,
        code_block_bg: k.c.layer_02,
        quote_bar: k.c.border_strong,
        marker: Some(k.c.text_muted),
    }
}

/// Shared shell painting: sheen over the base, button elevation, hover overlay, focus/invalid
/// outline, disabled dimming.
fn shell(k: &Kit, e: El, sheen: [f32; 2], state: State, invalid: bool, focused: bool) -> El {
    let c = &k.c;
    let hover = state == State::Hover && !invalid && !focused;
    let mut e = e
        .radius(k.radius.md)
        .fill(if hover { over(c.hover, c.bg_base) } else { c.bg_base })
        .gradient(k.light(sheen[0]), k.light(sheen[1]))
        .motion(k.motion.control);
    if state == State::Disabled {
        return shadows(k, e, &k.elevation.button_neutral).dim(0.5);
    }
    if invalid {
        e = e.outline(1.0, 0.0, c.danger.fg);
    } else if focused || state == State::Focus {
        e = e.outline(1.0, 0.0, c.border_focus);
    } else {
        e = shadows(k, e, &k.elevation.button_neutral);
        if state == State::Rest {
            e = e.hover(c.hover);
        }
    }
    e
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
        /// See [`El::pill_icon`](rune::El::pill_icon).
        pub fn pill_icon(mut self, icon: impl Fn(&str) -> Option<&'static Icon> + 'static) -> Self {
            self.edits.visual(move |e| e.pill_icon(icon));
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
    };
}

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

/// A single-line field: `TextInput::new(&s.name).placeholder("Name").on_edit(|s: &mut S| &mut s.name)`.
pub struct TextInput {
    edit: TextEdit,
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
    pub fn animate_edit_layout(mut self) -> Self {
        self.edits.handlers.push(Box::new(El::animate_edit_layout));
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
            state: State::Rest,
            edits: EditActs::default(),
            copy_acts: Acts::default(),
        }
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// `appearance="large"`: 32 points tall.
    pub fn large(mut self) -> Self {
        self.large = true;
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
    rich_forwarders!();

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let red = self.invalid && !disabled && (k.invalid_text || self.plain);
        let ink = if red { c.danger.fg } else { c.text_base };
        let hint = if red { c.danger.fg } else { c.text_faint };
        let style = self.style.unwrap_or(k.ty.ui_compact);
        let base: f32 = if self.large { 32.0 } else { 28.0 };
        let height = self.height.map_or_else(|| base.max(style.line_px() + 12.0), |h| h.max(style.line_px()));
        let field = text_input(&self.edit, style)
            .single_line()
            .key("input")
            .grow(1.0)
            .shrink()
            .color(ink)
            .rich_style(kit_rich_style(k))
            .placeholder(self.placeholder.clone(), hint)
            .caret(ink, k.selection());
        let field = self.edits.apply(field, disabled);
        let mut value = row().key("value").grow(1.0).shrink().gap(6.0).center_y();
        if let Some(i) = self.leading {
            value = value.child(el().key("leading").pl(8.0).center().child(icon(i, self.leading_size).color(c.icon_muted)));
        } else {
            value = value.child(el().w(2.0));
        }
        value = value.child(field);
        let mut e = row()
            .role("textbox")
            .label(self.placeholder.clone())
            .h(height)
            .pr(8.0)
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
                .child(icon(glyph, 16.0).color(c.icon_muted));
            if self.clear {
                b = b.mr(-8.0);
            }
            if !disabled {
                b = self.copy_acts.apply(b.hit().hover(c.hover).press(crate::press_over(c.hover, c.pressed)), true);
            }
            e = e.child(b);
        }
        if self.plain {
            return if disabled { e.dim(0.5) } else { e };
        }
        shell(k, e, [0.06, 0.02], self.state, self.invalid && !disabled, self.edit.focused && !disabled)
    }
}

/// A multi-line field (`Textarea`): 80 points tall at least, 13/1.35 type, 8 points of padding.
pub struct TextArea {
    edit: TextEdit,
    placeholder: String,
    width: Option<f32>,
    rows: usize,
    grow: Option<(usize, usize)>,
    max_height: Option<f32>,
    invalid: bool,
    state: State,
    edits: EditActs,
}

impl TextArea {
    /// Protect the model like a password field. Hidden non-empty values use a fixed one-line
    /// mask. Clicks and unshifted navigation move to the end; shifted navigation selects all.
    /// Typing/paste append, or replace the whole value if selected; Backspace deletes at the
    /// end. Reveal to edit an interior position. Enter inserts a newline; Cmd/Ctrl+Enter submits.
    pub fn secure(mut self) -> Self {
        self.edit.set_secure(true);
        self
    }
    /// Show the text without enabling undo, copy, cut, IME or accessibility text.
    /// Toggling preserves the model selection until a hidden edit or navigation occurs.
    pub fn revealed(mut self, revealed: bool) -> Self {
        self.edit.set_revealed(revealed);
        self.edits.visual(move |e| e.revealed(revealed));
        self
    }
    pub fn new(edit: &TextEdit) -> TextArea {
        TextArea {
            edit: edit.clone(),
            placeholder: String::new(),
            width: Some(280.0),
            rows: 3,
            grow: None,
            max_height: None,
            invalid: false,
            state: State::Rest,
            edits: EditActs::default(),
        }
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    pub fn width(mut self, w: Option<f32>) -> Self {
        self.width = w;
        self
    }
    pub fn rows(mut self, r: usize) -> Self {
        self.rows = r.max(1);
        self
    }
    /// Fit the text: at least `min_rows` lines tall, growing with each wrapped line up to
    /// `max_rows`, then scrolling inside with the caret kept in view. The height springs on
    /// the kit's control motion.
    pub fn auto_grow(mut self, min_rows: usize, max_rows: usize) -> Self {
        let min = min_rows.max(1);
        self.grow = Some((min, max_rows.max(min)));
        self
    }
    /// Cap the field's height and scroll overflowing text. With [`TextArea::auto_grow`],
    /// also stop growing at this many points of text.
    pub fn max_height(mut self, h: f32) -> Self {
        self.max_height = Some(h);
        self
    }
    pub fn animate_edit_layout(mut self) -> Self {
        self.edits.handlers.push(Box::new(El::animate_edit_layout));
        self
    }
    /// See [`El::wrap_anywhere`](rune::El::wrap_anywhere).
    pub fn wrap_anywhere(mut self) -> Self {
        self.edits.visual(El::wrap_anywhere);
        self
    }
    pub fn invalid(mut self, i: bool) -> Self {
        self.invalid = i;
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
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, &rune::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.edits.key(f);
        self
    }
    pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.edits.change(f);
        self
    }
    rich_forwarders!();

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let style = TextStyle { line_height: (13.0f32 * 1.35 * 2.0).round() / 2.0, ..k.ty.ui };
        let red = self.invalid && !disabled && k.invalid_text;
        let ink = if red { c.danger.fg } else { c.text_base };
        let hint = if red { c.danger.fg } else { c.text_faint };
        let field = text_input(&self.edit, style)
            .key("input")
            .w_full()
            .color(ink)
            .rich_style(kit_rich_style(k))
            .placeholder(self.placeholder.clone(), hint)
            .caret(ink, k.selection());
        let field = self.edits.apply(field, disabled);
        let line = style.line_px();
        let mut e = col().role("textbox").label(self.placeholder.clone()).padding(8.0);
        e = if let Some((min_rows, max_rows)) = self.grow {
            let viewport = rune::auto_grow(field, min_rows, max_rows);
            e.child(match self.max_height {
                Some(h) => viewport.max_h(h.min(max_rows as f32 * line).max(line)),
                None => viewport,
            })
        } else if let Some(h) = self.max_height {
            let max_inner = (h - 16.0).max(line);
            let min_inner = (self.rows as f32 * line).max(80.0 - 16.0).min(max_inner);
            let viewport = col()
                .key("scroll")
                .w_full()
                .shrink()
                .min_h(min_inner)
                .max_h(max_inner)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(field.animate_edit_layout());
            e.min_h(min_inner + 16.0).child(viewport)
        } else {
            // A <textarea rows=3>: rows of its line height plus padding, at least 80.
            e.min_h((self.rows as f32 * line + 16.0).max(80.0)).child(field)
        };
        e = match self.width {
            Some(w) => e.w(w),
            None => e.w_full(),
        };
        shell(k, e, [0.02, 0.0], self.state, self.invalid && !disabled, self.edit.focused && !disabled)
    }
}

/// The older labelled field (`components/text-field`): label over a bordered 32 point input.
pub struct TextField {
    label: String,
    edit: TextEdit,
    placeholder: String,
    hide_label: bool,
    ghost: bool,
    multiline: bool,
    copyable: bool,
    trailing: Option<El>,
    error: Option<String>,
    state: State,
    edits: EditActs,
    copy_acts: Acts,
}

impl TextField {
    pub fn new(label: impl Into<String>, edit: &TextEdit) -> TextField {
        TextField {
            label: label.into(),
            edit: edit.clone(),
            placeholder: String::new(),
            hide_label: false,
            ghost: false,
            multiline: false,
            copyable: false,
            trailing: None,
            error: None,
            state: State::Rest,
            edits: EditActs::default(),
            copy_acts: Acts::default(),
        }
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    pub fn hide_label(mut self) -> Self {
        self.hide_label = true;
        self
    }
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
        self
    }
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
    pub fn copyable(mut self) -> Self {
        self.copyable = true;
        self
    }
    /// Trailing accessory element inside the field's box.
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing = Some(e);
        self
    }
    /// The trailing copy button's press.
    pub fn on_action<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.copy_acts.press_tx(f);
        self
    }
    pub fn error(mut self, e: impl Into<String>) -> Self {
        self.error = Some(e.into());
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
    rich_forwarders!();

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let body = TextStyle::new(14.0, 400.0).lh(21.0);
        let field = text_input(&self.edit, body)
            .key("input")
            .grow(1.0)
            .shrink()
            .color(c.text_strong)
            .rich_style(kit_rich_style(k))
            .placeholder(self.placeholder.clone(), c.text_weak)
            .caret(c.text_strong, k.selection());
        let field = if self.multiline { field } else { field.single_line() };
        let field = self.edits.apply(field, self.state == State::Disabled);
        let mut wrapper = row()
            .key("wrapper")
            .w_full()
            .min_h(32.0)
            .px(12.0)
            .py(if self.multiline { 6.0 } else { 0.0 })
            .center_y()
            .child(field);
        if !self.ghost {
            let (fill, border) =
                if self.error.is_some() { (c.danger.bg, c.danger.border) } else { (Color::TRANSPARENT, c.border_weak) };
            wrapper = wrapper.radius(k.radius.md).fill(fill).border(1.0, border);
        }
        if let Some(t) = self.trailing {
            wrapper = wrapper.child(row().key("trailing").self_center().center_y().child(t));
        }
        if self.copyable {
            let mut b = row()
                .key("copy")
                .role("button")
                .label("Copy")
                .size(24.0, 24.0)
                .center()
                .radius(k.radius.md)
                .hover(c.hover)
                .child(icon(k.glyph.copy, 16.0).color(c.std_icon));
            if self.state != State::Disabled {
                b = self.copy_acts.apply(b.hit().hover(c.hover).press(crate::press_over(c.hover, c.pressed)), true);
            }
            wrapper = wrapper.pr(4.0).child(b);
        }
        let mut root = col().role("textbox").label(self.label.clone()).gap(8.0).w_full();
        if !self.hide_label {
            root = root.child(text(self.label.clone()).key("label").font(k.ty.text_xsmall_medium).color(c.text_weak));
        }
        root = root.child(wrapper);
        if let Some(e) = &self.error {
            root = root.child(text(e.clone()).key("error").font(k.ty.text_xsmall_medium).color(c.danger.fg));
        }
        if self.state == State::Disabled { root.dim(0.5) } else { root }
    }
}
