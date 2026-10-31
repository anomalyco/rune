use std::borrow::Cow;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use rune::{Color, El, GrowthSpec, Icon, KeyEvent, ScrollSpec, Span, Tx, col, el, icon, rich, row, text};

use crate::Kit;

use super::{FileNode, TreeKey, TreeModel, TreeRow};

/// What a standard tree row shows after its indent and chevron: an icon, a label and trailing
/// content (change counts, a status dot, a badge, or any element).
pub struct TreeItem {
    label: String,
    icon: Option<&'static Icon>,
    icon_color: Option<Color>,
    label_color: Option<Color>,
    highlight: Option<String>,
    changes: Option<(u32, u32)>,
    dot: Option<Color>,
    badge: Option<String>,
    trailing: Vec<El>,
}

impl TreeItem {
    pub fn new(label: impl Into<String>) -> TreeItem {
        TreeItem {
            label: label.into(),
            icon: None,
            icon_color: None,
            label_color: None,
            highlight: None,
            changes: None,
            dot: None,
            badge: None,
            trailing: Vec::new(),
        }
    }
    /// A file tree's row: the node's name and its folder or file icon ([`super::file_icon`]).
    pub fn file<D>(node: &FileNode<D>, row: &TreeRow) -> TreeItem {
        TreeItem::new(node.name.clone()).icon(node.icon(row.expanded == Some(true)))
    }
    pub fn icon(mut self, i: &'static Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn icon_color(mut self, c: Color) -> Self {
        self.icon_color = Some(c);
        self
    }
    /// The label's colour (the theme's text colour by default): dimmed for an ignored file, say.
    pub fn label_color(mut self, c: Color) -> Self {
        self.label_color = Some(c);
        self
    }
    /// Marks the first case-insensitive occurrence of `query` in the label, as a search does.
    pub fn highlight(mut self, query: impl Into<String>) -> Self {
        let q = query.into();
        self.highlight = (!q.is_empty()).then_some(q);
        self
    }
    /// Lines added and removed, as `+12 −3` (a zero side is left out; both zero shows nothing).
    pub fn changes(mut self, added: u32, removed: u32) -> Self {
        self.changes = (added > 0 || removed > 0).then_some((added, removed));
        self
    }
    /// A small status dot at the end of the row.
    pub fn dot(mut self, c: Color) -> Self {
        self.dot = Some(c);
        self
    }
    /// A short status letter or count at the end of the row (`M`, `3`).
    pub fn badge(mut self, b: impl Into<String>) -> Self {
        self.badge = Some(b.into());
        self
    }
    /// Any element at the end of the row.
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing.push(e);
        self
    }

    /// The row as [`Tree`] draws it, without handlers: indent guides, chevron, icon, label and
    /// trailing content, `style.row_height` tall. A sidebar can draw its rows with this.
    pub fn build(self, k: &Kit, r: &TreeRow, style: &TreeStyle) -> El {
        let c = &k.c;
        let pad = style.padding;
        let chevron_w = 16.0;
        let mut e = row()
            .role("treeitem")
            .label(self.label.clone())
            .level(r.depth + 1)
            .focus_selected(r.selected)
            .w_full()
            .h(style.row_height)
            .pl(pad + r.depth as f32 * style.indent)
            .pr(pad)
            .gap(4.0)
            .center_y()
            .radius(k.radius.sm)
            .motion(k.motion.control);
        if let Some(open) = r.expanded {
            e = e.expanded(open);
        }
        e = if r.selected { e.fill(style.selected) } else { e.hover(c.hover) };
        if r.cursor && r.multi {
            e = e.outline(1.0, -1.0, c.border_focus);
        }
        if style.guides {
            for level in 0..r.depth {
                let x = pad + level as f32 * style.indent + chevron_w * 0.5 - 0.5;
                e = e.child(el().key(format!("guide{level}")).pin(Some(x), Some(0.0), None, Some(0.0)).w(1.0).fill(style.guide));
            }
        }
        let chevron = el().key("chevron").size(chevron_w, chevron_w).center();
        e = e.child(match r.expanded {
            Some(open) => chevron.child(
                icon(k.glyph.chevron_down, 16.0)
                    .key("glyph")
                    .color(c.icon_muted)
                    .icon_rotation(if open { 0.0 } else { -FRAC_PI_2 })
                    .motion(k.motion.control),
            ),
            None => chevron,
        });
        if let Some(i) = self.icon {
            e = e.child(icon(i, 16.0).key("icon").color(self.icon_color.unwrap_or(c.icon_muted)));
        }
        let color = self.label_color.unwrap_or(c.text_base);
        let font = k.ty.ui;
        let spans = self
            .highlight
            .as_deref()
            .and_then(|q| find_ci(&self.label, q))
            .map(|(a, b)| vec![Span { color: Some(c.text_base), weight: Some(650.0), bg: Some(k.alpha(c.text_accent, 0.22)), bg_pad: 1.0, ..Span::new(a, b) }])
            .unwrap_or_default();
        let full = self.label.clone();
        let label = if spans.is_empty() { text(self.label) } else { rich(self.label, spans) };
        e = e.child(label.key("label").font(font).color(color).grow(1.0).shrink().min_w(0.0).truncate().help_when_truncated(full));
        if let Some((add, del)) = self.changes {
            let mut counts = row().key("changes").gap(4.0).center_y();
            let f = k.ty.text_xsmall.features("'tnum' 1");
            if add > 0 {
                counts = counts.child(text(format!("+{add}")).key("added").font(f).color(c.success.icon));
            }
            if del > 0 {
                counts = counts.child(text(format!("\u{2212}{del}")).key("removed").font(f).color(c.danger.icon));
            }
            e = e.child(counts);
        }
        if let Some(b) = self.badge {
            e = e.child(text(b).key("badge").font(k.ty.text_xsmall_medium).color(c.text_muted));
        }
        if let Some(d) = self.dot {
            e = e.child(el().key("dot").size(6.0, 6.0).radius(3.0).fill(d).ml(5.0).mr(5.0));
        }
        e.children(self.trailing)
    }
}

/// The byte range of `q` in `s`, ignoring ASCII case.
fn find_ci(s: &str, q: &str) -> Option<(usize, usize)> {
    let (sl, ql) = (s.to_lowercase(), q.to_lowercase());
    // Lowercasing can change byte lengths outside ASCII; only trust a same-length mapping.
    if sl.len() != s.len() {
        return s.find(q).map(|a| (a, a + q.len()));
    }
    sl.find(&ql).map(|a| (a, a + ql.len()))
}

/// A tree's metrics and colours. [`TreeStyle::from_kit`] takes them from the theme.
#[derive(Clone, Copy, Debug)]
pub struct TreeStyle {
    pub row_height: f32,
    /// Points each level steps in.
    pub indent: f32,
    /// Space at the row's ends.
    pub padding: f32,
    /// Hairlines down each level's indent.
    pub guides: bool,
    pub guide: Color,
    pub selected: Color,
}

impl TreeStyle {
    pub fn from_kit(k: &Kit) -> TreeStyle {
        TreeStyle {
            row_height: 24.0,
            indent: 14.0,
            padding: 6.0,
            guides: true,
            guide: k.c.border_muted,
            selected: k.alpha(k.c.text_accent, if k.dark { 0.24 } else { 0.2 }),
        }
    }
}

type RowFn<T> = Rc<dyn for<'a> Fn(&T, &TreeRow<'a>) -> TreeItem>;
type IdFn<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;

/// A virtualised tree over a [`TreeModel`] in the app's state `S`.
pub struct Tree<S, T> {
    get: fn(&S) -> &TreeModel<T>,
    get_mut: fn(&mut S) -> &mut TreeModel<T>,
    label: fn(&T) -> &str,
    row: Option<RowFn<T>>,
    on_select: Option<IdFn<S>>,
    on_open: Option<IdFn<S>>,
    style: Option<TreeStyle>,
    name: Option<String>,
}

impl<S: 'static, T: 'static> Tree<S, T> {
    /// `get` and `get_mut` find the model in the state; `label` names a node (rows show it, and
    /// typing jumps by it).
    pub fn new(get: fn(&S) -> &TreeModel<T>, get_mut: fn(&mut S) -> &mut TreeModel<T>, label: fn(&T) -> &str) -> Self {
        Tree { get, get_mut, label, row: None, on_select: None, on_open: None, style: None, name: None }
    }
    /// What each row shows; by default its label alone.
    pub fn row(mut self, f: impl for<'a> Fn(&T, &TreeRow<'a>) -> TreeItem + 'static) -> Self {
        self.row = Some(Rc::new(f));
        self
    }
    /// The cursor moved to `id`, by click or key: a diff view can follow it.
    pub fn on_select(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
    /// A row without children was double-clicked, or Return or Space pressed on it.
    pub fn on_open(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_open = Some(Rc::new(f));
        self
    }
    pub fn style(mut self, s: TreeStyle) -> Self {
        self.style = Some(s);
        self
    }
    /// The tree's accessibility label ("Files").
    pub fn name(mut self, n: impl Into<String>) -> Self {
        self.name = Some(n.into());
        self
    }

    /// The tree, filling the height its parent gives it (set one with `.h(..)` or `.grow(..)`).
    pub fn build(self, s: &S, k: &Kit) -> El {
        let Tree { get, get_mut, label, row, on_select, on_open, style, name } = self;
        let style = style.unwrap_or_else(|| TreeStyle::from_kit(k));
        let model = get(s);
        let count = model.visible_len();
        let tree_key = model.key.clone();
        let kit = Rc::new(k.clone());
        let row_fn = row;
        let (sel, open) = (on_select.clone(), on_open.clone());
        let build_row = move |s: &S, i: usize| -> El {
            let m = get(s);
            let Some((data, r)) = m.row(i) else { return el() };
            let item = match &row_fn {
                Some(f) => f(data, &r),
                None => TreeItem::new(label(data)),
            };
            let id = r.id.to_owned();
            let openable = r.expanded.is_some();
            let mut e = item.build(&kit, &r, &style).key(r.id);
            // A click selects the row (Shift extends and Cmd toggles, in a multi-select tree);
            // on a folder it also opens or closes it, as a file tree's does.
            let pick = id.clone();
            let sel = sel.clone();
            e = e.on_press_tx(move |s: &mut S, tx: &mut Tx| {
                let m = get_mut(s);
                let plain = m.click(&pick, tx.mods());
                if openable && plain {
                    m.toggle(&pick);
                }
                if let Some(f) = &sel {
                    f(s, &pick, tx);
                }
            });
            if !openable && let Some(f) = open.clone() {
                e = e.on_double_press_tx(move |s: &mut S, tx: &mut Tx| f(s, &id, tx));
            }
            e
        };
        let rows = col()
            .key("rows")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(
                count,
                style.row_height,
                move |s: &S, i| Cow::Borrowed(get(s).visible_id(i).unwrap_or_default()),
                build_row,
            );
        let viewport = col()
            .key("viewport")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            // Inside a scrolling page this reports every frame of the scroll; only a new height
            // changes what the view reads.
            .on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
                let m = get_mut(s);
                if m.viewport == r[3] && m.row_height == style.row_height { tx.live_only(); }
                m.viewport = r[3];
                m.row_height = style.row_height;
            })
            .child(rows);
        let mut root = col()
            .key(tree_key)
            .role("tree")
            .focusable(true)
            .w_full();
        if let Some(n) = name {
            root = root.label(n);
        }
        root
            .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                let m = get_mut(s);
                let before = m.cursor;
                let out = m.handle_key(ev, label, tx);
                let after = m.cursor;
                let id = after.map(|c| m.nodes[c].id.clone());
                match out {
                    TreeKey::Ignored => false,
                    TreeKey::Handled => {
                        if before != after && let (Some(f), Some(id)) = (&on_select, id) {
                            f(s, &id, tx);
                        }
                        true
                    }
                    TreeKey::Open(id) => {
                        if let Some(f) = &on_open {
                            f(s, &id, tx);
                        }
                        true
                    }
                }
            })
            .child(viewport)
    }
}
