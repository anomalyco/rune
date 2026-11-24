//! Anchored popup listbox (`PopupList` / `PopupItem`) for popovers filtered by an outside
//! field, such as a chat composer's `@`-mention and `/`-command popovers, and the shared list
//! inside [`crate::Combobox`].

use std::rc::Rc;

use rune::{Align, Color, El, GrowthSpec, Side, Transition, Tx, col, el, row, text};

use super::overlay::{Placement, content_key, push_raised_anchored, scrim, scrim_key};
use super::popover::{fit_scroll, scroll_area_key};
use crate::{ComboboxOption, Kit};

/// One row in a [`PopupList`]: `PopupItem::new("review", "/review").description("Review changes")`.
#[derive(Clone)]
pub struct PopupItem {
    id: String,
    label: String,
    label_el: Option<El>,
    description: Option<String>,
    leading: Option<El>,
    trailing: Option<El>,
    disabled: bool,
}

impl PopupItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            label_el: None,
            description: None,
            leading: None,
            trailing: None,
            disabled: false,
        }
    }
    /// Custom element drawn in place of the plain label text (such as highlighted match spans).
    /// The plain `label` passed to [`Self::new`] remains the option's accessible name, and the
    /// custom element is hidden from the accessibility tree so it is not read twice.
    ///
    /// ```
    /// use rune::{row, text};
    /// use rune_kit::PopupItem;
    ///
    /// let _item = PopupItem::new("review", "/review")
    ///     .label_with(row().child(text("/rev")).child(text("iew")));
    /// ```
    pub fn label_with(mut self, el: El) -> Self {
        self.label_el = Some(el);
        self
    }
    /// Secondary muted text shown after the label on the same row.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.description = (!text.is_empty()).then_some(text);
        self
    }
    /// Leading element before the label (an icon, avatar, or swatch).
    pub fn leading(mut self, el: El) -> Self {
        self.leading = Some(el);
        self
    }
    /// Trailing element at the right edge of the row (a keybind hint, badge, or check icon).
    pub fn trailing(mut self, el: El) -> Self {
        self.trailing = Some(el);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl From<&PopupItem> for PopupItem {
    fn from(item: &PopupItem) -> Self {
        item.clone()
    }
}

impl From<&ComboboxOption> for PopupItem {
    fn from(o: &ComboboxOption) -> Self {
        Self::new(&o.id, &o.label).description(&o.detail).disabled(o.disabled)
    }
}

impl From<ComboboxOption> for PopupItem {
    fn from(o: ComboboxOption) -> Self {
        Self::new(o.id, o.label).description(o.detail).disabled(o.disabled)
    }
}

type ActiveFn<S> = Rc<dyn Fn(&mut S, usize)>;
type ChooseFn<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;
type CloseFn<S> = Rc<dyn Fn(&mut S)>;

pub(super) trait PopupBind {
    fn bind_disabled_item(&self, item: El) -> El;
    fn bind_item(&self, item: El, index: usize, id: String) -> El;
    fn scrim(&self, key: String) -> Option<El>;
    fn bind_content(&self, content: El) -> El;
}

pub(super) struct PopupHandlers<S> {
    pub(super) on_active: Option<ActiveFn<S>>,
    pub(super) on_choose: Option<ChooseFn<S>>,
    pub(super) on_close: Option<CloseFn<S>>,
}

impl<S: 'static> PopupBind for PopupHandlers<S> {
    fn bind_disabled_item(&self, item: El) -> El {
        item.on_press(|_: &mut S| {})
    }

    fn bind_item(&self, mut item: El, index: usize, id: String) -> El {
        if let Some(on_active) = &self.on_active {
            let on_active = on_active.clone();
            item = item.on_hover(move |s: &mut S| on_active(s, index));
        }
        if let Some(on_choose) = &self.on_choose {
            let on_choose = on_choose.clone();
            item.on_press_tx(move |s: &mut S, tx| on_choose(s, &id, tx))
        } else {
            item.on_press(|_: &mut S| {})
        }
    }

    fn scrim(&self, key: String) -> Option<El> {
        self.on_close.as_ref().map(|close| {
            let close = close.clone();
            scrim(key, move |s: &mut S| close(s))
        })
    }

    fn bind_content(&self, content: El) -> El {
        content.on_press(|_: &mut S| {})
    }
}

pub(super) struct PopupListCore {
    pub(super) key: String,
    pub(super) anchor: String,
    pub(super) open: bool,
    pub(super) side: Side,
    pub(super) align: Align,
    pub(super) gutter: f32,
    pub(super) width: f32,
    pub(super) max_height: f32,
    pub(super) padding: f32,
    pub(super) row_radius: f32,
    pub(super) items: Vec<PopupItem>,
    pub(super) animate_filtering: bool,
    pub(super) active: Option<usize>,
    pub(super) empty: String,
    pub(super) label: Option<String>,
}

impl PopupListCore {
    pub(super) fn new(key: String, anchor: String) -> Self {
        Self {
            key,
            anchor,
            open: true,
            side: Side::Top,
            align: Align::Start,
            gutter: 4.0,
            width: 260.0,
            max_height: 240.0,
            padding: 4.0,
            row_radius: 4.0,
            items: Vec::new(),
            animate_filtering: false,
            active: None,
            empty: "No results".into(),
            label: None,
        }
    }

    pub(super) fn build_list(self, k: &Kit, bind: &dyn PopupBind) -> El {
        let active = self.active.filter(|&i| self.items.get(i).is_some_and(|item| !item.disabled));
        let mut rows = col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(28.0).reveal_selected(true);
        for (i, option) in self.items.iter().enumerate() {
            let id = option.id.clone();
            let is_active = active == Some(i);
            let mut item = row()
                .key(format!("{}-option-{}", self.key, id))
                .role("option")
                .label(&option.label)
                .description(option.description.clone().unwrap_or_default())
                .position_in_set(i + 1, self.items.len())
                .focus_selected(is_active)
                .w_full()
                .h(28.0)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .radius(self.row_radius)
                .transition(Transition::opacity())
                .fill(if is_active { k.c.hover } else { Color::TRANSPARENT });
            if let Some(leading) = &option.leading {
                item = item.child(leading.clone());
            }
            if let Some(desc) = option.description.as_deref().filter(|d| !d.is_empty()) {
                let label = match &option.label_el {
                    Some(el) => el.clone().shrink().a11y_hidden(),
                    None => text(&option.label).font(k.ty.ui_compact).color(k.c.text_base).shrink().truncate().a11y_hidden(),
                };
                item = item
                    .child(label)
                    .child(text(desc).key("description").font(k.ty.ui_compact).color(k.c.text_weak).grow(1.0).shrink().truncate().a11y_hidden());
            } else {
                let label = match &option.label_el {
                    Some(el) => el.clone().grow(1.0).shrink().a11y_hidden(),
                    None => text(&option.label).font(k.ty.ui_compact).color(k.c.text_base).grow(1.0).shrink().truncate().a11y_hidden(),
                };
                item = item.child(label);
            }
            if let Some(trailing) = &option.trailing {
                item = item.child(trailing.clone());
            }
            item = if option.disabled {
                bind.bind_disabled_item(k.disabled_dim(item, true).disabled(true))
            } else {
                bind.bind_item(item, i, id)
            };
            rows = rows.child(item);
        }
        // The viewport and the empty state both stay mounted, so the popover's height springs
        // between them as the search edits it instead of swapping one box for the other.
        let empty = self.items.is_empty();
        let mut viewport = fit_scroll(rows)
            .key("viewport")
            .role("listbox")
            .h((self.items.len() as f32 * 28.0).min(self.max_height))
            // A window too short for the list still shows one row to scroll.
            .min_h(if empty { 0.0 } else { 28.0 })
            .clip_soft(0.0);
        if let Some(label) = self.label.as_deref() {
            viewport = viewport.label(label);
        }
        col()
            .key("list")
            .preserve_focus_on_press(true)
            .w_full()
            .shrink()
            .min_h(0.0)
            .child(crate::scroll_bar(k, &scroll_area_key(&self.key), "viewport", viewport).w_full().shrink().min_h(0.0))
            .child(
                el().key("empty")
                    .h(if empty { 28.0 } else { 0.0 })
                    .px(8.0)
                    .center_y()
                    .clip_soft(0.0)
                    .opacity(if empty { 1.0 } else { 0.0 })
                    .child(text(self.empty).font(k.ty.ui_compact).color(k.c.text_weak)),
            )
    }

    fn build(self, k: &Kit, layer: &mut Vec<El>, bind: &dyn PopupBind) {
        if !self.open {
            return;
        }
        let dismiss = bind.scrim(scrim_key(&self.key));
        let key = content_key(&self.key);
        let anchor = self.anchor.clone();
        let side = self.side;
        let align = self.align;
        let gutter = self.gutter;
        let width = self.width;
        let padding = self.padding;
        let instant = !self.animate_filtering;
        let list = self.build_list(k, bind);
        let mut content = bind.bind_content(
            col()
                .key(key)
                .preserve_focus_on_press(true)
                .w(width)
                .padding(padding),
        )
        .child(list);
        if instant {
            content = content.instant_changes();
        }
        push_raised_anchored(k, layer, dismiss, content, &anchor, side, align, gutter);
    }
}

/// An anchored popup listbox driven by an outside field:
/// `PopupList::new("mentions", "composer").open(s.open).items(items).active(s.active).build(&k, &mut layer)`.
///
/// The popup takes no keyboard focus, so focus and typing stay in the app's field while it is
/// open. Moving `active` in app state scrolls the active option into view.
pub struct PopupList<S> {
    core: PopupListCore,
    handlers: PopupHandlers<S>,
}

impl<S: 'static> PopupList<S> {
    pub fn new(key: impl Into<String>, anchor: impl Into<String>) -> Self {
        Self {
            core: PopupListCore::new(key.into(), anchor.into()),
            handlers: PopupHandlers { on_active: None, on_choose: None, on_close: None },
        }
    }
    pub fn anchor(mut self, anchor: impl Into<String>) -> Self { self.core.anchor = anchor.into(); self }
    /// Anchor to `anchor` with explicit side, alignment and offset, as [`El::anchored`].
    pub fn anchored(mut self, anchor: impl Into<String>, side: Side, align: Align, gap: f32) -> Self {
        self.core.anchor = anchor.into();
        self.core.side = side;
        self.core.align = align;
        self.core.gutter = gap;
        self
    }
    pub fn placement(mut self, placement: Placement) -> Self {
        (self.core.side, self.core.align) = placement.split();
        self
    }
    pub fn side(mut self, side: Side) -> Self { self.core.side = side; self }
    pub fn align(mut self, align: Align) -> Self { self.core.align = align; self }
    pub fn offset(mut self, gap: f32) -> Self { self.core.gutter = gap; self }
    pub fn gutter(mut self, gap: f32) -> Self { self.core.gutter = gap; self }
    pub fn open(mut self, open: bool) -> Self { self.core.open = open; self }
    pub fn width(mut self, width: f32) -> Self { self.core.width = width; self }
    pub fn max_height(mut self, max_height: f32) -> Self { self.core.max_height = max_height; self }
    /// Space between the popover's edge and its rows, in points (default 4).
    pub fn padding(mut self, padding: f32) -> Self { self.core.padding = padding; self }
    /// Corner radius of each row's highlight, in points (default 4).
    pub fn row_radius(mut self, radius: f32) -> Self { self.core.row_radius = radius; self }
    /// The rows. A change lands at once: the popover takes its new height and rows appear and
    /// go in the same frame, even in a commit that animates the field the list follows (one
    /// with `animate_edit_layout`). [`Self::animate_filtering`] animates it instead.
    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<PopupItem>>) -> Self {
        self.core.items = items.into_iter().map(Into::into).collect();
        self
    }
    /// Animate a change to the rows: the popover springs to its new height, and the rows it
    /// drops fade out where they stood (a 0.24 s spring) as the new ones fade in (0.3 s).
    /// Off by default, so a list that follows each keystroke swaps at once. Opening, closing
    /// and the hover highlight animate either way.
    pub fn animate_filtering(mut self, animate: bool) -> Self { self.core.animate_filtering = animate; self }
    pub fn item(mut self, item: impl Into<PopupItem>) -> Self { self.core.items.push(item.into()); self }
    pub fn active(mut self, active: Option<usize>) -> Self { self.core.active = active; self }
    pub fn empty_message(mut self, text: impl Into<String>) -> Self { self.core.empty = text.into(); self }
    pub fn label(mut self, label: impl Into<String>) -> Self { self.core.label = Some(label.into()); self }
    pub fn on_active(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self { self.handlers.on_active = Some(Rc::new(f)); self }
    pub fn on_choose(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.on_choose = Some(Rc::new(f)); self }
    pub fn on_close(mut self, f: impl Fn(&mut S) + 'static) -> Self { self.handlers.on_close = Some(Rc::new(f)); self }

    /// Builds the scrollable listbox element without anchoring it into an overlay layer, so an
    /// app can embed the list inside its own popover or panel (pass `""` for `anchor` when
    /// unused). Respects [`Self::animate_filtering`] and does not check [`Self::open`].
    ///
    /// ```
    /// use rune_kit::{Kit, PopupItem, PopupList};
    ///
    /// struct App;
    /// let k = Kit::neutral(true);
    /// let _el = PopupList::<App>::new("completions", "")
    ///     .label("Completions")
    ///     .items([PopupItem::new("open", "Open")])
    ///     .build_list(&k);
    /// ```
    pub fn build_list(self, k: &Kit) -> El {
        let instant = !self.core.animate_filtering;
        let list = self.core.build_list(k, &self.handlers);
        if instant { list.instant_changes() } else { list }
    }

    pub fn build(self, k: &Kit, layer: &mut Vec<El>) {
        self.core.build(k, layer, &self.handlers);
    }
}
