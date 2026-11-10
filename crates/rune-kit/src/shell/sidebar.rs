//! A source-list sidebar: collapsible sections of selectable rows, with header and footer
//! slots, keyboard navigation and an optional virtualised body.

use std::borrow::Cow;

use rune::{Anim, Color, Drag, El, GrowthSpec, Icon, Key, Phase, ScrollSpec, TextEdit, TextStyle, Tx, col, el, icon, row, text, text_input};

use super::reorder::Reorder;
use crate::{ControlSize, IconButton, IconButtonVariant, Kit, Spinner};

type Apply = Box<dyn FnOnce(El) -> El>;
type Themed = Box<dyn FnOnce(&Kit) -> El>;

/// A sidebar row's height (AppKit's medium source-list row).
pub const ROW_HEIGHT: f32 = 28.0;
/// A two-line sidebar row's default height when [`SidebarRow::subtitle`] is set without
/// [`SidebarRow::height`].
pub const TWO_LINE_ROW_HEIGHT: f32 = 40.0;
/// Each level of [`SidebarRow::depth`] indents this far.
pub const INDENT: f32 = 16.0;
/// Rows sit this far in from the sidebar's sides.
const INSET: f32 = 8.0;

enum Body {
    Items(Vec<Themed>),
    Virtual(Box<dyn FnOnce(El, f32) -> El>),
}

/// A sidebar's content: header, a scrolling list of sections and rows, and footer.
/// `Sidebar::new().header(..).section(..).footer(..).build(&kit)`. The list is one Tab stop
/// that lands on the selected row. Up and Down move the selection (it follows focus, as in a
/// source list), Home and End go to the ends, typing a row's first letters jumps to it, and
/// Return presses the focused row.
pub struct Sidebar {
    label: String,
    header: Vec<El>,
    body: Body,
    footer: Vec<El>,
    inset: f32,
    header_gap: f32,
    header_pad: [f32; 2],
    row_gap: f32,
    row_height: f32,
}

impl Sidebar {
    pub fn new() -> Sidebar {
        Sidebar {
            label: "Sidebar".into(),
            header: Vec::new(),
            body: Body::Items(Vec::new()),
            footer: Vec::new(),
            inset: INSET,
            header_gap: 2.0,
            header_pad: [0.0, INSET],
            row_gap: 0.0,
            row_height: ROW_HEIGHT,
        }
    }
    /// Points between the sidebar's sides and its rows, header and footer (default 8).
    pub fn inset(mut self, points: f32) -> Self {
        self.inset = points.max(0.0);
        self
    }
    /// Points between the header's items (default 2).
    pub fn header_gap(mut self, points: f32) -> Self {
        self.header_gap = points.max(0.0);
        self
    }
    /// Points above the header and between it and the list (default 0 and 8).
    pub fn header_padding(mut self, top: f32, bottom: f32) -> Self {
        self.header_pad = [top.max(0.0), bottom.max(0.0)];
        self
    }
    /// Points between the list's items (default 0). Virtualised rows don't take it: give them
    /// the space in their height.
    pub fn row_gap(mut self, points: f32) -> Self {
        self.row_gap = points.max(0.0);
        self
    }
    /// The height of [`Self::virtual_rows`]' rows (default [`ROW_HEIGHT`]). Give the rows the
    /// same height with [`SidebarRow::height`].
    pub fn row_height(mut self, points: f32) -> Self {
        self.row_height = points.max(1.0);
        self
    }
    /// The list's accessible name.
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = l.into();
        self
    }
    /// Above the list, outside its scrolling: a "New" row or button, a search field.
    pub fn header(mut self, e: El) -> Self {
        self.header.push(e);
        self
    }
    /// Below the list: an account or settings row, a version line.
    pub fn footer(mut self, e: El) -> Self {
        self.footer.push(e);
        self
    }
    pub fn section(self, s: SidebarSection) -> Self {
        self.item(Box::new(move |k: &Kit| s.build(k)))
    }
    /// Any element in the list: a built [`SidebarRow`] or [`SidebarHeader`], a divider.
    pub fn child(self, e: El) -> Self {
        self.item(Box::new(move |_: &Kit| e))
    }
    /// A virtualised list of `count` rows for long histories: only the rows near the viewport
    /// are built. `key(s, i)` names row `i` and `build(s, i)` builds it, typically a
    /// [`SidebarRow`], or a [`SidebarHeader`] in a flattened list of sections. `El::virtual_rows`
    /// has the rules. Replaces any sections or children.
    pub fn virtual_rows<S: 'static>(
        mut self,
        count: usize,
        key: impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static,
        build: impl Fn(&S, usize) -> El + 'static,
    ) -> Self {
        self.body = Body::Virtual(Box::new(move |e: El, height: f32| e.virtual_rows(count, height, key, build)));
        self
    }

    fn item(mut self, e: Themed) -> Self {
        match &mut self.body {
            Body::Items(v) => v.push(e),
            Body::Virtual(_) => self.body = Body::Items(vec![e]),
        }
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let inset = self.inset;
        let list = col().key("sidebar-list").role("list").label(self.label).focus_group().focus_follows().w_full().px(inset);
        let list = match self.body {
            Body::Items(items) => list.pb(INSET).gap(self.row_gap).children(items.into_iter().map(|f| f(k))),
            Body::Virtual(rows) => rows(list.growth(GrowthSpec::default()), self.row_height),
        };
        let mut root = col().key("sidebar").w_full().h_full().min_h(0.0);
        if !self.header.is_empty() {
            let [top, bottom] = self.header_pad;
            root = root.child(col().key("sidebar-header").w_full().px(inset).pt(top).pb(bottom).gap(self.header_gap).children(self.header));
        }
        root = root.child(
            col()
                .key("sidebar-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .scroll_edge_fade(16.0)
                .child(list),
        );
        if !self.footer.is_empty() {
            root = root.child(col().key("sidebar-footer").w_full().px(inset).py(INSET).gap(self.header_gap).children(self.footer));
        }
        root
    }
}

impl Default for Sidebar {
    fn default() -> Self {
        Self::new()
    }
}

/// A section's header row: its title, an optional count, a disclosure triangle and a trailing
/// action (a small "+" button, say). Pressing it calls `on_toggle`; whether the section is
/// collapsed is the app's state. [`SidebarSection`] pairs it with its rows; use it on its own
/// in a virtualised list.
pub struct SidebarHeader {
    title: String,
    count: Option<String>,
    collapsed: Option<bool>,
    action: Option<El>,
    toggle: Option<Apply>,
}

impl SidebarHeader {
    pub fn new(title: impl Into<String>) -> SidebarHeader {
        SidebarHeader { title: title.into(), count: None, collapsed: None, action: None, toggle: None }
    }
    /// Shows a disclosure triangle for this state. Without it, or `on_toggle`, the header is
    /// a plain label.
    pub fn collapsed(mut self, c: bool) -> Self {
        self.collapsed = Some(c);
        self
    }
    pub fn count(mut self, n: usize) -> Self {
        self.count = Some(n.to_string());
        self
    }
    /// A trailing element, such as a small "+" icon button.
    pub fn action(mut self, e: El) -> Self {
        self.action = Some(e);
        self
    }
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.toggle = Some(Box::new(move |e: El| e.on_press_tx(f)));
        self.collapsed.get_or_insert(false);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let mut title = row()
            .key("section-title")
            .gap(4.0)
            .center_y()
            .shrink()
            .child(text(self.title.clone()).font(k.ty.text_xsmall_medium).color(c.text_muted).truncate().shrink());
        let counted = self.count.as_ref().map(|n| format!("{n} items")).unwrap_or_default();
        if let Some(n) = self.count {
            title = title.child(text(n).key("count").font(k.ty.text_xsmall_medium).color(c.text_faint));
        }
        if let Some(collapsed) = self.collapsed {
            let glyph = if collapsed { k.glyph.triangle_right } else { k.glyph.triangle_down };
            title = title.child(icon(glyph, 12.0).key("disclosure").color(c.icon_faint));
        }
        // Its title names it, the count describes it, and a reader hears it expanded or collapsed.
        let mut header = row()
            .key("section-header")
            .label(self.title)
            .description(counted)
            .w_full()
            .h(ROW_HEIGHT)
            .pt(4.0)
            .px(INSET)
            .center_y()
            .radius(k.radius.md)
            .child(title.a11y_hidden())
            .child(el().key("spacer").grow(1.0).a11y_hidden());
        if let Some(a) = self.action {
            header = header.child(a);
        }
        match self.toggle {
            // Pointer only: in the list's focus group the arrows step through rows, and with
            // selection following focus they would toggle the section as they passed.
            Some(toggle) => k.interactive(toggle(header)).role("button").expanded(!self.collapsed.unwrap_or(false))
                .hover_motion(Anim::INSTANT, Anim::INSTANT).focusable(false),
            None => header.role("heading"),
        }
    }
}

/// A titled group of rows that collapses: `SidebarSection::new("Today").collapsed(..)
/// .on_toggle(..).row(..)`. Rows below it move on the kit's disclosure spring as it opens.
pub struct SidebarSection {
    key: String,
    header: SidebarHeader,
    collapsed: bool,
    rows: Vec<El>,
}

impl SidebarSection {
    pub fn new(title: impl Into<String>) -> SidebarSection {
        let title = title.into();
        SidebarSection { key: title.clone(), header: SidebarHeader::new(title), collapsed: false, rows: Vec::new() }
    }
    /// Its key among the list's children (default: the title).
    pub fn key(mut self, k: impl Into<String>) -> Self {
        self.key = k.into();
        self
    }
    pub fn collapsed(mut self, c: bool) -> Self {
        self.collapsed = c;
        self.header = self.header.collapsed(c);
        self
    }
    pub fn count(mut self, n: usize) -> Self {
        self.header = self.header.count(n);
        self
    }
    pub fn action(mut self, e: El) -> Self {
        self.header = self.header.action(e);
        self
    }
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.header = self.header.on_toggle(f);
        self
    }
    /// A row: a built [`SidebarRow`], keyed by the app.
    pub fn row(mut self, e: El) -> Self {
        self.rows.push(e);
        self
    }
    pub fn rows(mut self, rows: impl IntoIterator<Item = El>) -> Self {
        self.rows.extend(rows);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let title = self.header.title.clone();
        let mut s = col().key(self.key).role("group").label(title).w_full().gap(2.0).motion(k.motion.disclosure).child(self.header.build(k));
        if !self.collapsed {
            s = s.children(self.rows);
        }
        s
    }
}

/// What a [`SidebarRow`]'s status slot shows.
pub enum RowStatus {
    /// Working: a small spinner.
    Busy,
    /// Waiting on the user: a dot in the warning colour.
    Attention,
    /// Something new: a 7 point dot in the accent colour.
    Unread,
    /// Any element, centred in the slot.
    Other(El),
}

/// A row that drags to a new place in its list: the list's drag, the row's index, and the
/// handlers that update the drag and make the move on a drop.
struct Reordering {
    state: Reorder,
    index: usize,
    count: usize,
    bind: Box<dyn FnOnce(El) -> El>,
}

/// A selectable sidebar row: icon, label, then a trailing detail (a time), badge or element.
/// `SidebarRow::new("Inbox").icon(&INBOX).badge("3").selected(sel).on_select(..).build(&kit)`.
/// Key the built row by its item's id.
///
/// A taller row takes a second, muted line, an avatar and a status slot that turns into a
/// close button under the pointer: `SidebarRow::new(title).height(45.0).avatar(..)
/// .subtitle(project).status(RowStatus::Busy).on_close(..)`.
pub struct SidebarRow {
    label: String,
    icon: Option<&'static Icon>,
    avatar: Option<El>,
    subtitle: Option<String>,
    detail: Option<String>,
    badge: Option<String>,
    trailing: Option<El>,
    status: Option<RowStatus>,
    close: Option<Apply>,
    selected: bool,
    depth: u8,
    height: Option<f32>,
    title_style: Option<TextStyle>,
    subtitle_style: Option<TextStyle>,
    subtitle_color: Option<Color>,
    select_only: bool,
    rename: Option<(TextEdit, Apply)>,
    reorder: Option<Reordering>,
    acts: Vec<Apply>,
}

impl SidebarRow {
    pub fn new(label: impl Into<String>) -> SidebarRow {
        SidebarRow {
            label: label.into(),
            icon: None,
            avatar: None,
            subtitle: None,
            detail: None,
            badge: None,
            trailing: None,
            status: None,
            close: None,
            selected: false,
            depth: 0,
            height: None,
            title_style: None,
            subtitle_style: None,
            subtitle_color: None,
            select_only: false,
            rename: None,
            reorder: None,
            acts: Vec::new(),
        }
    }
    pub fn icon(mut self, i: &'static Icon) -> Self {
        self.icon = Some(i);
        self
    }
    /// A 16 point leading element in place of the icon: a project's avatar, an initial.
    pub fn avatar(mut self, e: El) -> Self {
        self.avatar = Some(e);
        self
    }
    /// A second line under the label, smaller and fainter than it (11 point `caption` in
    /// `text_faint`, under a medium-weight 13 point label). The row's accessible name is then
    /// "Label, Subtitle". Give the row the height for it (default [`TWO_LINE_ROW_HEIGHT`]).
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }
    /// The label's type, in place of the default (13 point; medium weight with a subtitle).
    /// It also sets the inline rename field's type.
    pub fn title_style(mut self, style: TextStyle) -> Self {
        self.title_style = Some(style);
        self
    }
    /// The subtitle's type, in place of the default (the kit's `caption`).
    pub fn subtitle_style(mut self, style: TextStyle) -> Self {
        self.subtitle_style = Some(style);
        self
    }
    /// The subtitle's colour, in place of the default (`text_faint`).
    pub fn subtitle_color(mut self, color: Color) -> Self {
        self.subtitle_color = Some(color);
        self
    }
    /// A row that shows the selection but never takes focus: for a list driven from outside,
    /// such as one a search field filters and steers with its own keys. The row is out of the
    /// Tab order, Tab and Space don't act on it, and pressing it selects it (`on_select`)
    /// while a focused field stays focused (`El::preserve_focus_on_press`). Keyboard navigation
    /// of a select-only list (Up/Down and Return from the field) is the app's job. The selected
    /// row is still exposed as selected to assistive tech (`ElementInfo::item_selected`), and
    /// changing the selection never pulls focus to the row. Equivalent to
    /// `.focusable(false).focus_selected(..)` on the built row, plus the focus-preserving press.
    pub fn select_only(mut self) -> Self {
        self.select_only = true;
        self
    }
    /// The row's height (default [`ROW_HEIGHT`], or [`TWO_LINE_ROW_HEIGHT`] with a subtitle).
    pub fn height(mut self, points: f32) -> Self {
        self.height = Some(points.max(1.0));
        self
    }
    /// Faint trailing text: a time ("2m"), a count, a shortcut.
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    /// A trailing pill (an unread count).
    pub fn badge(mut self, b: impl Into<String>) -> Self {
        self.badge = Some(b.into());
        self
    }
    /// Any trailing element (a status dot, a menu button), after the detail and badge.
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing = Some(e);
        self
    }
    /// A 20 point slot at the row's end, after everything else.
    pub fn status(mut self, s: RowStatus) -> Self {
        self.status = Some(s);
        self
    }
    /// A close button in the status slot, in place of the status while the pointer is on the
    /// row. It swaps at once, with no fade.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.close = Some(Box::new(move |e: El| e.on_press_tx(f)));
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    /// Nesting level: each indents the row by [`INDENT`].
    pub fn depth(mut self, d: u8) -> Self {
        self.depth = d;
        self
    }
    /// Called when the row is pressed, or reached with the arrow keys.
    pub fn on_select<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.push(Box::new(move |e: El| e.on_press_tx(f)));
        self
    }
    /// Called on a double click (rename, open in a new window).
    pub fn on_double_click<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.push(Box::new(move |e: El| e.on_double_press_tx(f)));
        self
    }
    /// Drags the row to a new place among the list's `count` rows, of which it is row `index`.
    /// It lifts above its neighbours and follows the pointer, and they step aside on springs.
    /// A drop calls `drop(s, from, to)` to move the item; Escape, or the window losing focus,
    /// puts it back, and so does the pointer moving on when the row never hears its release
    /// (it unmounted mid-drag). `access` finds the [`Reorder`] in the app's state. A press that
    /// doesn't move is still a select.
    pub fn reorder<S: 'static>(
        mut self,
        state: &Reorder,
        index: usize,
        count: usize,
        access: fn(&mut S) -> &mut Reorder,
        drop: impl Fn(&mut S, usize, usize) + 'static,
    ) -> Self {
        let held = state.held();
        let bind = Box::new(move |e: El| {
            // While a row is held, every row hears where the pointer goes: a move its drag never
            // hears means the release was lost (the row unmounted, or a modal took the pointer).
            let e = if held {
                e.on_pointer_move_local(move |s: &mut S, p, tx: &mut Tx| {
                    if access(s).pointer([p.window_x, p.window_y]) {
                        tx.animated();
                    } else {
                        tx.live_only();
                    }
                })
            } else {
                e
            };
            e.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
                let r = access(s);
                match d.phase {
                    Phase::Down => r.press(index, d.window_point()),
                    Phase::Move => {
                        // The rows it passes spring aside; it follows the pointer exactly.
                        tx.animated();
                        r.drag(index, count, d.window_point(), d.dy, d.h);
                    }
                    Phase::Up => {
                        if let Some((from, to)) = r.end(d.cancelled()) {
                            drop(s, from, to);
                        }
                    }
                }
            })
        });
        self.reorder = Some(Reordering { state: *state, index, count, bind });
        self
    }
    /// Edits the label in place, in the field bound to `edit` (`access` finds it in the app's
    /// state). Set `edit.focused` and its text when renaming starts. Return calls `commit`,
    /// Escape calls `cancel`; both should end the rename.
    pub fn renaming<S: 'static>(
        mut self,
        edit: &TextEdit,
        access: fn(&mut S) -> &mut TextEdit,
        commit: impl Fn(&mut S, &mut Tx) + 'static,
        cancel: impl Fn(&mut S, &mut Tx) + 'static,
    ) -> Self {
        let bind: Apply = Box::new(move |e: El| {
            e.on_edit(access).on_submit(commit).on_input_key(move |s: &mut S, ev, tx| {
                if ev.key == Key::Escape {
                    cancel(s, tx);
                    return true;
                }
                false
            })
        });
        self.rename = Some((edit.clone(), bind));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let renaming = self.rename.is_some();
        let (ink, glyph) = if self.selected { (c.text_base, c.icon_base) } else { (c.text_muted, c.icon_muted) };
        let two_lines = self.subtitle.is_some();
        let height = self.height.unwrap_or(if two_lines { TWO_LINE_ROW_HEIGHT } else { ROW_HEIGHT });
        let font = self.title_style.unwrap_or(if two_lines {
            TextStyle { weight: 500.0, ..k.ty.ui_compact }
        } else {
            k.ty.ui
        });
        let sub_font = self.subtitle_style.unwrap_or_else(|| k.ty.caption());
        let name = match &self.subtitle {
            Some(sub) if !sub.is_empty() => format!("{}, {sub}", self.label),
            _ => self.label.clone(),
        };
        let focusable = !renaming && !self.select_only;
        // The row's name and description say what its texts, badge and status mark show.
        let status_words = match &self.status {
            Some(RowStatus::Busy) => Some("Working"),
            Some(RowStatus::Attention) => Some("Needs attention"),
            Some(RowStatus::Unread) => Some("Unread"),
            _ => None,
        };
        let said: Vec<&str> = [self.detail.as_deref(), self.badge.as_deref(), status_words].into_iter().flatten().collect();
        let mut r = row()
            .label(name)
            .description(said.join(", "))
            .w_full()
            .h(height)
            .pl(INSET + self.depth as f32 * INDENT)
            .pr(INSET)
            .gap(6.0)
            .center_y()
            .radius(k.radius.md)
            .motion(k.motion.control)
            .hover_group()
            .hover_motion(Anim::INSTANT, Anim::INSTANT)
            .focusable(focusable)
            .focus_selected(self.selected && !renaming);
        if self.select_only {
            r = r.preserve_focus_on_press(true);
        }
        r = k.interactive(r);
        if self.selected {
            r = r.fill(c.pressed);
        }
        let lead = match (self.avatar, self.icon) {
            (Some(a), _) => Some(row().key("avatar").size(16.0, 16.0).center().child(a).a11y_hidden()),
            (None, Some(i)) => Some(icon(i, 16.0).key("icon").color(glyph).a11y_hidden()),
            _ => None,
        };
        let has_lead = lead.is_some();
        let label = match self.rename {
            Some((edit, bind)) => bind(
                text_input(&edit, font)
                    .single_line()
                    .key("rename")
                    .focus_selected(true)
                    .focus_return("sidebar-list")
                    .grow(1.0)
                    .shrink()
                    .color(c.text_base)
                    .caret(c.text_base, k.selection()),
            ),
            None => text(self.label).key("label").font(font).color(ink).truncate().grow(1.0).shrink().a11y_hidden(),
        };
        r = match self.subtitle {
            // Two lines: the lead and the label, then the subtitle under the label.
            Some(sub) => {
                let first = row().key("line1").w_full().gap(6.0).center_y().children(lead).child(label);
                let second = text(sub).key("subtitle").font(sub_font).color(self.subtitle_color.unwrap_or(c.text_faint)).truncate().w_full().a11y_hidden();
                let second = row().key("line2").w_full().pl(if has_lead { 16.0 + 6.0 } else { 0.0 }).child(second);
                r.child(col().key("lines").grow(1.0).shrink().min_w(0.0).child(first).child(second))
            }
            None => r.children(lead).child(label),
        };
        if let Some(d) = self.detail {
            r = r.child(text(d).key("detail").font(k.ty.small).color(c.text_faint).a11y_hidden());
        }
        if let Some(b) = self.badge {
            r = r.child(
                row()
                    .a11y_hidden()
                    .key("badge")
                    .h(16.0)
                    .px(5.0)
                    .center()
                    .radius(8.0)
                    .fill(c.layer_03)
                    .child(text(b).font(k.ty.small_medium).color(c.text_muted)),
            );
        }
        if let Some(t) = self.trailing {
            r = r.child(t);
        }
        if self.status.is_some() || self.close.is_some() {
            let dot = |color| el().size(7.0, 7.0).radius(3.5).fill(color);
            let status = self.status.map(|s| match s {
                RowStatus::Busy => Spinner::new().size(12.0).color(c.icon_muted).build(k),
                RowStatus::Attention => dot(c.warning.icon),
                RowStatus::Unread => dot(c.bg_accent),
                RowStatus::Other(e) => e,
            });
            let mut slot = row().key("status").size(20.0, 20.0).center();
            if let Some(st) = status {
                let st = row().key("status-mark").size(20.0, 20.0).center().child(st).a11y_hidden();
                slot = slot.child(if self.close.is_some() { st.group_opacity(1.0, 0.0) } else { st });
            }
            if let Some(bind) = self.close {
                let close = IconButton::new(k.glyph.close).control_size(ControlSize::Small).variant(IconButtonVariant::GhostMuted).label("Close").build(k);
                // Pointer only, like the row's other hover controls: the list's arrows step rows.
                slot = slot.child(bind(close).key("close").absolute(0.0, 0.0).group_opacity(0.0, 1.0).focusable(false));
            }
            r = r.child(slot);
        }
        for a in self.acts {
            r = a(r);
        }
        if let Some(re) = self.reorder {
            let lifted = re.state.dragging() == Some(re.index);
            r = r.position_in_set(re.index + 1, re.count).offset(0.0, re.state.shift(re.index));
            if lifted {
                // Follows the pointer exactly; the rest of the list springs around it.
                // Opaque, on the window's deep background, so the rows it passes don't show through.
                r = k.elevate(r.raise().offset_motion(Anim::INSTANT).fill(crate::over(c.pressed, c.bg_deep)), &k.elevation.raised);
            }
            r = (re.bind)(r);
        }
        // After the handlers, which would make it a button.
        r.role("listitem")
    }
}
