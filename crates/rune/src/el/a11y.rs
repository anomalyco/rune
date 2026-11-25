//! What an element tells assistive technology beyond its role and label: description, value,
//! state, live updates and its place in a set. Screen readers, Voice Control and automation
//! tools read it through the OS accessibility tree (`crate::a11y`).

use std::borrow::Cow;
use crate::El;

/// How assistive technology announces changes to an element's text ([`El::live_region`]), as ARIA's
/// `aria-live`. Role `"status"` is polite and `"alert"` assertive unless set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LiveRegion {
    /// Changes aren't announced.
    #[default]
    Off,
    /// Announced when the user is idle: a status line, a saved notice.
    Polite,
    /// Announced at once, interrupting: an error.
    Assertive,
}

/// The kind of interactive popup an element opens ([`El::has_popup`]), as ARIA's `aria-haspopup`.
///
/// ```
/// # use rune::{HasPopup, el, text};
/// let trigger = el()
///     .key("fruit-btn")
///     .role("button")
///     .has_popup(HasPopup::Listbox)
///     .child(text("Fruit"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HasPopup {
    /// Opens a menu (`aria-haspopup="menu"`).
    Menu,
    /// Opens a listbox (`aria-haspopup="listbox"`).
    Listbox,
    /// Opens a tree (`aria-haspopup="tree"`).
    Tree,
    /// Opens a grid (`aria-haspopup="grid"`).
    Grid,
    /// Opens a dialog (`aria-haspopup="dialog"`).
    Dialog,
}

impl HasPopup {
    pub(crate) fn as_aria(self) -> &'static str {
        match self {
            Self::Menu => "menu",
            Self::Listbox => "listbox",
            Self::Tree => "tree",
            Self::Grid => "grid",
            Self::Dialog => "dialog",
        }
    }
}

/// The layout axis of a composite widget, toolbar, scrollbar, or separator ([`El::orientation`]),
/// as ARIA's `aria-orientation`.
///
/// ```
/// # use rune::{Orientation, row};
/// let tabs = row().key("tabs").role("tablist").orientation(Orientation::Horizontal);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// Items run horizontally (`aria-orientation="horizontal"`).
    Horizontal,
    /// Items run vertically (`aria-orientation="vertical"`).
    Vertical,
}

impl Orientation {
    pub(crate) fn as_aria(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

/// How typing in a text field or editable combobox suggests or completes values
/// ([`El::auto_complete`]), as ARIA's `aria-autocomplete`.
///
/// Leave unset for ARIA's implicit `none` default (no completion).
///
/// ```
/// # use rune::{AutoComplete, TextEdit, TextStyle, text_input};
/// # let edit = TextEdit::default();
/// let field = text_input(&edit, TextStyle::new(14.0, 400.0))
///     .single_line()
///     .role("combobox")
///     .label("Fruit")
///     .auto_complete(AutoComplete::List);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutoComplete {
    /// Inserts an inline suggestion after the caret (`aria-autocomplete="inline"`).
    Inline,
    /// Filters or presents a popup list of suggestions (`aria-autocomplete="list"`).
    List,
    /// Completes inline and presents a popup list of suggestions (`aria-autocomplete="both"`).
    Both,
}

impl AutoComplete {
    pub(crate) fn as_aria(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::List => "list",
            Self::Both => "both",
        }
    }
}

/// A checkbox's, switch's or toggle button's state for accessibility ([`El::checked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Checked {
    False,
    True,
    Mixed,
}

/// A range control's value ([`El::range_value`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Range {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
}

impl Range {
    /// The bounds `(min, max)` when finite and ordered (`min <= max`).
    pub(crate) fn usable_bounds(self) -> Option<(f64, f64)> {
        (self.min.is_finite() && self.max.is_finite() && self.min <= self.max)
            .then_some((self.min, self.max))
    }

    /// The step when the range is finite, ordered (`min <= max`) and has a finite positive step.
    pub(crate) fn usable_step(self) -> Option<f64> {
        if !self.value.is_finite() || self.usable_bounds().is_none() {
            return None;
        }
        self.step.filter(|s| s.is_finite() && *s > 0.0)
    }
}

/// The rarer accessibility properties, boxed so an element without them stays small.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct A11yMore {
    pub description: Option<Cow<'static, str>>,
    pub value: Option<String>,
    pub range: Option<Range>,
    /// 1-based position and the set's size.
    pub set: Option<(usize, usize)>,
    pub shortcut: Option<String>,
    /// `El::active_descendant`'s key.
    pub active: Option<String>,
    pub invalid: bool,
    pub read_only: bool,
    /// `El::sorted`: ascending or descending.
    pub sorted: Option<bool>,
    /// `El::href`: the URL this element links to.
    pub href: Option<String>,
    /// `El::labelled_by`'s key.
    pub labelled_by: Option<Cow<'static, str>>,
    /// `El::described_by`'s key.
    pub described_by: Option<Cow<'static, str>>,
    /// `El::busy`.
    pub busy: bool,
    /// Total rows in a `table`, `grid` or `treegrid` (`El::row_count`).
    pub row_count: Option<usize>,
    /// Total columns in a `table`, `grid` or `treegrid` (`El::column_count`).
    pub column_count: Option<usize>,
    /// 1-based row index of a `row`, `cell`, `gridcell`, `columnheader` or `rowheader` (`El::row_index`).
    pub row_index: Option<usize>,
    /// 1-based column index of a `row`, `cell`, `gridcell`, `columnheader` or `rowheader` (`El::column_index`).
    pub column_index: Option<usize>,
    /// Whether a `listbox`, `tree`, `grid` or `tablist` allows multiple selected items (`El::multiselectable`).
    pub multiselectable: bool,
    /// Whether user input or selection is required (`El::required`).
    pub required: bool,
    /// Kind of interactive popup triggered by this element (`El::has_popup`).
    pub has_popup: Option<HasPopup>,
    /// Layout axis of a composite widget, toolbar, scrollbar or separator (`El::orientation`).
    pub orientation: Option<Orientation>,
    /// How typing suggests or completes values (`El::auto_complete`).
    pub auto_complete: Option<AutoComplete>,
    /// `El::controls`'s key (ARIA `aria-controls`).
    pub controls: Option<String>,
    /// `El::owns`'s key (ARIA `aria-owns`).
    pub owns: Option<String>,
    /// Starting ordinal of an ordered `"list"` element (HTML `<ol start="…">`).
    pub list_start: Option<u64>,
}

/// Accessibility state beyond role and label (`El::level`, `El::expanded`, `El::secure`, and
/// this file's builders).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct A11y {
    pub level: Option<u32>,
    pub expanded: Option<bool>,
    pub secure: bool,
    pub checked: Option<Checked>,
    pub disabled: bool,
    pub hidden: bool,
    /// `El::unselectable`: a web page's mirror leaves this subtree's text out of the selection.
    pub unselectable: bool,
    pub live: Option<LiveRegion>,
    pub more: Option<Box<A11yMore>>,
}

impl A11y {
    pub(crate) const EMPTY: A11y = A11y {
        level: None, expanded: None, secure: false, checked: None, disabled: false, hidden: false, unselectable: false,
        live: None, more: None,
    };

    fn more(&mut self) -> &mut A11yMore {
        self.more.get_or_insert_with(Default::default)
    }
}

impl El {
    /// Make this element a link to `url`, as an HTML `<a href>` is: its role is `"link"`, a
    /// press opens `url` with [`Tx::open_url`](crate::Tx::open_url), and on web the semantic
    /// mirror puts a real `<a href>` over it, so middle-click, ⌘-click, "Copy Link", the
    /// keyboard and crawlers treat it as one. A static page export (`--html`) writes it as one
    /// too. `url` may be relative to the page (`/docs`) or a fragment (`#install`); natively
    /// those don't open. Its label is its text unless [`El::label`] sets one.
    pub fn href(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.cold_mut().a11y.more().href = Some(url.clone());
        self.set_role_name("link");
        self.handlers.on_press = Some(std::rc::Rc::new(move |_, tx: &mut crate::Tx| {
            if url.contains(':') { tx.open_url(&url).ok(); }
        }));
        self
    }

    /// The URL [`El::href`] set.
    pub fn link_url(&self) -> Option<&str> {
        self.cold.a11y.more.as_ref()?.href.as_deref()
    }

    /// A longer explanation than the label, read after it (ARIA `aria-description`): what a
    /// setting does, or the shortcut a button has. The label stays short. Without one, an
    /// [`El::help`] tooltip describes the element; `description("")` turns that off.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().description = Some(Cow::Owned(text.into()));
        self
    }
    /// [`El::description`] from a string that lives for the program (a translated message, a
    /// literal): kept as it is, with nothing copied, so a view that runs every frame pays
    /// nothing for it.
    pub fn description_static(mut self, text: &'static str) -> Self {
        self.cold_mut().a11y.more().description = Some(Cow::Borrowed(text));
        self
    }
    /// The element whose content is active while this one keeps keyboard focus (ARIA
    /// `aria-activedescendant`): the highlighted row of a tree, table or suggestion list whose
    /// container or search field holds focus. `key` names it as `Cx::layout` does; an empty
    /// one names none, and costs nothing.
    pub fn active_descendant(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        if key.is_empty() {
            return self;
        }
        self.cold_mut().a11y.more().active = Some(key);
        self
    }
    /// The element whose text names this one (ARIA `aria-labelledby`): a setting's title for
    /// its switch, a dialog's heading for the dialog. `key` names it as `Cx::layout` does, and the
    /// nearest element with that key counts: the first inside this one's parent, else inside its
    /// grandparent, and so on, so a component's controls can all name their own `"title"`. A label
    /// set with [`El::label`] wins.
    pub fn labelled_by(mut self, key: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().labelled_by = Some(Cow::Owned(key.into()));
        self
    }
    /// The element whose text describes this one (ARIA `aria-describedby`): a setting's
    /// explanation for its switch, a dialog's message for the dialog.
    pub fn described_by(mut self, key: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().described_by = Some(Cow::Owned(key.into()));
        self
    }
    /// The element is still changing and a reader should wait for it to settle (ARIA
    /// `aria-busy`): a turn while the reply streams, a tool call while it runs.
    pub fn busy(mut self, busy: bool) -> Self {
        if busy || self.cold.a11y.more.is_some() { self.cold_mut().a11y.more().busy = busy; }
        self
    }
    /// Whether the value entered here fails validation (ARIA `aria-invalid`).
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.cold_mut().a11y.more().invalid = invalid;
        self
    }
    /// A control whose value can be read but not changed now (ARIA `aria-readonly`): a
    /// checklist shown in a finished document, or a read-only [`text_input`](crate::text_input).
    ///
    /// On a bound [`text_input`](crate::text_input) (with [`El::on_edit`]), `read_only(true)`
    /// matches HTML's `<input readonly>` / `<textarea readonly>`: the field stays focusable with
    /// a visible caret, pointer and keyboard navigation, `Shift` selection, `Cmd/Ctrl+A`,
    /// `Cmd/Ctrl+C`, and assistive `Focus`/`Click`/`SetTextSelection`, while refusing typing,
    /// `Backspace`/`Delete`, `Enter`, `Cut`, `Paste`, `Undo`/`Redo`, rich formatting chords,
    /// IME composition, spellcheck suggestions, and assistive `SetValue`/`ReplaceSelectedText`.
    ///
    /// ```no_run
    /// # use rune::{TextEdit, TextStyle, text_input};
    /// # struct App { preview: TextEdit }
    /// # let s = App { preview: TextEdit::new("Read-only text") };
    /// let _ = text_input(&s.preview, TextStyle::new(14.0, 400.0))
    ///     .read_only(true)
    ///     .on_edit(|s: &mut App| &mut s.preview);
    /// ```
    pub fn read_only(mut self, read_only: bool) -> Self {
        if read_only || self.cold.a11y.more.is_some() {
            self.cold_mut().a11y.more().read_only = read_only;
        }
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.read_only = read_only;
        }
        self
    }
    /// How a column header sorts its table: `Some(true)` ascending, `Some(false)` descending
    /// (ARIA `aria-sort`).
    pub fn sorted(mut self, ascending: Option<bool>) -> Self {
        self.cold_mut().a11y.more().sorted = ascending;
        self
    }
    /// The value assistive technology reads for a control whose value isn't its text: a
    /// picker's chosen item, a colour well's colour, a step counter's "3 of 5". Text fields
    /// report their text and range controls their number without it.
    pub fn a11y_value(mut self, text: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().value = Some(text.into());
        self
    }
    /// A slider's or progress bar's number, between `min` and `max` (ARIA `aria-valuenow`,
    /// `aria-valuemin` and `aria-valuemax`). Assistive technology sets it through
    /// [`El::on_set_value`] and increments or decrements it through [`El::on_step`].
    pub fn range_value(mut self, value: f64, min: f64, max: f64) -> Self {
        let step = self.cold.a11y.more.as_ref().and_then(|m| m.range).and_then(|r| r.step);
        self.cold_mut().a11y.more().range = Some(Range { value, min, max, step });
        self
    }
    /// Bind a [`range_value`](Self::range_value)'s number to a live value, so a progress bar or
    /// meter tells assistive technology where it is without a view commit. The bounds stay those
    /// of `range_value` (0 to 1 without one). A smoothed binding reports the value it moves to,
    /// not each step of its spring; a [`Tween`](crate::Tween) reports each frame's sample.
    ///
    /// ```no_run
    /// # use rune::{Live, el};
    /// let done = Live::new(0.0_f32);
    /// let bar = el().role("progressbar").range_value(0.0, 0.0, 1.0).live_range_value(&done);
    /// done.set(0.4);
    /// ```
    pub fn live_range_value(mut self, value: impl Into<crate::live::Binding<f32>>) -> Self {
        let more = self.cold_mut().a11y.more();
        more.range.get_or_insert(Range { value: 0.0, min: 0.0, max: 1.0, step: None });
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::RangeValue));
        self
    }
    /// The amount one step of [`El::on_step`] moves a [`range_value`](Self::range_value).
    /// [`El::on_set_value`] takes absolute numeric values with or without a step.
    pub fn range_step(mut self, step: f64) -> Self {
        let more = self.cold_mut().a11y.more();
        let range = more.range.get_or_insert(Range { value: 0.0, min: 0.0, max: 0.0, step: None });
        range.step = Some(step);
        self
    }
    /// Whether a checkbox, switch, radio button or toggle button is on (ARIA `aria-checked`
    /// or `aria-pressed`). Leave it unset on controls that can't be.
    pub fn checked(mut self, on: bool) -> Self {
        self.cold_mut().a11y.checked = Some(if on { Checked::True } else { Checked::False });
        self
    }
    /// A checkbox that is partly on: some of what it controls is (ARIA `aria-checked="mixed"`).
    pub fn checked_mixed(mut self) -> Self {
        self.cold_mut().a11y.checked = Some(Checked::Mixed);
        self
    }
    /// A control that can't be used now: assistive technology reads it as dimmed. It also
    /// stops it taking focus; leave its press handler off as well. Use [`El::aria_disabled`]
    /// when the element should announce unavailable state while keeping its focusability intact.
    pub fn disabled(mut self, disabled: bool) -> Self {
        if disabled || !self.cold.is_empty() {
            let cold = self.cold_mut();
            cold.a11y.disabled = disabled;
            let f = &mut *cold.focus;
            f.disabled = disabled;
            if disabled { f.enabled = false; f.opt_out = true; }
        }
        self
    }
    /// Mark this element unavailable to assistive technology (ARIA `aria-disabled`) without
    /// altering its keyboard focusability.
    ///
    /// Unlike [`El::disabled`], calling `aria_disabled(true)` leaves [`El::focusable`],
    /// `Cx::web_focus`, and an earlier [`El::disabled`] untouched, so a pending or unavailable
    /// control can remain in the Tab order and accept assistive focus while activation actions
    /// stay suppressed (WAI-ARIA Authoring Practices: *Focusability of Disabled Controls*).
    /// Leave its press handler off as well.
    ///
    /// ```
    /// # use rune::{el, text};
    /// let pending = el()
    ///     .key("save")
    ///     .role("button")
    ///     .focusable(true)
    ///     .aria_disabled(true)
    ///     .child(text("Saving…"));
    /// ```
    pub fn aria_disabled(mut self, disabled: bool) -> Self {
        if disabled || !self.cold.is_empty() {
            self.cold_mut().a11y.disabled = disabled;
        }
        self
    }
    /// Whether users may select more than one item in a `listbox`, `tree`, `grid`, or
    /// `tablist` (ARIA `aria-multiselectable`).
    ///
    /// ```
    /// # use rune::{col, el, text};
    /// let list = col()
    ///     .key("fruits")
    ///     .role("listbox")
    ///     .multiselectable(true)
    ///     .child(el().key("apple").role("option").child(text("Apple")));
    /// ```
    pub fn multiselectable(mut self, on: bool) -> Self {
        if on || self.cold.a11y.more.is_some() {
            self.cold_mut().a11y.more().multiselectable = on;
        }
        self
    }
    /// Whether user input or selection is required on this control before submitting (ARIA
    /// `aria-required`).
    ///
    /// ```
    /// # use rune::{el, text};
    /// let picker = el().key("country").role("combobox").required(true).child(text("Country"));
    /// ```
    pub fn required(mut self, on: bool) -> Self {
        if on || self.cold.a11y.more.is_some() {
            self.cold_mut().a11y.more().required = on;
        }
        self
    }
    /// The kind of interactive popup this element triggers (ARIA `aria-haspopup`).
    ///
    /// ```
    /// # use rune::{HasPopup, el, text};
    /// let trigger = el()
    ///     .key("actions")
    ///     .role("button")
    ///     .has_popup(HasPopup::Menu)
    ///     .child(text("Actions"));
    /// ```
    pub fn has_popup(mut self, kind: HasPopup) -> Self {
        self.cold_mut().a11y.more().has_popup = Some(kind);
        self
    }
    /// The layout orientation of a composite control, toolbar, scrollbar, or separator (ARIA
    /// `aria-orientation`).
    ///
    /// ```
    /// # use rune::{Orientation, row};
    /// let bar = row().key("tabs").role("tablist").orientation(Orientation::Horizontal);
    /// ```
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.cold_mut().a11y.more().orientation = Some(orientation);
        self
    }
    /// How typing in this control suggests or completes values (ARIA `aria-autocomplete`), on
    /// an editable `combobox`, `textbox`, or `searchbox`. Leave unset for ARIA's implicit
    /// `none` default.
    ///
    /// ```
    /// # use rune::{AutoComplete, HasPopup, TextEdit, TextStyle, text_input};
    /// # let edit = TextEdit::default();
    /// let field = text_input(&edit, TextStyle::new(14.0, 400.0))
    ///     .single_line()
    ///     .key("fruit")
    ///     .role("combobox")
    ///     .label("Fruit")
    ///     .has_popup(HasPopup::Listbox)
    ///     .auto_complete(AutoComplete::List);
    /// ```
    pub fn auto_complete(mut self, mode: AutoComplete) -> Self {
        self.cold_mut().a11y.more().auto_complete = Some(mode);
        self
    }
    /// The element whose contents or presence this element controls (ARIA `aria-controls`),
    /// such as a tab naming its panel or a popup trigger naming its menu or listbox. `key`
    /// resolves nearest-scope-first (own subtree, then enclosing ancestors) with fallback to
    /// root-attached overlays and keyed paths; an empty key names none and costs nothing.
    ///
    /// ```
    /// # use rune::{HasPopup, el, text};
    /// let trigger = el()
    ///     .key("trigger")
    ///     .role("button")
    ///     .has_popup(HasPopup::Listbox)
    ///     .controls("fruit-list")
    ///     .child(text("Fruit"));
    /// ```
    pub fn controls(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        if key.is_empty() {
            if let Some(more) = self.cold.a11y.more.as_ref() && more.controls.is_some() {
                self.cold_mut().a11y.more().controls = None;
            }
            return self;
        }
        self.cold_mut().a11y.more().controls = Some(key);
        self
    }
    /// An element logically owned by this element even when not a tree descendant (ARIA
    /// `aria-owns`), such as a listbox or combobox whose popup options sit in a root-attached
    /// overlay. `key` resolves nearest-scope-first with fallback to root-attached overlays and
    /// keyed paths; an empty key names none and costs nothing.
    ///
    /// ```
    /// # use rune::{col, el};
    /// let combo = col().key("combo").role("combobox").owns("combo-popup").child(el());
    /// ```
    pub fn owns(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        if key.is_empty() {
            if let Some(more) = self.cold.a11y.more.as_ref() && more.owns.is_some() {
                self.cold_mut().a11y.more().owns = None;
            }
            return self;
        }
        self.cold_mut().a11y.more().owns = Some(key);
        self
    }
    /// Leave this element and everything inside it out of the accessibility tree: decoration,
    /// or a duplicate of what another element already says. It still draws and takes input.
    pub fn a11y_hidden(mut self) -> Self {
        self.cold_mut().a11y.hidden = true;
        self
    }
    /// Keep the text in this element and everything inside it out of a web page's selection
    /// (CSS `user-select: none`). A page ([`App::web_document`](crate::App::web_document)) lets
    /// people select and copy its text as any web page does, except text inside something that
    /// takes the pointer: a press, drag or gesture handler, a button, link or field. Use this
    /// for other text that shouldn't select, such as a wordmark drawn as text.
    /// [`El::selectable`] text inside still selects. Outside a web page it changes nothing.
    pub fn unselectable(mut self) -> Self {
        self.cold_mut().a11y.unselectable = true;
        self
    }
    /// How changes to this element's text are announced (ARIA `aria-live`), for status text
    /// that updates while the user does something else.
    pub fn live_region(mut self, live: LiveRegion) -> Self {
        self.cold_mut().a11y.live = Some(live);
        self
    }
    /// This element's place in its set, 1-based, and the set's size (ARIA `aria-posinset` and
    /// `aria-setsize`): a row of a list that builds only some of its rows says where it is.
    /// Item rows of `virtual_rows` and `virtualize` lists get it without this; non-item rows
    /// such as headings do not.
    pub fn position_in_set(mut self, position: usize, size: usize) -> Self {
        self.cold_mut().a11y.more().set = Some((position, size));
        self
    }
    /// Marks a `"list"` element as an ordered list starting at `start` (HTML `<ol start="…">`).
    ///
    /// On web, the accessibility mirror and static HTML export emit `<ol>` (with `start` when
    /// `start != 1`) instead of `<ul>`. Natively, `"listitem"` children receive
    /// `accesskit::ListStyle::Numeric` and `"list-marker"` children expose their number.
    ///
    /// ```
    /// # use rune::{col, row, text};
    /// let _ = col()
    ///     .role("list")
    ///     .list_start(3)
    ///     .child(row().role("listitem").child(text("Third")));
    /// ```
    pub fn list_start(mut self, start: u64) -> Self {
        self.cold_mut().a11y.more().list_start = Some(start);
        self
    }
    /// Total number of rows in a `table`, `grid` or `treegrid` (ARIA `aria-rowcount`), when
    /// not all rows are mounted. Overrides the count a `virtual_rows` container derives from its
    /// rows.
    pub fn row_count(mut self, count: usize) -> Self {
        self.cold_mut().a11y.more().row_count = Some(count);
        self
    }
    /// Total number of columns in a `table`, `grid` or `treegrid` (ARIA `aria-colcount`), when
    /// not all columns are mounted.
    pub fn column_count(mut self, count: usize) -> Self {
        self.cold_mut().a11y.more().column_count = Some(count);
        self
    }
    /// This element's 1-based row index in a `table`, `grid` or `treegrid` (ARIA
    /// `aria-rowindex`), on a `row`, `cell`, `gridcell`, `columnheader` or `rowheader`.
    /// Rows directly under a `virtual_rows` `table` or `grid` get it without this.
    pub fn row_index(mut self, index: usize) -> Self {
        self.cold_mut().a11y.more().row_index = Some(index);
        self
    }
    /// This element's 1-based column index in a `table`, `grid` or `treegrid` (ARIA
    /// `aria-colindex`), on a `row`, `cell`, `gridcell`, `columnheader` or `rowheader`.
    pub fn column_index(mut self, index: usize) -> Self {
        self.cold_mut().a11y.more().column_index = Some(index);
        self
    }
    /// The keyboard shortcut that does what this control does, as the platform writes it
    /// (`"Ctrl+S"`, `"⌘S"`), for assistive technology to read.
    pub fn shortcut_hint(mut self, keys: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().shortcut = Some(keys.into());
        self
    }
    /// Handle assistive-technology actions ([`accesskit::Action`]) sent to descendants that do
    /// not handle them directly.
    ///
    /// Use this on a composite container (`"listbox"`, `"tree"`, `"grid"`, `"table"`, `"menu"`,
    /// `"tablist"`) that owns pointer and keyboard input instead of placing [`El::on_press`] on
    /// every row. While this container is enabled and reachable, actionable descendant items with
    /// composite-item roles (`"option"`, `"treeitem"`, `"row"`, `"cell"`, `"gridcell"`, `"tab"`,
    /// `"menuitem"`, `"menuitemcheckbox"`, `"menuitemradio"`) advertise `Action::Click` in the
    /// accessibility tree and web mirror; decorative and non-item descendants do not.
    ///
    /// When an unhandled element action arrives on an enabled descendant, Rune walks live,
    /// enabled ancestors innermost-first and calls `f` with the target element's key (or its path
    /// when unkeyed) and the requested action. Return `true` to consume the action, or `false` to
    /// let an enclosing `on_a11y_action` handle it.
    ///
    /// ```
    /// # use rune::{accesskit::Action, col, el, text};
    /// # struct State { selected: String }
    /// col().key("list").role("listbox").label("Files")
    ///     .on_a11y_action(|s: &mut State, key, action, _| {
    ///         if action == Action::Click {
    ///             s.selected = key.to_owned();
    ///             return true;
    ///         }
    ///         false
    ///     })
    ///     .child(el().key("o1").role("option").label("README.md").child(text("README.md")));
    /// ```
    pub fn on_a11y_action<S: 'static>(
        mut self,
        f: impl Fn(&mut S, &str, accesskit::Action, &mut crate::el::Tx) -> bool + 'static,
    ) -> Self {
        self.handlers.on_a11y_action = Some(std::rc::Rc::new(move |s, key, action, tx| {
            f(crate::states::downcast::<S>(s), key, action, tx)
        }));
        self
    }
}

impl El {
    /// The element's accessible role. `"textbox"` on a container also makes a press anywhere
    /// on it (padding, a leading icon, a trailing label) focus its one reachable text editor and
    /// place the caret at the nearer end, without blurring a focused field or dragging the
    /// window; buttons and other controls inside keep their own presses. With zero or more
    /// than one reachable editor inside, the container does nothing special.
    #[inline]
    pub fn role(mut self, role: &'static str) -> Self {
        self.set_role(role);
        self
    }
    /// [`El::role`] in place.
    #[inline]
    pub fn set_role(&mut self, role: &'static str) -> &mut Self {
        self.set_role_name(role);
        self
    }
    #[inline]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.set_label(label);
        self
    }
    /// [`El::label`] from a string that lives for the program (a translated message, a
    /// literal): kept as it is, with nothing copied, so a view that runs every frame pays
    /// nothing for its names.
    #[inline]
    pub fn label_static(mut self, label: &'static str) -> Self {
        self.cold_mut().label = Some(Cow::Borrowed(label));
        self
    }
    /// [`El::label`] in place.
    #[inline]
    pub fn set_label(&mut self, label: impl Into<String>) -> &mut Self {
        self.cold_mut().label = Some(Cow::Owned(label.into()));
        self
    }
    /// An outline row's depth for accessibility, 1 at the top (ARIA `aria-level`), as a
    /// `treeitem` carries it. The element tree reports it as `level`.
    pub fn level(mut self, level: u32) -> Self {
        self.cold_mut().a11y.level = Some(level);
        self
    }
    /// Whether a disclosure (a tree row with children, a collapsible header) is open, for
    /// accessibility (ARIA `aria-expanded`). Leave it unset on an element that can't open.
    /// The element tree reports it as `expanded`.
    pub fn expanded(mut self, open: bool) -> Self {
        self.cold_mut().a11y.expanded = Some(open);
        self
    }
}
