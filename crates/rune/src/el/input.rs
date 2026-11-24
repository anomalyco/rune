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
    /// [`El::placeholder_font`]: the placeholder's own style, as CSS's `::placeholder`; else the field's.
    pub placeholder_style: Option<TextStyle>,
    pub caret: Color,
    pub selection: Color,
    /// One unwrapped line that scrolls sideways to keep the caret visible, like `<input>`.
    pub single_line: bool,
    /// `placeholder_color`, `caret` and `selection` came from [`El::placeholder`], [`El::caret`]
    /// and [`El::selection_color`]; otherwise they follow the field's text colour ([`El::color`],
    /// else [`Cx::foreground`](crate::Cx::foreground), else white).
    pub(crate) placeholder_inked: bool,
    pub(crate) caret_inked: bool,
    pub(crate) selection_inked: bool,
    pub(crate) revealed_override: Option<bool>,
    pub(crate) line_home_end_override: Option<bool>,
    pub(crate) decorations: Vec<Span>,
    /// [`El::strike`] ranges over `edit.text`, mapped onto the drawn text with the spans.
    pub(crate) decoration_strikes: Vec<(u32, u32, Option<Color>)>,
    /// [`El::overline`] ranges over `edit.text`, mapped onto the drawn text with the spans.
    pub(crate) decoration_overlines: Vec<(u32, u32, Option<Color>)>,
    /// [`El::line_fills`] ranges over `edit.text`, mapped onto the drawn text with the spans.
    pub(crate) line_fills: Vec<LineFill>,
    pub(crate) rich_style: crate::edit::RichStyle,
    pub(crate) pill_icon: Option<PillIconFn>,
    pub(crate) pill_image: Option<PillImageFn>,
    pub(crate) pill_style: Option<PillStyleFn>,
    pub(crate) pill_copy_text: Option<PillCopyTextFn>,
    pub(crate) on_pill_press: Option<PillPressFn>,
    pub(crate) on_pill_secondary: Option<PillSecondaryFn>,
    pub(crate) access: Option<EditFn>,
    pub(crate) submit: Option<PressFn>,
    pub(crate) key: Option<InputKeyFn>,
    pub(crate) change: Option<PressFn>,
    pub(crate) selection_change: Option<PressFn>,
    pub(crate) paste: Option<InputPasteFn>,
    pub(crate) copy: Option<InputCopyFn>,
    /// `El::spellcheck`: the platform's checker marks misspelled words.
    pub(crate) spellcheck: bool,
    /// [`El::reveal_caret_on_focus`]: scroll enclosing viewports to the caret when this field gains focus.
    pub(crate) reveal_caret_on_focus: bool,
    /// `El::read_only`: mirrors read-only state onto offscreen virtual inputs.
    pub(crate) read_only: bool,
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
        placeholder_style: None,
        caret: Color::WHITE,
        selection: DEFAULT_SELECTION,
        single_line: false,
        placeholder_inked: false,
        caret_inked: false,
        selection_inked: false,
        revealed_override: None,
        line_home_end_override: None,
        decorations: Vec::new(),
        decoration_strikes: Vec::new(),
        decoration_overlines: Vec::new(),
        line_fills: Vec::new(),
        rich_style: crate::edit::RichStyle::default(),
        pill_icon: None,
        pill_image: None,
        pill_style: None,
        pill_copy_text: None,
        on_pill_press: None,
        on_pill_secondary: None,
        access: None,
        submit: None,
        key: None,
        change: None,
        selection_change: None,
        paste: None,
        copy: None,
        spellcheck: false,
        reveal_caret_on_focus: false,
        read_only: false,
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
    /// Use visual line start/end for plain `Home` and `End` (and `Shift+Home` / `Shift+End`) on
    /// macOS instead of document start/end, keeping `Cmd+Home`/`Cmd+End`, `Ctrl+Home`/`Ctrl+End`,
    /// and `Cmd+Up`/`Cmd+Down` on the document's ends. Off by default (`false`) so native macOS
    /// text navigation is unchanged; on Windows and Linux plain `Home` and `End` already move to
    /// the current visual line's edges.
    ///
    /// ```no_run
    /// # use rune::{TextEdit, TextStyle, text_input};
    /// let code = TextEdit::new("first\nsecond");
    /// let editor = text_input(&code, TextStyle::new(13.0, 400.0).mono()).line_home_end(true);
    /// ```
    pub fn line_home_end(mut self, on: bool) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.edit.set_line_home_end(on);
            input.line_home_end_override = Some(on);
        }
        self
    }
    /// Scroll enclosing scroll viewports to bring this text field's caret into view when it gains
    /// focus (via Tab, F6, [`Tx::focus`](crate::Tx::focus), `TextEdit::focused`, modal focus, a
    /// click on a clipped line, or assistive technology), matching Chrome's focus scroll for
    /// `<textarea>` and `<input>`.
    ///
    /// Off by default (`false`). A caret already inside the viewport moves nothing, and while the
    /// field keeps focus, wheel scrolling away from the caret or empty IME events leave the scroll
    /// position untouched. Respects [`El::scroll_margin`].
    ///
    /// ```no_run
    /// # use rune::{ScrollSpec, TextEdit, TextStyle, col, text_input};
    /// let edit = TextEdit::new("line 1\nline 2\nline 3");
    /// let view = col()
    ///     .h(80.0)
    ///     .scroll(ScrollSpec::default())
    ///     .child(text_input(&edit, TextStyle::new(13.0, 400.0)).reveal_caret_on_focus(true));
    /// ```
    pub fn reveal_caret_on_focus(mut self, on: bool) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.reveal_caret_on_focus = on;
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
    /// Called after a focused text field's caret or selection (`anchor`, `focus`) changes from
    /// runtime input (arrow keys, `Home`/`End`, `Shift` navigation, `Cmd/Ctrl+A`, clicks,
    /// double/triple clicks, pointer drags, edge autoscroll, field context-menu caret moves,
    /// assistive-technology selection actions, or text edits), in the same turn before the view
    /// rebuilds.
    ///
    /// When an edit changes both the text and the caret/selection, [`El::on_input_change`] runs
    /// first and `on_input_selection` runs immediately after it. No-op moves (such as `Left` at
    /// offset 0) do not fire it, and direct app-side mutations to `TextEdit` outside runtime
    /// input do not re-trigger it.
    ///
    /// ```no_run
    /// # use rune::{TextEdit, TextStyle, text_input};
    /// # struct App { edit: TextEdit, sel: (usize, usize) }
    /// # let s = App { edit: TextEdit::new("hello"), sel: (0, 0) };
    /// let _ = text_input(&s.edit, TextStyle::new(14.0, 400.0))
    ///     .on_edit(|s: &mut App| &mut s.edit)
    ///     .on_input_selection(|s: &mut App, _tx| {
    ///         s.sel = (s.edit.anchor, s.edit.focus);
    ///     });
    /// ```
    pub fn on_input_selection<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.selection_change = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
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
    /// Intercept Copy and Cut on this focused text field across both keyboard shortcuts
    /// (`Cmd/Ctrl+C`, `Cmd/Ctrl+X`) and the field's context menu. The `bool` argument is `true`
    /// for Cut and `false` for Copy.
    ///
    /// Return `Some(content)` to write that [`ClipboardContent`](crate::ClipboardContent) via
    /// [`Tx::copy_rich`] (on Cut, the editor still deletes the selection and records undo after
    /// the hook returns); return `None` to fall back to the default plain/rich copy. Never
    /// invoked for secure fields or empty selections.
    ///
    /// ```no_run
    /// # use rune::{ClipboardContent, TextEdit, TextStyle, text_input};
    /// # struct App { draft: TextEdit }
    /// # let s = App { draft: TextEdit::new("hello").rich() };
    /// let _ = text_input(&s.draft, TextStyle::new(14.0, 400.0))
    ///     .on_edit(|s: &mut App| &mut s.draft)
    ///     .on_input_copy(|s: &mut App, _cut, _| {
    ///         let plain = s.draft.selected().to_uppercase();
    ///         Some(ClipboardContent::new(plain).with_html(s.draft.selected_html()))
    ///     });
    /// ```
    pub fn on_input_copy<S: 'static>(
        mut self,
        f: impl Fn(&mut S, bool, &mut Tx) -> Option<crate::ClipboardContent> + 'static,
    ) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.copy = Some(Rc::new(move |s, cut, tx| f(downcast::<S>(s), cut, tx)));
        }
        self
    }
    /// Plain-text copy/cut payload for each [`Mark::Pill`](crate::Mark::Pill) in this
    /// [`text_input`], by the pill's id (`None` falls back to the pill's display label).
    ///
    /// During Copy and Cut (both keyboard and context menu), wholly selected pills are expanded
    /// with [`TextEdit::selected_with_pills`](crate::TextEdit::selected_with_pills) for the plain
    /// clipboard representation while rich fields keep `<span data-pill="id">label</span>` in
    /// HTML so pasting back into Rune restores the pill. Defaults to `None` (copies the display
    /// label).
    ///
    /// ```no_run
    /// # use rune::{TextEdit, TextStyle, text_input};
    /// # let draft = TextEdit::new("").rich();
    /// let _ = text_input(&draft, TextStyle::new(14.0, 400.0))
    ///     .pill_copy_text(|id| id.strip_prefix("url:").map(str::to_owned));
    /// ```
    pub fn pill_copy_text(mut self, f: impl Fn(&str) -> Option<String> + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.pill_copy_text = Some(Rc::new(f));
        }
        self
    }
    /// Primary press (left-click or tap) on a [`Mark::Pill`](crate::Mark::Pill) chip inside
    /// this [`text_input`], invoked before caret placement. Called with the
    /// pill's id and the press point in window points; return `true` to consume the press
    /// without moving the caret or starting a text-selection drag, or `false` to fall through to
    /// normal caret placement. Skipped while `Shift` is held so `Shift+click` continues to
    /// extend the selection. While hovering a pill in a field with this handler, the cursor
    /// shows [`Cursor::Pointer`].
    ///
    /// ```no_run
    /// # use rune::{TextEdit, TextStyle, text_input};
    /// # struct App { edit: TextEdit, picked: Option<String> }
    /// # let s = App { edit: TextEdit::new("").rich(), picked: None };
    /// let _ = text_input(&s.edit, TextStyle::new(14.0, 400.0))
    ///     .on_edit(|s: &mut App| &mut s.edit)
    ///     .on_pill_press(|s: &mut App, id, _press, _tx| {
    ///         s.picked = Some(id.to_owned());
    ///         true
    ///     });
    /// ```
    pub fn on_pill_press<S: 'static>(
        mut self,
        f: impl Fn(&mut S, &str, PointerPress, &mut Tx) -> bool + 'static,
    ) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.on_pill_press = Some(Rc::new(move |s, id, press, tx| f(downcast::<S>(s), id, press, tx)));
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
