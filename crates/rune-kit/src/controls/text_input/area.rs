use rune::{Color, El, Icon, RichStyle, Span, TextEdit, Tx, col, row, text_input};

use crate::{Kit, ScrollBarStyle, State, TextAreaMetrics};

use super::{EditActs, kit_rich_style, name_field, shell};

#[derive(Clone, Copy)]
enum AreaScrollBar {
    None,
    Kit,
    Styled(ScrollBarStyle),
}

/// A multi-line field (`Textarea`): 80 points tall at least, 13/1.35 type, 8 points of padding
/// (or the kit's own type and padding, `Kit::metrics.text_area`).
/// Wrap in [`crate::Field`] with `.width(None)` for a visible label and wrapped help text.
pub struct TextArea {
    edit: TextEdit,
    label: Option<String>,
    description: Option<String>,
    placeholder: String,
    width: Option<f32>,
    rows: usize,
    grow: Option<(usize, usize)>,
    max_height: Option<f32>,
    plain: bool,
    trailing: Option<El>,
    scroll_bar: AreaScrollBar,
    invalid: bool,
    read_only: bool,
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
    /// Mark misspelled words and offer their suggestions in the field's menu (`El::spellcheck`).
    pub fn spellcheck(mut self, on: bool) -> Self {
        self.edits.visual(move |e| e.spellcheck(on));
        self
    }
    /// Use visual line start/end for plain `Home` and `End` (and `Shift+Home` / `Shift+End`) on
    /// macOS instead of document start/end ([`El::line_home_end`](rune::El::line_home_end)),
    /// keeping `Cmd+Home`/`Cmd+End`, `Ctrl+Home`/`Ctrl+End`, and `Cmd+Up`/`Cmd+Down` on the
    /// document's ends. Off by default (`false`).
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextArea};
    /// # struct App { code: TextEdit }
    /// # let s = App { code: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextArea::new(&s.code).line_home_end(true).on_edit(|s: &mut App| &mut s.code).build(&k)
    /// # ;
    /// ```
    pub fn line_home_end(mut self, on: bool) -> Self {
        self.edit.set_line_home_end(on);
        self.edits.visual(move |e| e.line_home_end(on));
        self
    }
    /// Scroll enclosing scroll viewports (including this field's internal [`Self::auto_grow`] or
    /// [`Self::max_height`] viewport) to bring the caret into view when the field gains focus
    /// ([`El::reveal_caret_on_focus`](rune::El::reveal_caret_on_focus)). Off by default (`false`).
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextArea};
    /// # struct App { notes: TextEdit }
    /// # let s = App { notes: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextArea::new(&s.notes)
    ///     .max_height(120.0)
    ///     .reveal_caret_on_focus(true)
    ///     .on_edit(|s: &mut App| &mut s.notes)
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn reveal_caret_on_focus(mut self, on: bool) -> Self {
        self.edits.visual(move |e| e.reveal_caret_on_focus(on));
        self
    }
    pub fn new(edit: &TextEdit) -> TextArea {
        TextArea {
            edit: edit.clone(),
            label: None,
            description: None,
            placeholder: String::new(),
            width: Some(280.0),
            rows: 3,
            grow: None,
            max_height: None,
            plain: false,
            trailing: None,
            scroll_bar: AreaScrollBar::None,
            invalid: false,
            read_only: false,
            state: State::Rest,
            edits: EditActs::default(),
        }
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// The accessible name, as [`TextInput::label`](super::TextInput::label).
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// Help or a validation error for the inner editor, separate from its name, as
    /// [`TextInput::description`](super::TextInput::description). Accessibility-only (not drawn).
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextArea};
    /// # struct App { notes: TextEdit }
    /// # let s = App { notes: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextArea::new(&s.notes).label("Notes").description("Markdown supported").build(&k)
    /// # ;
    /// ```
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
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
    /// A borderless multi-line field for sitting inside a custom container, such as a chat
    /// composer: no fill, sheen, shadow, border, hover tint or focus outline, so the surrounding
    /// container draws the box. Focus shows as the caret and selection (in a forced-colors theme,
    /// the focus outline still draws), an invalid field shows as red text, and a disabled field
    /// dims. Inner 8-point padding, `width`, `rows`, `auto_grow`, `max_height`, `trailing`,
    /// `scroll_bar` and press-anywhere focus all keep working.
    ///
    /// ```
    /// # use rune::{TextEdit, col};
    /// # use rune_kit::{Kit, TextArea};
    /// # struct App { draft: TextEdit }
    /// # let s = App { draft: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// col().role("textbox").fill(k.c.layer_01).radius(k.radius.lg)
    ///     .child(TextArea::new(&s.draft).plain().auto_grow(1, 8).width(None).build(&k))
    /// # ;
    /// ```
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }
    /// Trailing accessory element inside the field's box, anchored at the bottom-end beside the
    /// editable viewport (such as a Send or Commit button). The editor reserves width clear of
    /// the accessory, wraps and scrolls inside `auto_grow` / `max_height`, and leaves the
    /// accessory independently focusable and clickable.
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Button, Kit, TextArea};
    /// # struct App { draft: TextEdit }
    /// # impl App { fn send(&mut self) {} }
    /// # let s = App { draft: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextArea::new(&s.draft)
    ///     .auto_grow(1, 4)
    ///     .width(None)
    ///     .trailing(Button::new("Send").on_press(|s: &mut App| s.send()).build(&k))
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing = Some(e);
        self
    }
    /// Draws a kit scroll bar on the field's internal scroll viewport using
    /// [`ScrollBarStyle::from_kit`] when `on` is true, following [`rune::system_scroll_bars`].
    /// Applies when [`Self::auto_grow`] or [`Self::max_height`] creates an internal scroll
    /// viewport; otherwise a no-op. Off by default.
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextArea};
    /// # struct App { draft: TextEdit }
    /// # let s = App { draft: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextArea::new(&s.draft).auto_grow(2, 6).scroll_bar(true).build(&k)
    /// # ;
    /// ```
    pub fn scroll_bar(mut self, on: bool) -> Self {
        self.scroll_bar = if on { AreaScrollBar::Kit } else { AreaScrollBar::None };
        self
    }
    /// Draws a scroll bar on the field's internal scroll viewport in `style`, following
    /// [`rune::system_scroll_bars`]. Applies when [`Self::auto_grow`] or [`Self::max_height`]
    /// creates an internal scroll viewport; otherwise a no-op.
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, ScrollBarStyle, TextArea};
    /// # struct App { draft: TextEdit }
    /// # let s = App { draft: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextArea::new(&s.draft)
    ///     .max_height(120.0)
    ///     .scroll_bar_styled(ScrollBarStyle::from_kit(&k).always_visible())
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn scroll_bar_styled(mut self, style: ScrollBarStyle) -> Self {
        self.scroll_bar = AreaScrollBar::Styled(style);
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
    /// Make the field read-only ([`El::read_only`](rune::El::read_only)): keeps focus, caret,
    /// keyboard and pointer navigation/selection, and Copy while refusing typing, Cut, Paste,
    /// Undo/Redo, IME, and spellcheck edits. Pair with [`Self::on_edit`] so the runtime can
    /// move the caret and selection in your [`TextEdit`].
    ///
    /// ```
    /// # use rune::TextEdit;
    /// # use rune_kit::{Kit, TextArea};
    /// # struct App { preview: TextEdit }
    /// # let s = App { preview: TextEdit::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// TextArea::new(&s.preview).read_only(true).on_edit(|s: &mut App| &mut s.preview).build(&k)
    /// # ;
    /// ```
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
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
    field_hook_forwarders!("TextArea::new(&s.edit)");
    rich_forwarders!();

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let TextAreaMetrics { style, padding } = k.text_area_metrics();
        let min_height = k.text_area_min_height();
        let red = self.invalid && !disabled && (k.invalid_text || self.plain);
        let ink = if red { c.danger.fg } else { c.text_base };
        let hint = if red { c.danger.fg } else { c.text_faint };
        let field = text_input(&self.edit, style)
            .key("input")
            .w_full()
            .color(ink)
            .rich_style(kit_rich_style(k))
            .placeholder(self.placeholder.clone(), hint)
            .caret(ink, k.selection());
        let field = name_field(field, self.label.as_deref(), &self.placeholder, self.description.as_deref(), disabled)
            .invalid(self.invalid)
            .read_only(self.read_only);
        let field = self.edits.apply(field, disabled);
        let line = style.line_px();
        let bar_style = match self.scroll_bar {
            AreaScrollBar::None => None,
            AreaScrollBar::Kit => Some(ScrollBarStyle::from_kit(k)),
            AreaScrollBar::Styled(s) => Some(s),
        };
        let attach_bar = |viewport: El| match bar_style {
            Some(s) => crate::ScrollBar::scroll_bar_styled(viewport, s),
            None => viewport,
        };
        let (content, min_h) = if let Some((min_rows, max_rows)) = self.grow {
            let viewport = rune::auto_grow(field, min_rows, max_rows);
            let viewport = match self.max_height {
                Some(h) => viewport.max_h(h.min(max_rows as f32 * line).max(line)),
                None => viewport,
            };
            (attach_bar(viewport), None)
        } else if let Some(h) = self.max_height {
            let max_inner = (h - 2.0 * padding).max(line);
            let min_inner = (self.rows as f32 * line).max(min_height - 2.0 * padding).min(max_inner);
            let viewport = col()
                .key("scroll")
                .w_full()
                .shrink()
                .min_h(min_inner)
                .max_h(max_inner)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(field.animate_edit_layout());
            (attach_bar(viewport), Some(min_inner + 2.0 * padding))
        } else {
            // A <textarea rows=3>: rows of its line height plus padding, at least min_height.
            (field, Some((self.rows as f32 * line + 2.0 * padding).max(min_height)))
        };
        let mut e = if let Some(t) = self.trailing {
            row()
                .role("textbox")
                .padding(padding)
                .gap(8.0)
                .child(content.grow(1.0).shrink().min_w(0.0))
                .child(row().key("trailing").self_end().child(t))
        } else {
            col().role("textbox").padding(padding).child(content)
        };
        if let Some(h) = min_h {
            e = e.min_h(h);
        }
        e = match self.width {
            Some(w) => e.w(w),
            None => e.w_full(),
        };
        if self.plain {
            let e = match k.forced {
                Some(highlight) if (self.edit.focused || self.state == State::Focus) && !disabled => {
                    e.outline(2.0, 2.0, highlight)
                }
                _ => e,
            };
            return k.disabled_dim(e, disabled);
        }
        shell(k, e, self.state, self.invalid && !disabled, self.edit.focused && !disabled)
    }
}
