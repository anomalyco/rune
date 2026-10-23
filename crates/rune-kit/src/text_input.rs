//! TextInput and Textarea (`@opencode/ui` forms/text-input, forms/textarea), and the older
//! TextField (`components/text-field`).

use rune::{Color, El, Icon, TextEdit, TextStyle, Tx, col, el, icon, row, text, text_input};

use crate::{Acts, Kit, State, over, shadows};

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


/// Edit bindings collected for the inner text field.
#[derive(Default)]
struct EditActs(Vec<Box<dyn FnOnce(El) -> El>>);

impl EditActs {
    fn edit<S: 'static>(&mut self, f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) {
        self.0.push(Box::new(move |e: El| e.on_edit(f)));
    }
    fn submit<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.0.push(Box::new(move |e: El| e.on_submit(f)));
    }
    fn key<S: 'static>(&mut self, f: impl Fn(&mut S, &rune::KeyEvent, &mut Tx) -> bool + 'static) {
        self.0.push(Box::new(move |e: El| e.on_input_key(f)));
    }
    fn change<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.0.push(Box::new(move |e: El| e.on_input_change(f)));
    }
    fn apply(self, mut e: El) -> El {
        for f in self.0 {
            e = f(e);
        }
        e
    }
}

/// A single-line field: `TextInput::new(&s.name).placeholder("Name").on_edit(|s: &mut S| &mut s.name)`.
pub struct TextInput {
    edit: TextEdit,
    placeholder: String,
    large: bool,
    width: Option<f32>,
    leading: Option<&'static Icon>,
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
    pub fn animate_edit_layout(mut self) -> Self {
        self.edits.0.push(Box::new(El::animate_edit_layout));
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
            width: Some(280.0),
            leading: None,
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
    /// Width in points (default 280); `None` fills the parent.
    pub fn width(mut self, w: Option<f32>) -> Self {
        self.width = w;
        self
    }
    pub fn leading_icon(mut self, i: &'static Icon) -> Self {
        self.leading = Some(i);
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

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let red = self.invalid && !disabled && k.invalid_text;
        let ink = if red { c.danger.fg } else { c.text_base };
        let hint = if red { c.danger.fg } else { c.text_faint };
        let field = text_input(&self.edit, k.ty.ui_compact)
            .single_line()
            .key("input")
            .grow(1.0)
            .shrink()
            .color(ink)
            .placeholder(self.placeholder.clone(), hint)
            .caret(if self.invalid && k.invalid_text { c.danger.fg } else { c.text_base }, k.alpha(c.text_accent, 0.3));
        let field = if disabled { field } else { self.edits.apply(field) };
        let mut value = row().key("value").grow(1.0).shrink().gap(6.0).center_y();
        if let Some(i) = self.leading {
            value = value.child(el().key("leading").pl(8.0).center().child(icon(i, 16.0).color(c.icon_muted)));
        } else {
            value = value.child(el().w(2.0));
        }
        value = value.child(field);
        let mut e = row()
            .role("textbox")
            .label(self.placeholder.clone())
            .h(if self.large { 32.0 } else { 28.0 })
            .pr(8.0)
            .gap(8.0)
            .child(value);
        e = match self.width {
            Some(w) => e.w(w),
            None => e.w_full(),
        };
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
                b = self.copy_acts.apply(b.hover(c.hover).press(crate::press_over(c.hover, c.pressed)), true);
            }
            e = e.child(b);
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
    invalid: bool,
    state: State,
    edits: EditActs,
}

impl TextArea {
    pub fn new(edit: &TextEdit) -> TextArea {
        TextArea {
            edit: edit.clone(),
            placeholder: String::new(),
            width: Some(280.0),
            rows: 3,
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
            .placeholder(self.placeholder.clone(), hint)
            .caret(ink, k.alpha(c.text_accent, 0.3));
        let field = if disabled { field } else { self.edits.apply(field) };
        // A <textarea rows=3>: rows of its line height plus padding, at least 80.
        let min = (self.rows as f32 * style.line_px() + 16.0).max(80.0);
        let mut e = col().role("textbox").label(self.placeholder.clone()).min_h(min).padding(8.0).child(field);
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
    error: Option<String>,
    state: State,
    edits: EditActs,
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
            error: None,
            state: State::Rest,
            edits: EditActs::default(),
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

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let body = TextStyle::new(14.0, 400.0).lh(21.0);
        let field = text_input(&self.edit, body)
            .key("input")
            .grow(1.0)
            .shrink()
            .color(c.text_strong)
            .placeholder(self.placeholder.clone(), c.text_weak)
            .caret(c.text_strong, k.alpha(c.text_accent, 0.3));
        let field = if self.multiline { field } else { field.single_line() };
        let field = if self.state == State::Disabled { field } else { self.edits.apply(field) };
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
        if self.copyable {
            wrapper = wrapper.pr(4.0).child(
                row()
                    .key("copy")
                    .size(24.0, 24.0)
                    .center()
                    .radius(k.radius.md)
                    .hover(c.hover)
                    .child(icon(k.glyph.copy, 16.0).color(c.std_icon)),
            );
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
