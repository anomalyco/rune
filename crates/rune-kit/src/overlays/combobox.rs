//! Searchable select: Popover + TextInput + virtual rows. Keyboard contract follows
//! @opencode/ui's use-filtered-list: looping arrows, Ctrl-N/P, composition-safe Enter,
//! and editor modifiers left to the text field. Fuzzy scores are Rune's own ordering.
use std::rc::Rc;
use rune::machine::Effects;
use rune::{El, TextEdit, Tx, col, el, icon, row};
use super::popup_list::{PopupHandlers, PopupListCore};
use crate::list_nav::{NavAction, NavPolicy, page_size};
use crate::{Kit, Placement, Popover, PopupItem, Select, SelectAppearance, TextInput};

#[derive(Clone, Debug)]
pub struct ComboboxOption {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub disabled: bool,
}
impl ComboboxOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), detail: String::new(), disabled: false }
    }
}

/// A combobox's popover and highlight, owned by the app (`ComboboxState::default()`), as one
/// explicit machine: closed, or open with its search field focused and a highlighted row. The
/// highlight is the row the keys or the pointer chose, or none, which is the first enabled row
/// the query lists: an edit to the query lets go of a chosen row, as Desktop's list does
/// (`ui/src/hooks/list-machine.ts`), so deleting back to an earlier query never brings back a
/// row chosen for it.
#[derive(Clone, Debug, Default)]
pub struct ComboboxState {
    search: TextEdit,
    phase: ComboboxPhase,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ComboboxPhase {
    #[default]
    Closed,
    /// `highlight`: the row the keys or the pointer chose; none is the query's first enabled row.
    Open { highlight: Option<String> },
}

pub enum ComboboxEvent<'a> {
    /// The trigger: opens on `current` (the chosen option), or closes.
    Toggle { current: Option<&'a str> },
    /// Escape, a press outside, a choice.
    Close,
    /// The query changed.
    Edited,
    /// The keys (`reveal`, which scrolls it into view) or the pointer moved the highlight to row `id`.
    Highlight { id: String, reveal: bool },
}

#[derive(Debug, PartialEq, Eq)]
pub enum ComboboxEffect {
    /// Scroll row `id` into view.
    Reveal(String),
}

impl ComboboxState {
    pub fn is_open(&self) -> bool { matches!(self.phase, ComboboxPhase::Open { .. }) }
    pub fn phase(&self) -> &ComboboxPhase { &self.phase }
    pub fn search(&self) -> &TextEdit { &self.search }
    /// The row the keys or the pointer chose, while open.
    pub fn active(&self) -> Option<&str> {
        match &self.phase {
            ComboboxPhase::Open { highlight } => highlight.as_deref(),
            ComboboxPhase::Closed => None,
        }
    }
    pub fn close(&mut self) { self.step(ComboboxEvent::Close); }

    pub fn step(&mut self, event: ComboboxEvent<'_>) -> Effects<ComboboxEffect> {
        match (&mut self.phase, event) {
            (ComboboxPhase::Closed, ComboboxEvent::Toggle { current }) => {
                self.search.clear();
                self.search.focused = true;
                self.phase = ComboboxPhase::Open { highlight: current.map(str::to_owned) };
                current.map_or(Effects::none(), |id| Effects::one(ComboboxEffect::Reveal(id.to_owned())))
            }
            (ComboboxPhase::Open { .. }, ComboboxEvent::Toggle { .. } | ComboboxEvent::Close) => {
                self.phase = ComboboxPhase::Closed;
                self.search.focused = false;
                self.search.cancel_composition();
                self.search.break_history_group();
                Effects::none()
            }
            (ComboboxPhase::Open { highlight }, ComboboxEvent::Edited) => {
                *highlight = None;
                Effects::none()
            }
            (ComboboxPhase::Open { highlight }, ComboboxEvent::Highlight { id, reveal }) => {
                *highlight = Some(id.clone());
                if reveal { Effects::one(ComboboxEffect::Reveal(id)) } else { Effects::none() }
            }
            (ComboboxPhase::Closed, _) => Effects::none(),
        }
    }

    fn active_index(&self, options: &[ComboboxOption], filtered: &[usize]) -> Option<usize> {
        if let Some(index) = filtered.iter().copied().find(|i| !options[*i].disabled && Some(options[*i].id.as_str()) == self.active()) {
            return Some(index);
        }
        filtered.iter().copied().find(|i| !options[*i].disabled)
    }
}

// Case-insensitive subsequence matching; contiguous matches and word starts rank first.
fn score(query: &str, value: &str) -> Option<i32> {
    let needle: Vec<_> = query.chars().collect();
    let mut n = 0;
    let mut score = 0;
    let mut previous = false;
    let mut boundary = true;
    for c in value.to_lowercase().chars() {
        if n < needle.len() && c == needle[n] {
            score += 10 + if previous { 12 } else { 0 } + if boundary { 8 } else { 0 };
            previous = true;
            n += 1;
        } else { previous = false; score -= 1; }
        boundary = !c.is_alphanumeric();
    }
    (n == needle.len()).then_some(score)
}
fn filtered(options: &[ComboboxOption], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() { return (0..options.len()).collect(); }
    let mut matches: Vec<_> = options.iter().enumerate().filter_map(|(i, o)| {
        score(&query, &o.label).into_iter().chain(score(&query, &o.detail)).max().map(|s| (i, s))
    }).collect();
    matches.sort_by_key(|(i, s)| (std::cmp::Reverse(*s), *i));
    matches.into_iter().map(|(i, _)| i).collect()
}

enum SearchKeyOutcome {
    Ignored,
    Handled,
    Choose(String),
}

fn toggle_combobox(state: &mut ComboboxState, current: Option<&str>, key: &str, tx: &mut Tx) {
    for effect in state.step(ComboboxEvent::Toggle { current }) {
        match effect { ComboboxEffect::Reveal(id) => tx.scroll_into_view(format!("{key}-option-{id}")) }
    }
}

fn on_search_change(state: &mut ComboboxState, options: &[ComboboxOption], key: &str, tx: &mut Tx) {
    state.step(ComboboxEvent::Edited);
    if let Some(i) = filtered(options, &state.search.text).into_iter().find(|i| !options[*i].disabled) {
        tx.scroll_into_view(format!("{key}-option-{}", options[i].id));
    }
}

fn handle_search_key(state: &mut ComboboxState, options: &[ComboboxOption], key: &str, page: usize, ev: &rune::KeyEvent, tx: &mut Tx) -> SearchKeyOutcome {
    if state.search.preedit().is_some() { return SearchKeyOutcome::Ignored; }
    let Some(action) = NavPolicy::search_list(state.search.text.is_empty()).decode(ev, page) else {
        return SearchKeyOutcome::Ignored;
    };
    if action == NavAction::Escape {
        if !tx.key_repeat() { state.close(); }
        return SearchKeyOutcome::Handled;
    }
    let choices = filtered(options, &state.search.text);
    let enabled: Vec<_> = choices.into_iter().filter(|i| !options[*i].disabled).collect();
    let active = state.active_index(options, &enabled);
    match action {
        NavAction::Activate => {
            if tx.key_repeat() { return SearchKeyOutcome::Handled; }
            let selected = active.map(|i| options[i].id.clone());
            state.close();
            selected.map_or(SearchKeyOutcome::Handled, SearchKeyOutcome::Choose)
        }
        NavAction::Move { step, .. } => {
            if !enabled.is_empty() {
                let index = active.and_then(|i| enabled.iter().position(|v| *v == i));
                let next = step.target(index, enabled.len(), true).unwrap();
                let id = options[enabled[next]].id.clone();
                for effect in state.step(ComboboxEvent::Highlight { id, reveal: true }) {
                    match effect { ComboboxEffect::Reveal(id) => tx.scroll_into_view(format!("{key}-option-{id}")) }
                }
            }
            SearchKeyOutcome::Handled
        }
        NavAction::Escape | NavAction::SelectAll => SearchKeyOutcome::Handled,
    }
}

fn on_option_hover(state: &mut ComboboxState, options: &[ComboboxOption], choices: &[usize], i: usize) {
    if let Some(&orig) = choices.get(i) {
        state.step(ComboboxEvent::Highlight { id: options[orig].id.clone(), reveal: false });
    }
}

type Access<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut ComboboxState>;
type Choose<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;

trait ComboboxBind {
    fn bind_trigger(&self, trigger: El, current: Option<String>, key: String) -> El;
    fn bind_popover(&self, popover: Popover) -> Popover;
    fn bind_field(&self, field: TextInput, options: Rc<Vec<ComboboxOption>>, key: String, page: usize) -> TextInput;
    fn build_list(&self, list: PopupListCore, options: Rc<Vec<ComboboxOption>>, choices: Vec<usize>, k: &Kit) -> El;
}

struct ComboboxHandlers<S> {
    access: Access<S>,
    choose: Option<Choose<S>>,
}

impl<S: 'static> ComboboxBind for ComboboxHandlers<S> {
    fn bind_trigger(&self, trigger: El, current: Option<String>, key: String) -> El {
        let access = self.access.clone();
        trigger.on_press_tx(move |s: &mut S, tx| toggle_combobox(access(s), current.as_deref(), &key, tx))
    }

    fn bind_popover(&self, popover: Popover) -> Popover {
        let access = self.access.clone();
        popover.on_close(move |s: &mut S| access(s).close())
    }

    fn bind_field(&self, field: TextInput, options: Rc<Vec<ComboboxOption>>, key: String, page: usize) -> TextInput {
        let (edit_access, change_access, key_access) = (self.access.clone(), self.access.clone(), self.access.clone());
        let (change_options, change_key, choose) = (options.clone(), key.clone(), self.choose.clone());
        field
            .on_edit(move |s: &mut S| &mut edit_access(s).search)
            .on_change(move |s: &mut S, tx| on_search_change(change_access(s), &change_options, &change_key, tx))
            .on_key(move |s: &mut S, ev, tx| match handle_search_key(key_access(s), &options, &key, page, ev, tx) {
                SearchKeyOutcome::Ignored => false,
                SearchKeyOutcome::Handled => true,
                SearchKeyOutcome::Choose(selected) => {
                    if let Some(choose) = &choose { choose(s, &selected, tx); }
                    true
                }
            })
    }

    fn build_list(&self, list: PopupListCore, options: Rc<Vec<ComboboxOption>>, choices: Vec<usize>, k: &Kit) -> El {
        let (hover_access, choose_access, choose) = (self.access.clone(), self.access.clone(), self.choose.clone());
        let handlers: PopupHandlers<S> = PopupHandlers {
            on_active: Some(Rc::new(move |s: &mut S, i| on_option_hover(hover_access(s), &options, &choices, i))),
            on_choose: Some(Rc::new(move |s: &mut S, id: &str, tx: &mut Tx| {
                choose_access(s).close();
                if let Some(choose) = &choose { choose(s, id, tx); }
            })),
            on_close: None,
        };
        list.build_list(k, &handlers)
    }
}

struct ComboboxCore {
    key: String,
    state: ComboboxState,
    options: Rc<Vec<ComboboxOption>>,
    current: Option<String>,
    placeholder: String,
    search_placeholder: String,
    search_label: Option<String>,
    empty: String,
    width: f32,
    max_height: f32,
    placement: Placement,
    trigger: Option<El>,
    disabled: bool,
    animate_filtering: bool,
    label: Option<String>,
}

impl ComboboxCore {
    fn new(key: String, state: &ComboboxState, options: &[ComboboxOption]) -> Self {
        Self {
            key,
            state: state.clone(),
            options: Rc::new(options.to_vec()),
            current: None,
            placeholder: String::new(),
            search_placeholder: "Search".into(),
            search_label: None,
            empty: "No results".into(),
            width: 280.0,
            max_height: 192.0,
            placement: Placement::BottomStart,
            trigger: None,
            disabled: false,
            animate_filtering: false,
            label: None,
        }
    }

    fn build(self, k: &Kit, layer: &mut Vec<El>, bind: &dyn ComboboxBind) -> El {
        let selected = self.options.iter().find(|o| Some(&o.id) == self.current.as_ref());
        let name = self.label.clone().unwrap_or_else(|| self.placeholder.clone());
        let mut trigger = match self.trigger {
            Some(trigger) => trigger,
            None => Select::new::<&str>(format!("{}-value", self.key), &[])
                .current(selected.map(|o| o.label.as_str())).placeholder(self.placeholder).label(name.clone())
                .appearance(SelectAppearance::Field).width(self.width).disabled(self.disabled)
                .trigger(k),
        };
        if !self.disabled {
            trigger = bind.bind_trigger(trigger, self.current.clone(), self.key.clone());
        }
        let popover = bind.bind_popover(
            Popover::new(&self.key).trigger(trigger).label(name.clone()).placement(self.placement).padding(0.0)
                .width(self.width)
                .open(self.state.is_open() && !self.disabled),
        );
        if !self.state.is_open() || self.disabled { return popover.build(k, layer); }
        let choices = filtered(&self.options, &self.state.search.text);
        let active = self.state.active_index(&self.options, &choices);
        let page = page_size(self.max_height, k.popup_row_metrics().height);
        // The search field sits borderless over a divider: the open popover is its frame, and the
        // caret shows its focus.
        let search_label = self.search_label
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| if name.is_empty() { "Search".to_owned() } else { format!("Search {name}") });
        let mut field = TextInput::new(&self.state.search).plain().leading_icon(k.glyph.search).height(36.0)
            .width(None).placeholder(self.search_placeholder).label(search_label)
            .auto_complete(rune::AutoComplete::List)
            .active_descendant(active.map(|i| format!("{}-option-{}", self.key, self.options[i].id)));
        // Without it, an edit's commit is instant: the filtered rows and the height land with the text.
        if self.animate_filtering { field = field.animate_edit_layout(); }
        let field = bind.bind_field(field, self.options.clone(), self.key.clone(), page).build(k).key("search");
        let active_pos = active.and_then(|orig| choices.iter().position(|&c| c == orig));
        let items: Vec<_> = choices.iter().map(|&i| {
            let option = &self.options[i];
            PopupItem::new(&option.id, &option.label).description(&option.detail).disabled(option.disabled).trailing(
                icon(k.glyph.check_small, 14.0)
                    .color(k.c.icon_base)
                    .opacity(if self.current.as_deref() == Some(option.id.as_str()) { 1.0 } else { 0.0 }),
            )
        }).collect();
        let mut list = PopupListCore::new(self.key, String::new());
        list.label = Some(name);
        list.max_height = self.max_height;
        list.items = items;
        list.active = active_pos;
        list.empty = self.empty;
        let list = bind.build_list(list, self.options, choices, k);
        let results = col().key("results").w_full().padding(4.0).shrink().min_h(0.0).child(list);
        let body = col().w_full().shrink().min_h(0.0)
            .child(row().key("header").w_full().px(4.0).child(field))
            .child(el().key("divider").w_full().h(1.0).fill(k.c.border_weak).a11y_hidden())
            .child(results);
        popover.child(body).build(k, layer)
    }
}

/// `Combobox::new("models", &s.combo, &options, |s: &mut App| &mut s.combo)`.
/// Selection is app-controlled through `current` and `on_choose`; search/open/highlight
/// state belongs to the supplied ComboboxState. Only mounted rows are laid out/shaped.
pub struct Combobox<S> {
    core: ComboboxCore,
    handlers: ComboboxHandlers<S>,
}
impl<S: 'static> Combobox<S> {
    pub fn new(key: impl Into<String>, state: &ComboboxState, options: &[ComboboxOption],
        access: impl for<'a> Fn(&'a mut S) -> &'a mut ComboboxState + 'static) -> Self {
        Self { core: ComboboxCore::new(key.into(), state, options), handlers: ComboboxHandlers { access: Rc::new(access), choose: None } }
    }
    /// The accessible name: what the choice is for ("Model"). The chosen option is read as its
    /// value; without it the placeholder names the field.
    pub fn label(mut self, text: impl Into<String>) -> Self { self.core.label = Some(text.into()); self }
    pub fn current(mut self, id: Option<&str>) -> Self { self.core.current = id.map(str::to_owned); self }
    pub fn placeholder(mut self, text: impl Into<String>) -> Self { self.core.placeholder = text.into(); self }
    pub fn search_placeholder(mut self, text: impl Into<String>) -> Self { self.core.search_placeholder = text.into(); self }
    /// The accessible name for the popover's search input (the web's `aria-label`), for
    /// localisation or a more specific prompt than the default `"Search"` / `"Search <label>"`.
    /// Accessibility-only (not drawn); blank values fall back to the default.
    ///
    /// ```
    /// # use rune_kit::{Combobox, ComboboxOption, ComboboxState, Kit};
    /// # struct App { combo: ComboboxState }
    /// # let s = App { combo: ComboboxState::default() };
    /// # let options: [ComboboxOption; 0] = [];
    /// # let kit = Kit::neutral(true);
    /// # let mut layer = Vec::new();
    /// Combobox::new("models", &s.combo, &options, |s: &mut App| &mut s.combo)
    ///     .label("Modèle")
    ///     .search_placeholder("Rechercher des modèles…")
    ///     .search_label("Rechercher des modèles")
    ///     .build(&kit, &mut layer);
    /// ```
    pub fn search_label(mut self, text: impl Into<String>) -> Self { self.core.search_label = Some(text.into()); self }
    pub fn empty_message(mut self, text: impl Into<String>) -> Self { self.core.empty = text.into(); self }
    /// The popover's width, 280 by default; the trigger keeps its own.
    pub fn width(mut self, width: f32) -> Self { self.core.width = width; self }
    /// The tallest the result list grows, 192 by default. A window with less room below (or
    /// above) the trigger shortens it further, and it scrolls.
    pub fn max_height(mut self, height: f32) -> Self { self.core.max_height = height; self }
    pub fn placement(mut self, placement: Placement) -> Self { self.core.placement = placement; self }
    /// Replaces the default field trigger (the chosen label and a chevron) with any element;
    /// pressing it opens and closes the popover.
    pub fn trigger(mut self, trigger: El) -> Self { self.core.trigger = Some(trigger); self }
    pub fn disabled(mut self, disabled: bool) -> Self { self.core.disabled = disabled; self }
    /// Animate each keystroke's filtering: the popover springs to its new height and the
    /// rows crossfade, as [`crate::PopupList::animate_filtering`]. Off by default, so the rows and
    /// the height a keystroke changes land in the same frame.
    pub fn animate_filtering(mut self, animate: bool) -> Self { self.core.animate_filtering = animate; self }
    pub fn on_choose(mut self, choose: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.handlers.choose = Some(Rc::new(choose)); self
    }
    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        self.core.build(k, layer, &self.handlers)
    }
}
