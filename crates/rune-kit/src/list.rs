//! List (`@opencode/ui` components/list): an optional search field over grouped rows with an
//! active (highlighted) row, selected check and optional dividers.

use rune::{El, Icon, TextEdit, TextStyle, Tx, col, el, icon, row, text, text_input};

use crate::Kit;

pub struct ListItem {
    label: String,
    group: Option<String>,
    selected: bool,
    actions: Vec<Box<dyn FnOnce(El) -> El>>,
}

impl ListItem {
    pub fn new(label: impl Into<String>) -> ListItem {
        ListItem { label: label.into(), group: None, selected: false, actions: Vec::new() }
    }
    pub fn group(mut self, g: impl Into<String>) -> Self {
        self.group = Some(g.into());
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn on_select<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.actions.push(Box::new(move |e: El| e.on_press_tx(f)));
        self
    }
}

pub struct List {
    items: Vec<ListItem>,
    search: Option<(TextEdit, String)>,
    active: Option<usize>,
    divider: bool,
    active_icon: Option<&'static Icon>,
    empty: String,
    add: Option<El>,
    edit: Option<Box<dyn FnOnce(El) -> El>>,
}

impl List {
    pub fn new() -> List {
        List {
            items: Vec::new(),
            search: None,
            active: Some(0),
            divider: false,
            active_icon: None,
            empty: "No results".into(),
            add: None,
            edit: None,
        }
    }
    pub fn item(mut self, i: ListItem) -> Self {
        self.items.push(i);
        self
    }
    /// A search field bound to `edit` (filtering is the app's job).
    pub fn search(mut self, edit: &TextEdit, placeholder: impl Into<String>) -> Self {
        self.search = Some((edit.clone(), placeholder.into()));
        self
    }
    pub fn on_search_edit<S: 'static>(mut self, f: fn(&mut S) -> &mut TextEdit) -> Self {
        self.edit = Some(Box::new(move |e: El| e.on_edit(f)));
        self
    }
    /// The highlighted row (keyboard focus); `None` for none.
    pub fn active(mut self, i: Option<usize>) -> Self {
        self.active = i;
        self
    }
    pub fn divider(mut self) -> Self {
        self.divider = true;
        self
    }
    /// An icon shown at the end of the active row.
    pub fn active_icon(mut self, i: &'static Icon) -> Self {
        self.active_icon = Some(i);
        self
    }
    pub fn empty_message(mut self, m: impl Into<String>) -> Self {
        self.empty = m.into();
        self
    }
    /// A trailing row (e.g. "Add item").
    pub fn add_row(mut self, e: El) -> Self {
        self.add = Some(e);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let item_font = TextStyle::new(14.0, 500.0).lh(21.0);
        let mut root = col().role("list").w_full().gap(12.0);
        if let Some((edit, placeholder)) = &self.search {
            let mut input = text_input(edit, TextStyle::new(14.0, 400.0).lh(20.0))
                .single_line()
                .key("input")
                .grow(1.0)
                .shrink()
                .color(c.text_strong)
                .placeholder(placeholder.clone(), c.text_weak);
            if let Some(f) = self.edit {
                input = f(input);
            }
            root = root.child(
                row().key("search-wrapper").w_full().mb(4.0).gap(8.0).center_y().child(
                    row()
                        .key("search")
                        .grow(1.0)
                        .padding(8.0)
                        .gap(8.0)
                        .center_y()
                        .radius(k.radius.md)
                        .fill(c.surface_base)
                        .child(icon(k.glyph.search, 16.0).color(c.std_icon))
                        .child(input),
                ),
            );
        }
        let mut scroll = col().key("scroll").w_full().gap(12.0);
        if self.items.is_empty() && self.add.is_none() {
            scroll = scroll.child(
                col()
                    .key("empty")
                    .w_full()
                    .px(48.0)
                    .py(32.0)
                    .center()
                    .child(text(self.empty).font(TextStyle::new(14.0, 400.0).lh(21.0)).color(c.text_weak)),
            );
        }
        let n = self.items.len();
        let mut group_el: Option<(Option<String>, El)> = None;
        let mut groups = Vec::new();
        for (i, item) in self.items.into_iter().enumerate() {
            if group_el.as_ref().is_none_or(|(g, _)| *g != item.group) {
                if let Some(g) = group_el.take() {
                    groups.push(g.1);
                }
                let mut g = col().key(format!("group{i}")).w_full();
                if let Some(name) = &item.group {
                    g =
                        g.child(
                            row().key("header").w_full().pt(8.0).pb(8.0).pl(8.0).pr(12.0).fill(c.surface_raised).child(
                                text(name.clone()).font(TextStyle::new(14.0, 500.0).lh(21.0)).color(c.text_weak),
                            ),
                        );
                }
                group_el = Some((item.group.clone(), g));
            }
            let active = self.active == Some(i);
            let mut r = row()
                .key(format!("item{i}"))
                .role("option")
                .label(item.label.clone())
                .w_full()
                .px(8.0)
                .py(6.0)
                .center_y()
                .radius(k.radius.md)
                .gap(4.0)
                .child(text(item.label).font(item_font).color(c.text_strong).shrink().truncate());
            if item.selected {
                r = r.child(icon(k.glyph.check_small, 16.0).color(c.std_icon_strong));
            }
            if active {
                r = r.fill(c.surface_raised_hover);
                if let Some(ai) = self.active_icon {
                    r = r.child(icon(ai, 16.0).color(c.std_icon_strong));
                }
            } else {
                r = r.hover(c.surface_raised_hover);
            }
            if self.divider && i + 1 < n {
                r = r
                    .child(el().key("divider").h(1.0).pin(Some(16.0), None, Some(16.0), Some(0.0)).fill(c.border_weak));
            }
            for a in item.actions {
                r = a(r);
            }
            if let Some((_, g)) = group_el.as_mut() {
                *g = std::mem::replace(g, col()).child(r);
            }
        }
        if let Some((_, g)) = group_el.take() {
            groups.push(g);
        }
        if let Some(add) = self.add {
            groups.push(col().key("add").w_full().child(add));
        }
        let count = groups.len();
        for (i, g) in groups.into_iter().enumerate() {
            scroll = scroll.child(if i + 1 == count { g.pb(12.0) } else { g });
        }
        root.child(scroll)
    }
}

impl Default for List {
    fn default() -> Self {
        Self::new()
    }
}
