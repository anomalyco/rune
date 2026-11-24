use rune::{Color, El, Icon, RichStyle, Span, TextEdit, TextStyle, Tx, col, icon, row, text, text_input};

use crate::{Acts, Kit, State};

use super::{EditActs, kit_rich_style, name_field};

/// The older labelled field (`components/text-field`): label over a bordered 32 point input.
///
/// Draws a visible label and a wrapped, assertive [`Self::error`] line, but has no `secure`,
/// `on_change` or visible help text. Prefer [`crate::Field`] with [`crate::TextInput`] or
/// [`crate::TextArea`] for new forms.
pub struct TextField {
    label: String,
    description: Option<String>,
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
            description: None,
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
    /// Help text for the inner editor, separate from its label and [`Self::error`].
    /// Accessibility-only (not drawn); when [`Self::error`] is also set, both describe the
    /// editor, with the description before the error.
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextField};
    /// # struct App { name: TextEdit }
    /// # let s = App { name: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextField::new("Name", &s.name).description("Letters and digits only").build(&k)
    /// # ;
    /// ```
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
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
    /// Called when the user submits the focused field with Return (`El::on_submit`). Skipped when disabled.
    /// ```
    /// # use rune::{TextEdit, Tx};
    /// # use rune_kit::{Kit, TextField};
    /// # struct App { name: TextEdit }
    /// # impl App { fn save(&mut self, _: &mut Tx) {} }
    /// # let s = App { name: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextField::new("Name", &s.name).on_edit(|s: &mut App| &mut s.name).on_submit(|s: &mut App, tx: &mut Tx| s.save(tx)).build(&k)
    /// # ;
    /// ```
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.edits.submit(f);
        self
    }
    field_hook_forwarders!("TextField::new(\"Name\", &s.edit)");
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
        let desc = match (
            self.description.as_deref().filter(|s| !s.is_empty()),
            self.error.as_deref().filter(|s| !s.is_empty()),
        ) {
            (Some(d), Some(e)) if d.ends_with(['.', '!', '?']) => Some(format!("{d} {e}")),
            (Some(d), Some(e)) => Some(format!("{d}. {e}")),
            (Some(d), None) => Some(d.to_owned()),
            (None, Some(e)) => Some(e.to_owned()),
            (None, None) => None,
        };
        // Description and error describe the editor; the label above names it.
        let field = name_field(field, Some(self.label.as_str()), &self.placeholder, desc.as_deref(), self.state == State::Disabled);
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
                .child(icon(k.glyph.copy, 16.0).color(c.std_icon))
                .disabled(self.state == State::Disabled);
            if self.state != State::Disabled {
                b = self.copy_acts.apply(k.interactive(b.hit()), true);
            }
            wrapper = wrapper.pr(4.0).child(b);
        }
        let mut root = col().role("textbox").gap(8.0).w_full();
        if !self.hide_label {
            root = root.child(text(self.label.clone()).key("label").font(k.ty.text_xsmall_medium).color(c.text_weak).a11y_hidden());
        }
        root = root.child(wrapper);
        if let Some(e) = &self.error {
            // Announced as it appears, and read again as the field's description.
            root = root.child(text(e.clone()).key("error").font(k.ty.text_xsmall_medium).color(c.danger.fg)
                .wrap().w_full().live_region(rune::LiveRegion::Assertive));
        }
        k.disabled_dim(root, self.state == State::Disabled)
    }
}
