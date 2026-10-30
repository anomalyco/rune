//! Searchable select: Popover + TextInput + virtual rows. Keyboard contract follows
//! @opencode/ui's use-filtered-list: looping arrows, Ctrl-N/P, composition-safe Enter,
//! and editor modifiers left to the text field. Fuzzy scores are Rune's own ordering.
use std::rc::Rc;
use rune::{El, Key, TextEdit, Tx, col, el, icon, row};
use crate::{Kit, Placement, Popover, PopupItem, PopupList, Select, SelectAppearance, TextInput};

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

#[derive(Clone, Debug, Default)]
pub struct ComboboxState {
    pub open: bool,
    pub search: TextEdit,
    pub active: Option<String>,
    query: String,
}
impl ComboboxState {
    pub fn close(&mut self) {
        self.open = false;
        self.search.focused = false;
        self.search.preedit = None;
        self.search.break_history_group();
    }
    fn active_index(&self, options: &[ComboboxOption], filtered: &[usize]) -> Option<usize> {
        if self.query == self.search.text
            && let Some(index) = filtered.iter().copied().find(|i| !options[*i].disabled && Some(&options[*i].id) == self.active.as_ref())
        { return Some(index); }
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

type Access<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut ComboboxState>;
type Choose<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;

/// `Combobox::new("models", &s.combo, &options, |s: &mut App| &mut s.combo)`.
/// Selection is app-controlled through `current` and `on_choose`; search/open/highlight
/// state belongs to the supplied ComboboxState. Only mounted rows are laid out/shaped.
pub struct Combobox<S> {
    key: String,
    state: ComboboxState,
    options: Rc<Vec<ComboboxOption>>,
    access: Access<S>,
    choose: Option<Choose<S>>,
    current: Option<String>,
    placeholder: String,
    search_placeholder: String,
    empty: String,
    width: f32,
    max_height: f32,
    placement: Placement,
    trigger: Option<El>,
    disabled: bool,
    animate_filtering: bool,
}
impl<S: 'static> Combobox<S> {
    pub fn new(key: impl Into<String>, state: &ComboboxState, options: &[ComboboxOption],
        access: impl for<'a> Fn(&'a mut S) -> &'a mut ComboboxState + 'static) -> Self {
        Self { key: key.into(), state: state.clone(), options: Rc::new(options.to_vec()), access: Rc::new(access),
            choose: None, current: None, placeholder: String::new(), search_placeholder: "Search".into(),
            empty: "No results".into(), width: 280.0, max_height: 192.0, placement: Placement::BottomStart,
            trigger: None, disabled: false, animate_filtering: false }
    }
    pub fn current(mut self, id: Option<&str>) -> Self { self.current = id.map(str::to_owned); self }
    pub fn placeholder(mut self, text: impl Into<String>) -> Self { self.placeholder = text.into(); self }
    pub fn search_placeholder(mut self, text: impl Into<String>) -> Self { self.search_placeholder = text.into(); self }
    pub fn empty_message(mut self, text: impl Into<String>) -> Self { self.empty = text.into(); self }
    /// The popover's width, 280 by default; the trigger keeps its own.
    pub fn width(mut self, width: f32) -> Self { self.width = width; self }
    /// The tallest the result list grows, 192 by default. A window with less room below (or
    /// above) the trigger shortens it further, and it scrolls.
    pub fn max_height(mut self, height: f32) -> Self { self.max_height = height; self }
    pub fn placement(mut self, placement: Placement) -> Self { self.placement = placement; self }
    /// Replaces the default field trigger (the chosen label and a chevron) with any element;
    /// pressing it opens and closes the popover.
    pub fn trigger(mut self, trigger: El) -> Self { self.trigger = Some(trigger); self }
    pub fn disabled(mut self, disabled: bool) -> Self { self.disabled = disabled; self }
    /// Animate each keystroke's filtering: the popover springs to its new height and the
    /// rows crossfade, as [`PopupList::animate_filtering`]. Off by default, so the rows and
    /// the height a keystroke changes land in the same frame.
    pub fn animate_filtering(mut self, animate: bool) -> Self { self.animate_filtering = animate; self }
    pub fn on_choose(mut self, choose: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.choose = Some(Rc::new(choose)); self
    }
    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let selected = self.options.iter().find(|o| Some(&o.id) == self.current.as_ref());
        let access = self.access.clone();
        let current = self.current.clone();
        let key = self.key.clone();
        let open_key = key.clone();
        let mut trigger = match self.trigger {
            Some(trigger) => trigger,
            None => Select::new::<&str>(format!("{}-value", self.key), &[])
                .current(selected.map(|o| o.label.as_str())).placeholder(self.placeholder)
                .appearance(SelectAppearance::Field).width(self.width).disabled(self.disabled)
                .trigger(k),
        };
        if !self.disabled {
            trigger = trigger.on_press_tx(move |s: &mut S, tx| {
                let state = access(s);
                state.open = !state.open;
                state.search.clear();
                state.search.focused = state.open;
                state.query.clear();
                state.active = current.clone();
                if state.open && let Some(id) = &current { tx.scroll_into_view(format!("{open_key}-option-{id}")); }
            });
        }
        let mut popover = Popover::new(&self.key).trigger(trigger).placement(self.placement).padding(0.0)
            .open(self.state.open && !self.disabled);
        let access = self.access.clone();
        popover = popover.on_close(move |s: &mut S| access(s).close());
        if !self.state.open || self.disabled { return popover.build(k, layer); }
        let choices = filtered(&self.options, &self.state.search.text);
        let active = self.state.active_index(&self.options, &choices);
        let access = self.access.clone();
        let key_access = self.access.clone();
        let options = self.options.clone();
        let choose = self.choose.clone();
        let change_access = self.access.clone();
        let change_options = self.options.clone();
        let change_key = self.key.clone();
        // The search field sits borderless over a divider: the open popover is its frame, and the
        // caret shows its focus.
        let mut field = TextInput::new(&self.state.search).plain().leading_icon(k.glyph.search).height(36.0)
            .width(None).placeholder(self.search_placeholder);
        // Without it, an edit's commit is instant: the filtered rows and the height land with the text.
        if self.animate_filtering { field = field.animate_edit_layout(); }
        let field = field
            .on_edit(move |s: &mut S| &mut access(s).search)
            .on_change(move |s: &mut S, tx| {
                let state = change_access(s);
                state.active = None;
                state.query = state.search.text.clone();
                if let Some(i) = filtered(&change_options, &state.search.text).into_iter().find(|i| !change_options[*i].disabled) {
                    tx.scroll_into_view(format!("{change_key}-option-{}", change_options[i].id));
                }
            })
            .on_key(move |s: &mut S, ev, tx| {
                let state = key_access(s);
                if state.search.preedit.is_some() { return false; }
                if ev.key == Key::Escape {
                    if !tx.key_repeat() { state.close(); }
                    return true;
                }
                if ev.mods.alt || ev.mods.logo { return false; }
                let choices = filtered(&options, &state.search.text);
                let enabled: Vec<_> = choices.into_iter().filter(|i| !options[*i].disabled).collect();
                let active = state.active_index(&options, &enabled);
                if ev.key == Key::Enter {
                    if tx.key_repeat() { return true; }
                    let selected = active.map(|i| options[i].id.clone());
                    state.close();
                    if let (Some(selected), Some(choose)) = (selected, &choose) { choose(s, &selected, tx); }
                    return true;
                }
                let control = ev.mods.ctrl && !ev.mods.shift;
                let delta = match &ev.key {
                    Key::Down if !ev.mods.cmd && !ev.mods.ctrl => 1,
                    Key::Up if !ev.mods.cmd && !ev.mods.ctrl => -1,
                    Key::Text(c) if control && c == "n" => 1,
                    Key::Text(c) if control && c == "p" => -1,
                    Key::Command('n') if control => 1,
                    Key::Command('p') if control => -1,
                    _ => return false,
                };
                if !enabled.is_empty() {
                    let index = active.and_then(|i| enabled.iter().position(|v| *v == i)).unwrap_or(0);
                    let next = (index as isize + delta).rem_euclid(enabled.len() as isize) as usize;
                    let id = options[enabled[next]].id.clone();
                    tx.scroll_into_view(format!("{key}-option-{id}"));
                    state.active = Some(id);
                    state.query = state.search.text.clone();
                }
                true
            }).build(k).key("search");
        let active_pos = active.and_then(|orig| choices.iter().position(|&c| c == orig));
        let items: Vec<_> = choices.iter().map(|&i| {
            let option = &self.options[i];
            PopupItem::new(&option.id, &option.label).description(&option.detail).disabled(option.disabled).trailing(
                icon(k.glyph.check_small, 14.0)
                    .color(k.c.icon_base)
                    .opacity(if self.current.as_deref() == Some(option.id.as_str()) { 1.0 } else { 0.0 }),
            )
        }).collect();
        let hover_access = self.access.clone();
        let hover_options = self.options.clone();
        let hover_choices = choices;
        let choose_access = self.access;
        let choose = self.choose;
        let list = PopupList::new(&self.key, "")
            .max_height(self.max_height)
            .items(items)
            .active(active_pos)
            .empty_message(self.empty)
            .on_active(move |s: &mut S, i| {
                if let Some(&orig) = hover_choices.get(i) {
                    let state = hover_access(s);
                    state.active = Some(hover_options[orig].id.clone());
                    state.query = state.search.text.clone();
                }
            })
            .on_choose(move |s: &mut S, id, tx| {
                choose_access(s).close();
                if let Some(choose) = &choose {
                    choose(s, id, tx);
                }
            })
            .build_list(k);
        let results = col().key("results").w_full().padding(4.0).shrink().min_h(0.0).child(list);
        let body = col().w(self.width).shrink().min_h(0.0)
            .child(row().key("header").w_full().px(4.0).child(field))
            .child(el().key("divider").w_full().h(1.0).fill(k.c.border_weak))
            .child(results);
        popover.child(body).build(k, layer)
    }
}
