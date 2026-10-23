//! Searchable select: Popover + TextInput + virtual rows. Keyboard contract follows
//! @opencode/ui's use-filtered-list: looping arrows, Ctrl-N/P, composition-safe Enter,
//! and editor modifiers left to the text field. Fuzzy scores are Rune's own ordering.
use std::rc::Rc;
use rune::{El, GrowthSpec, Key, ScrollSpec, TextEdit, Tx, col, el, icon, row, text};
use crate::{Kit, Placement, Popover, Select, SelectAppearance, TextInput};

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
    disabled: bool,
}
impl<S: 'static> Combobox<S> {
    pub fn new(key: impl Into<String>, state: &ComboboxState, options: &[ComboboxOption],
        access: impl for<'a> Fn(&'a mut S) -> &'a mut ComboboxState + 'static) -> Self {
        Self { key: key.into(), state: state.clone(), options: Rc::new(options.to_vec()), access: Rc::new(access),
            choose: None, current: None, placeholder: String::new(), search_placeholder: "Search".into(),
            empty: "No results".into(), width: 280.0, disabled: false }
    }
    pub fn current(mut self, id: Option<&str>) -> Self { self.current = id.map(str::to_owned); self }
    pub fn placeholder(mut self, text: impl Into<String>) -> Self { self.placeholder = text.into(); self }
    pub fn search_placeholder(mut self, text: impl Into<String>) -> Self { self.search_placeholder = text.into(); self }
    pub fn empty_message(mut self, text: impl Into<String>) -> Self { self.empty = text.into(); self }
    pub fn width(mut self, width: f32) -> Self { self.width = width; self }
    pub fn disabled(mut self, disabled: bool) -> Self { self.disabled = disabled; self }
    pub fn on_choose(mut self, choose: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.choose = Some(Rc::new(choose)); self
    }
    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let selected = self.options.iter().find(|o| Some(&o.id) == self.current.as_ref());
        let access = self.access.clone();
        let current = self.current.clone();
        let key = self.key.clone();
        let open_key = key.clone();
        let mut trigger = Select::new::<&str>(format!("{}-value", self.key), &[])
            .current(selected.map(|o| o.label.as_str())).placeholder(self.placeholder)
            .appearance(SelectAppearance::Field).width(self.width).disabled(self.disabled)
            .build(k, &mut Vec::new());
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
        let mut popover = Popover::new(&self.key).trigger(trigger).placement(Placement::BottomStart)
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
        let field = TextInput::new(&self.state.search).width(None).placeholder(self.search_placeholder)
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
                if ev.key == Key::Escape { state.close(); return true; }
                if ev.mods.alt || ev.mods.logo { return false; }
                let choices = filtered(&options, &state.search.text);
                let enabled: Vec<_> = choices.into_iter().filter(|i| !options[*i].disabled).collect();
                let active = state.active_index(&options, &enabled);
                if ev.key == Key::Enter {
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
        let mut rows = col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(28.0);
        for i in choices.iter().copied() {
            let option = &self.options[i];
            let id = option.id.clone();
            let access = self.access.clone();
            let choose = self.choose.clone();
            let hover_access = self.access.clone();
            let hover_id = id.clone();
            let mut item = row().key(format!("{}-option-{}", self.key, id)).role("option").label(&option.label)
                .w_full().h(28.0).px(8.0).gap(8.0).center_y().radius(4.0)
                .fill(if active == Some(i) { k.c.hover } else { rune::Color::TRANSPARENT })
                .child(text(&option.label).font(k.ty.ui_compact).color(k.c.text_base).grow(1.0).shrink().truncate())
                .child(icon(k.glyph.check_small, 14.0).color(k.c.icon_base)
                    .opacity(if self.current.as_deref() == Some(id.as_str()) { 1.0 } else { 0.0 }));
            if option.disabled { item = item.dim(0.5); } else {
                item = item.on_hover(move |s: &mut S| {
                    let state = hover_access(s);
                    state.active = Some(hover_id.clone()); state.query = state.search.text.clone();
                }).on_press_tx(move |s: &mut S, tx| {
                    access(s).close();
                    if let Some(choose) = &choose { choose(s, &id, tx); }
                });
            }
            rows = rows.child(item);
        }
        let list = if choices.is_empty() {
            el().h(28.0).center_y().child(text(self.empty).font(k.ty.ui_compact).color(k.c.text_weak))
        } else {
            col().key("viewport").role("listbox").w_full().h((choices.len() as f32 * 28.0).min(192.0))
                .scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0).child(rows)
        };
        popover.child(col().w(self.width).gap(8.0).child(field).child(list)).build(k, layer)
    }
}
