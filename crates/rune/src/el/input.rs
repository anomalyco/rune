//! Text-field construction, security and editing callbacks.

use super::*;

/// A text field bound to a [`TextEdit`] in app state.
#[derive(Clone)]
pub struct InputSpec {
    /// Text edits are immediate, but dependent layout (e.g. filtered results) may animate.
    pub animate_layout: bool,
    pub edit: TextEdit,
    pub placeholder: String,
    pub placeholder_color: Color,
    pub caret: Color,
    pub selection: Color,
    /// One unwrapped line that scrolls sideways to keep the caret visible, like `<input>`.
    pub single_line: bool,
    /// `placeholder_color`, and `caret` with `selection`, came from [`El::placeholder`] and
    /// [`El::caret`]; the others follow the window's foreground ([`Cx::foreground`](crate::Cx::foreground)).
    pub(crate) placeholder_inked: bool,
    pub(crate) caret_inked: bool,
    pub(crate) revealed_override: Option<bool>,
    pub(crate) decorations: Vec<Span>,
    pub(crate) rich_style: crate::edit::RichStyle,
    pub(crate) pill_icon: Option<PillIconFn>,
    pub(crate) pill_style: Option<PillStyleFn>,
    pub(crate) access: Option<EditFn>,
    pub(crate) submit: Option<PressFn>,
    pub(crate) key: Option<InputKeyFn>,
    pub(crate) change: Option<PressFn>,
    pub(crate) paste: Option<InputPasteFn>,
    /// `El::spellcheck`: the platform's checker marks misspelled words.
    pub(crate) spellcheck: bool,
    /// [`El::selection_style`]: the selection's highlight and ink, by window focus, over `selection`.
    pub(crate) selection_style: Option<SelectionStyle>,
}

impl std::fmt::Debug for InputSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputSpec")
            .field("animate_layout", &self.animate_layout)
            .field("edit", &self.edit)
            .field("placeholder", &self.placeholder)
            .field("placeholder_color", &self.placeholder_color)
            .field("caret", &self.caret)
            .field("selection", &self.selection)
            .field("single_line", &self.single_line)
            .finish_non_exhaustive()
    }
}

/// A text field showing `edit`; route edits back with `.on_edit(|s| &mut s.field)`.
///
/// Multi-line by default (see [`El::single_line`]). Plain Return in a paragraph submits (calling
/// [`El::on_submit`], or pressing the [`El::default_button`] when none is set) while Shift+Return
/// and Option+Return insert a line break; a document editor that wants plain Return to break
/// paragraphs attaches `.on_submit(|s, _| s.field.insert("\n"))`.
pub fn text_input(edit: &TextEdit, style: TextStyle) -> El {
    // An app may have assigned `edit.text` directly: clamp stale marks, blocks and caret to it.
    let mut edit = edit.clone();
    edit.set_single_line(false);
    edit.sync_invariants();
    let (content, _) = edit.display();
    let mut e = text(content).font(style);
    e.role = RoleTag::TEXTBOX;
    e.cold_mut().a11y.secure = edit.is_secure();
    if let Some(t) = &mut e.text {
        t.wrap = true;
        t.wrap_anywhere = edit.is_secure();
        t.redacted = edit.is_secure();
    }
    e.cold_mut().input = Some(Box::new(InputSpec {
        animate_layout: false,
        edit,
        placeholder: String::new(),
        placeholder_color: Color::WHITE.alpha(0.4),
        caret: Color::WHITE,
        selection: DEFAULT_SELECTION,
        single_line: false,
        placeholder_inked: false,
        caret_inked: false,
        revealed_override: None,
        decorations: Vec::new(),
        rich_style: crate::edit::RichStyle::default(),
        pill_icon: None,
        pill_style: None,
        access: None,
        submit: None,
        key: None,
        change: None,
        paste: None,
        spellcheck: false,
        selection_style: None,
    }));
    e.sync_input_pills();
    e.sync_input_rich();
    e
}

/// Fits a multi-line [`text_input`] to its text: `min_lines` tall at least, growing with each
/// wrapped line up to `max_lines`, then scrolling inside with the caret kept in view. The height
/// springs on the element's motion. Returns `field` inside a scroll viewport keyed `"scroll"`, so
/// key the field and attach its handlers first. A smaller `.max_h(..)` caps it in points.
#[cfg_attr(debug_assertions, track_caller)]
pub fn auto_grow(field: El, min_lines: usize, max_lines: usize) -> El {
    let line = field.text.as_ref().map_or(0.0, |t| t.style.line_px());
    let (min, max) = (min_lines.max(1), max_lines.max(min_lines.max(1)));
    col().key("scroll").w_full().shrink().min_h(min as f32 * line).max_h(max as f32 * line)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(field.animate_edit_layout())
}

/// Pins a closure to the signature [`El::on_edit`] needs, so Rust ties the returned borrow to
/// the state: `.on_edit(edit_access(move |s: &mut App| &mut s.rows[i].name))`.
pub fn edit_access<S, F: for<'a> Fn(&'a mut S) -> &'a mut TextEdit>(f: F) -> F {
    f
}

impl El {
    /// On the root or around text fields: the app's spell checking setting, `on`, which the
    /// fields' menus show as a Spellcheck item with a check mark, as a browser's do. Picking it
    /// calls `f` with the setting flipped; the app keeps it and passes it to its fields'
    /// [`El::spellcheck`].
    pub fn on_spellcheck<S: 'static>(mut self, on: bool, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_spellcheck = Some((on, Rc::new(move |s, on, tx| f(downcast::<S>(s), on, tx))));
        self
    }
    /// Beside [`El::on_spellcheck`], where the platform has dictionaries to choose
    /// ([`crate::spell::available_languages`]): the fields' menus add a Spellcheck Languages
    /// submenu, as Chromium's do off macOS. Picking a dictionary checks with the new choice at
    /// once ([`crate::spell::set_languages`]) and calls `f` with it, for the app to keep and
    /// pass to `set_languages` at its next launch.
    pub fn on_spellcheck_languages<S: 'static>(mut self, f: impl Fn(&mut S, Vec<String>, &mut Tx) + 'static) -> Self {
        self.handlers.on_spellcheck_languages = Some(Rc::new(move |s, tags, tx| f(downcast::<S>(s), tags, tx)));
        self
    }
    /// Mask a text element or text input before layout; disable undo, copy, cut, selection and
    /// IME composition, omit accessibility text, redact [`Debug`], bypass layout/raster caches,
    /// and zeroize owned buffers on conceal and drop.
    ///
    /// Non-empty read-only [`text`] and [`rich`] labels show a fixed twelve-bullet mask on
    /// one line, hiding their length as well as their content. Explicit labels and help must
    /// describe the field, not contain its secret value. Links and rolling text are disabled.
    /// Pair with [`revealed`](Self::revealed) to toggle plaintext display.
    pub fn secure(mut self) -> Self {
        self.cold_mut().a11y.secure = true;
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.edit.set_secure(true);
            let single = input.single_line;
            if let Some(text) = &mut self.text {
                text.replace_content(input.edit.display().0);
                text.redacted = true;
                if !single {
                    text.wrap_anywhere = true;
                }
            }
            self.sync_input_pills();
            self.sync_input_rich();
        } else if let Some(text) = &mut self.text {
            text.redacted = true;
            if text.selectable {
                text.selectable = false;
                if self.handlers.cursor == Some(Cursor::Text) {
                    self.handlers.cursor = None;
                }
            }
        }
        self
    }

    /// Show the plain characters of a [`secure`](Self::secure) text element or text input while
    /// keeping all other secure protections (no undo, copy, cut, selection, IME or accessibility
    /// text, redacted [`Debug`], uncached shaping/raster, and zeroized buffers). Prefer
    /// [`TextEdit::revealed`] or [`TextEdit::set_revealed`] when an input model owns the state.
    pub fn revealed(mut self, revealed: bool) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.edit.set_revealed(revealed);
            input.revealed_override = Some(revealed);
            if let Some(text) = &mut self.text {
                text.replace_content(input.edit.display().0);
            }
            self.sync_input_pills();
            self.sync_input_rich();
        } else if let Some(text) = &mut self.text {
            text.cold.secure_revealed = revealed;
        }
        self
    }
    /// Called after a focused field's text changes, with the updated model available.
    pub fn on_input_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.change = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        }
        self
    }
    /// Handle a paste into this focused text field before the editor, with every form the
    /// clipboard held ([`ClipboardContent`](crate::ClipboardContent): plain text, and sanitised
    /// HTML and RTF when the source offered them). Return true when the app took it; false lets
    /// the editor paste the plain text as usual. Without it a field pastes plain text only.
    ///
    /// ```ignore
    /// text_input(&s.draft).on_input_paste(|s: &mut Chat, content, _| match content.html() {
    ///     Some(html) => { s.draft_rich.paste_html(html, content.text()); true }
    ///     None => false,
    /// })
    /// ```
    pub fn on_input_paste<S: 'static>(mut self, f: impl Fn(&mut S, &crate::ClipboardContent, &mut Tx) -> bool + 'static) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.paste = Some(Rc::new(move |s, content, tx| f(downcast::<S>(s), content, tx)));
        }
        self
    }
    /// Handle a focused text field's key before the editor. Return true to consume it.
    pub fn on_input_key<S: 'static>(mut self, f: impl Fn(&mut S, &crate::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.key = Some(Rc::new(move |s, key, tx| f(downcast::<S>(s), key, tx)));
        }
        self
    }
}
