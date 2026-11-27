//! Point picking (`Runtime::element_at`, `element_at_with`, `element_at_in`, `contains`) and
//! picking options (`Pick`, `Picked`).
use super::*;

/// An element [`Runtime::element_at`] found.
///
/// `PartialEq` and `Eq` compare element identity (`path` and `key`), ignoring the queried
/// geometry (`rect` and `local`) and attached data.
#[derive(Clone)]
pub struct Picked {
    path: String,
    key: Option<String>,
    rect: [f32; 4],
    local: [f32; 2],
    /// [`El::data`] values from the picked element up to the root, nearest first.
    data: Vec<crate::el::data::ElData>,
}

impl std::fmt::Debug for Picked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picked")
            .field("path", &self.path)
            .field("key", &self.key)
            .field("rect", &self.rect)
            .field("local", &self.local)
            .field("data", &self.data.len())
            .finish()
    }
}

impl PartialEq for Picked {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.key == other.key
    }
}

impl Eq for Picked {}

/// Options for [`Runtime::element_at_with`] and its siblings. Fields are private, so options can
/// be added without breaking callers; `Pick::default()` is [`Runtime::element_at`]'s behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    transparent: bool,
    interactive: bool,
    drop_targets: bool,
}

impl Pick {
    pub fn new() -> Self { Self::default() }

    /// Also find elements whose own or inherited opacity is 0, as a design canvas does to
    /// select an invisible layer. Exiting elements are skipped either way, and so are hidden
    /// or unmounted ones.
    ///
    /// Has no effect when [`Pick::interactive`] or [`Pick::drop_targets`] is set, because those
    /// query the frame's live hit-test lists and elements at opacity 0 take no input.
    pub fn transparent(mut self, on: bool) -> Self {
        self.transparent = on;
        self
    }

    /// Only find interactive hit targets — elements that receive pointer, press, drag, focus,
    /// or selectable-text input on the last frame — skipping passive layers, [`El::inert`]
    /// subtrees, elements behind an active modal, and fully transparent elements that take no
    /// input.
    ///
    /// Combined with [`Pick::drop_targets`], matches either an interactive hit target or a
    /// drop target, whichever painted topmost.
    ///
    /// ```
    /// use rune::{Env, Pick, Runtime, col, el};
    ///
    /// let mut rt = Runtime::new(
    ///     (),
    ///     |_: &(), _| col().size(200.0, 40.0)
    ///         .child(el().key("row").size(200.0, 40.0).on_press(|_: &mut ()| {}))
    ///         .child(el().key("layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))),
    ///     (200.0, 40.0),
    ///     Env::new(true, 1.0),
    /// );
    /// rt.frame(0.0);
    /// let hit = rt.element_at_with(50.0, 20.0, Pick::new().interactive(true)).unwrap();
    /// assert_eq!(hit.key(), Some("row"));
    /// ```
    pub fn interactive(mut self, on: bool) -> Self {
        self.interactive = on;
        self
    }

    /// Only find drop targets (elements with [`El::on_drop_files`] or drop callbacks/observers),
    /// skipping passive layers, [`El::inert`] subtrees, elements behind an active modal, and
    /// fully transparent elements that take no drops.
    ///
    /// Combined with [`Pick::interactive`], matches either an interactive hit target or a
    /// drop target, whichever painted topmost.
    ///
    /// ```
    /// use rune::{Env, Pick, Runtime, col, el};
    ///
    /// let mut rt = Runtime::new(
    ///     (),
    ///     |_: &(), _| col().size(200.0, 40.0)
    ///         .child(el().key("zone").size(200.0, 40.0).on_drop_files(|_: &mut (), _, _| {}))
    ///         .child(el().key("layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))),
    ///     (200.0, 40.0),
    ///     Env::new(true, 1.0),
    /// );
    /// rt.frame(0.0);
    /// let hit = rt.element_at_with(50.0, 20.0, Pick::new().drop_targets(true)).unwrap();
    /// assert_eq!(hit.key(), Some("zone"));
    /// ```
    pub fn drop_targets(mut self, on: bool) -> Self {
        self.drop_targets = on;
        self
    }
}

impl Picked {
    /// Its path in the element tree (as [`ElementInfo::path`]).
    pub fn path(&self) -> &str { &self.path }

    /// Its key ([`El::key`]), if it has one.
    pub fn key(&self) -> Option<&str> { self.key.as_deref() }

    /// Its drawn rect `[x, y, w, h]` in window points as of the last frame (as
    /// [`ElementInfo::rect`]).
    ///
    /// ```
    /// use rune::{Env, Runtime, col, el};
    ///
    /// let mut rt = Runtime::new(
    ///     (),
    ///     |_: &(), _| col().size(200.0, 100.0).padding(10.0)
    ///         .child(el().key("box").size(80.0, 30.0)),
    ///     (200.0, 100.0),
    ///     Env::new(true, 1.0),
    /// );
    /// rt.frame(0.0);
    /// let hit = rt.element_at(25.0, 20.0).unwrap();
    /// assert_eq!(hit.rect(), [10.0, 10.0, 80.0, 30.0]);
    /// ```
    pub fn rect(&self) -> [f32; 4] { self.rect }

    /// The queried point in this element's own layout points: from its top-left as drawn,
    /// divided by its drawn scale, following scrolling, [`El::scale`], [`El::rotate`], and
    /// live transforms — matching [`PointerEvent`] local coordinates.
    ///
    /// ```
    /// use rune::{Env, Runtime, col, el};
    ///
    /// let mut rt = Runtime::new(
    ///     (),
    ///     |_: &(), _| col().size(200.0, 100.0).padding(10.0)
    ///         .child(el().key("box").size(80.0, 30.0)),
    ///     (200.0, 100.0),
    ///     Env::new(true, 1.0),
    /// );
    /// rt.frame(0.0);
    /// let hit = rt.element_at(25.0, 20.0).unwrap();
    /// assert_eq!(hit.local(), [15.0, 10.0]);
    /// ```
    pub fn local(&self) -> [f32; 2] { self.local }

    /// The nearest value of type `T` attached with [`El::data`] to this element or one of its
    /// ancestors, as of the last frame. `None` when nothing on the hit path holds a `T`.
    ///
    /// ```
    /// use rune::{Env, Pick, Runtime, col, el};
    ///
    /// struct Zone(&'static str);
    ///
    /// let mut rt = Runtime::new(
    ///     (),
    ///     |_: &(), _| col().size(200.0, 40.0).data(Zone("window"))
    ///         .child(el().data(Zone("row")).size(200.0, 20.0).on_press(|_: &mut ()| {}))
    ///         .child(el().key("layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))),
    ///     (200.0, 40.0),
    ///     Env::new(true, 1.0),
    /// );
    /// rt.frame(0.0);
    /// let row = rt.element_at_with(10.0, 10.0, Pick::new().interactive(true)).unwrap();
    /// assert_eq!(row.data::<Zone>().map(|z| z.0), Some("row"));
    /// let layer = rt.element_at(10.0, 30.0).unwrap();
    /// assert_eq!(layer.data::<Zone>().map(|z| z.0), Some("window"));
    /// assert!(layer.data::<u32>().is_none());
    /// ```
    pub fn data<T: std::any::Any>(&self) -> Option<&T> {
        self.data.iter().find_map(|d| d.downcast_ref::<T>())
    }
}

impl RuntimeCore {
    /// The topmost element drawn at window point (`x`, `y`) in points, as of the last frame: the
    /// point is tested against each element's drawn box the way a press is, through `El::rotate`,
    /// `El::scale`, `El::origin`, offsets and scrolling, and inside its ancestors' clips
    /// (including a turned ancestor's clip), in paint order (so `El::raise`, sticky and flying
    /// elements count). Unlike a press, it also finds elements with no handlers, and it ignores
    /// `El::inert` and modals, so a press can land on a different element than this returns.
    /// Elements that are exiting or fully transparent are skipped (see
    /// [`Runtime::element_at_with`] to include transparent ones or filter to interactive/drop
    /// targets). `El::hit`, which marks native overlay input, makes no difference for the
    /// default filter, and overlay panels outside the window are not considered. An element is
    /// not cut by its own clip, only by its ancestors'. This is box geometry, not pixel/alpha
    /// picking: a box with no fill remains pickable (as an invisible input area does), and
    /// rounded corners and soft clip fades do not trim its hit box. Running handlers can use
    /// [`Tx::element_at`] without holding a `Runtime`.
    pub fn element_at(&self, x: f32, y: f32) -> Option<Picked> {
        self.element_at_with(x, y, Pick::default())
    }

    /// [`Runtime::element_at`] with options: [`Pick::transparent`] also finds fully transparent
    /// elements (exiting ones stay skipped); [`Pick::interactive`] matches only live pointer hit
    /// targets; [`Pick::drop_targets`] matches only live drop targets.
    pub fn element_at_with(&self, x: f32, y: f32, pick: Pick) -> Option<Picked> {
        self.element_at_within(None, x, y, pick)
    }

    /// [`Runtime::element_at`] restricted to an element and its descendants, ignoring other
    /// subtrees. `within` accepts a key or key path as [`Cx::layout`] does.
    pub fn element_at_in(&self, within: &str, x: f32, y: f32) -> Option<Picked> {
        self.element_at_in_with(within, x, y, Pick::default())
    }

    /// [`Runtime::element_at_in`] with options, as [`Runtime::element_at_with`].
    pub fn element_at_in_with(&self, within: &str, x: f32, y: f32, pick: Pick) -> Option<Picked> {
        self.element_at_within(Some(within), x, y, pick)
    }

    pub(super) fn element_at_within(&self, within: Option<&str>, x: f32, y: f32, pick: Pick) -> Option<Picked> {
        let scope = match within {
            Some(key) => Some(self.tree.selects_index(key)?),
            None => None,
        };
        let hit = if pick.interactive || pick.drop_targets {
            let resolve = |id: NodeId| {
                let i = *self.tree.node_index.get(&id)?;
                (self.paint.drawn.get(i).is_some()
                    && !self.subtree_exiting(i)
                    && self.allowed(i)
                    && scope.is_none_or(|scope| self.tree.is_descendant(i, scope)))
                    .then_some(i)
            };
            match (pick.interactive, pick.drop_targets) {
                (true, false) => self.hit.find_hit(|id, r| {
                    self.hit_contains(id, r, x, y).then(|| resolve(id)).flatten()
                }),
                (false, true) => self.hit.find_drop_hit(|id, r| {
                    self.hit_contains(id, r, x, y).then(|| resolve(id)).flatten()
                }),
                (true, true) => self.hit.top_drop_or_hit(|id, r| self.hit_contains(id, r, x, y), resolve),
                (false, false) => None,
            }
        } else {
            let n = self.tree.nodes.len();
            let order = self.paint.layer_paint_order();
            let at = |k: usize| if order.is_empty() { k } else { order[k] };
            let count = if order.is_empty() { n } else { order.len() };
            (0..count).rev().map(at).find(|&i| {
                scope.is_none_or(|scope| self.tree.is_descendant(i, scope))
                    && self.pickable(i, x, y, pick)
            })
        };
        hit.map(|i| self.picked(i, x, y))
    }

    /// Whether window point (`x`, `y`), in points, is inside the drawn, clipped and turned box
    /// of the element keyed `key`, whatever is drawn above it and whether or not it has handlers.
    /// False when there is no such element, it is exiting or fully transparent. Same geometry
    /// as [`Runtime::element_at`].
    pub fn contains(&self, key: &str, x: f32, y: f32) -> bool {
        self.contains_with(key, x, y, Pick::default())
    }

    /// [`Runtime::contains`] with options, as [`Runtime::element_at_with`].
    pub fn contains_with(&self, key: &str, x: f32, y: f32, pick: Pick) -> bool {
        self.tree.selects_index(key).is_some_and(|i| self.pickable(i, x, y, pick))
    }

    fn pickable(&self, i: usize, x: f32, y: f32, pick: Pick) -> bool {
        if pick.interactive || pick.drop_targets {
            if self.paint.drawn.get(i).is_none() || self.subtree_exiting(i) || !self.allowed(i) {
                return false;
            }
            let id = self.tree.nodes[i].id;
            let in_hits = pick.interactive
                && self.hit.hits().iter().any(|&(hid, r)| hid == id && self.hit_contains(hid, r, x, y));
            let in_drops = pick.drop_targets
                && self.hit.drop_hits().iter().any(|&(hid, r, _)| hid == id && self.hit_contains(hid, r, x, y));
            return in_hits || in_drops;
        }
        self.paint.drawn.get(i).is_some()
            && self.paint.inh_scratch.get(i).is_some_and(|h| pick.transparent || h.opacity > 0.0)
            && !self.subtree_exiting(i)
            && self.node_contains(i, x, y)
    }

    fn picked(&self, i: usize, x: f32, y: f32) -> Picked {
        let rect = self.paint.drawn.get(i).copied().unwrap_or_else(|| {
            let t = self.tree.nodes[i].target;
            [t[0], t[1], t[2] - t[0], t[3] - t[1]]
        });
        Picked {
            path: self.tree.nodes[i].path.clone(),
            key: self.tree.nodes[i].key.clone(),
            rect,
            local: self.to_local(i, x, y),
            data: self.data_path(i),
        }
    }

    /// `El::data` values from node `i` up to the root, nearest first.
    fn data_path(&self, i: usize) -> Vec<crate::el::data::ElData> {
        let mut data = Vec::new();
        let mut at = Some(i);
        while let Some(j) = at {
            let n = &self.tree.nodes[j];
            if let Some(c) = n.cold.get() { data.extend(c.data.iter().cloned()); }
            at = n.parent;
        }
        data
    }
}

impl RuntimeCore {
    /// Snapshot of what the pointer hovers for [`Tx::hovered`] and [`Tx::is_hovered`], cached
    /// until pointer, capture, modal, layout or commit state changes.
    pub(in crate::runtime) fn hover_snapshot(&self) -> Option<Rc<crate::el::HoverSnapshot>> {
        if self.pointer.left() || self.pointer.is_touch_input() { return None; }
        let (x, y) = self.pointer.pos.filter(|&(x, y)| self.in_viewport(x, y))?;
        let held = self.pointer.hover_held() && !self.pointer.captured();
        let pos = Some((x.to_bits(), y.to_bits()));
        let basis = crate::runtime::input::capture::HoverBasis {
            commits: self.commit.commits,
            commit_gen: self.commit.commit_gen(),
            build_gen: self.tree.build_gen,
            built: self.tree.built,
            pos,
            pressing: self.pointer.pressing(),
            press_inside: self.pointer.press_inside(),
            secondary_held: self.pointer.secondary_held(),
            captured: self.pointer.captured_path().map(str::to_owned),
            held,
            modal: self.active_modal(),
        };
        {
            let cache = self.pointer.hover_cache.borrow();
            if cache.basis.as_ref() == Some(&basis) {
                return cache.snapshot.clone();
            }
        }
        let snapshot = if held || !self.tree.built {
            let last = self.pointer.hover_cache.borrow().last_built.clone()
                .filter(|(p, _)| *p == pos)
                .map(|(_, s)| s);
            self.live_hover_snapshot(last.as_deref(), !self.tree.built && !held)
        } else {
            self.compute_hover_snapshot(x, y)
        }.map(Rc::new);
        let mut cache = self.pointer.hover_cache.borrow_mut();
        if self.tree.built && !held {
            cache.last_built = snapshot.clone().map(|s| (pos, s));
        }
        cache.basis = Some(basis);
        cache.snapshot = snapshot.clone();
        snapshot
    }

    fn node_visually_contains(&self, i: usize, x: f32, y: f32) -> bool {
        !self.subtree_exiting(i)
            && !self.subtree_unmounted(i)
            && self.allowed(i)
            && !self.tree.nodes[i].focus.hover_below
            && self.paint.drawn.get(i).is_some()
            && self.paint.inh_scratch.get(i).is_some_and(|h| h.opacity > 0.001)
            && self.node_contains(i, x, y)
    }

    fn hovered_link_at(&self, i: usize, x: f32, y: f32) -> Option<(String, String)> {
        if self.node_disabled(i) { return None; }
        let k = self.link_at(i, x, y)?;
        let id = self.tree.nodes[i].text.as_deref()?.cold.links.get(k)?.id.clone();
        Some((self.tree.nodes[i].path.clone(), id))
    }

    fn push_hover_leaf(paths: &mut Vec<String>, path: &str) {
        if paths.iter().any(|p| super::hover::holds(path, p)) { return; }
        paths.retain(|p| !super::hover::holds(p, path));
        paths.push(path.to_owned());
    }

    fn collect_keyed_descendants(&self, root: usize, x: f32, y: f32, paths: &mut Vec<String>) {
        let end = (root + 1..self.tree.nodes.len())
            .take_while(|&j| self.tree.is_descendant(j, root))
            .last();
        let Some(end) = end else { return };
        let mut found: Vec<usize> = (root + 1..=end)
            .rev()
            .filter(|&j| self.tree.nodes[j].key.is_some() && self.node_visually_contains(j, x, y))
            .collect();
        let order = self.paint.layer_paint_order();
        if found.len() > 1 && !order.is_empty() {
            found.sort_by_key(|&j| std::cmp::Reverse(order.iter().position(|&o| o == j).unwrap_or(0)));
        }
        for j in found {
            Self::push_hover_leaf(paths, &self.tree.nodes[j].path);
        }
    }

    fn compute_hover_snapshot(&self, x: f32, y: f32) -> Option<crate::el::HoverSnapshot> {
        let mut paths = Vec::new();
        let mut link = None;
        let mut hit = false;
        if let Some(cap) = self.pointer.captured_path() {
            let cap_i = self.hover_live(cap)?;
            let cap_on = if self.pointer.pressing() {
                self.pointer.press_inside()
            } else {
                self.node_visually_contains(cap_i, x, y)
            };
            if cap_on {
                hit = true;
                link = self.hovered_link_at(cap_i, x, y);
                self.collect_keyed_descendants(cap_i, x, y, &mut paths);
                if paths.is_empty() {
                    paths.push(self.tree.nodes[cap_i].path.clone());
                }
            } else if let Some(a) = std::iter::successors(self.tree.nodes[cap_i].parent, |&p| self.tree.nodes[p].parent)
                .find(|&a| self.node_visually_contains(a, x, y))
                && crate::el::innermost_hover_key(&self.tree.nodes[a].path).is_some()
            {
                paths.push(self.tree.nodes[a].path.clone());
            }
        } else if let Some(t) = self.top_hover_hit(x, y) {
            hit = true;
            link = self.hovered_link_at(t, x, y);
            self.collect_keyed_descendants(t, x, y, &mut paths);
            if paths.is_empty() {
                paths.push(self.tree.nodes[t].path.clone());
            }
        } else {
            let order = self.paint.layer_paint_order();
            let count = if order.is_empty() { self.tree.nodes.len() } else { order.len() };
            for rev in (0..count).rev() {
                let i = if order.is_empty() { rev } else { order[rev] };
                if self.tree.nodes[i].key.is_some() && self.node_visually_contains(i, x, y) {
                    Self::push_hover_leaf(&mut paths, &self.tree.nodes[i].path);
                }
            }
        }
        if paths.is_empty() && link.is_none() { return None; }
        let key = link.as_ref().map(|(_, id)| id.clone())
            .or_else(|| paths.iter().find_map(|p| crate::el::innermost_hover_key(p)))
            .or_else(|| hit.then(|| paths.first().map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())).flatten());
        Some(crate::el::HoverSnapshot { key, paths, link })
    }

    fn live_hover_snapshot(&self, prev: Option<&crate::el::HoverSnapshot>, allow_ancestors: bool) -> Option<crate::el::HoverSnapshot> {
        let mut paths = Vec::new();
        let mut keep = |p: &str| {
            if self.hover_live(p).is_some() {
                Self::push_hover_leaf(&mut paths, p);
            } else if allow_ancestors {
                let mut cur = p;
                while let Some((above, _)) = cur.rsplit_once('/') {
                    if self.hover_live(above).is_some() && crate::el::innermost_hover_key(above).is_some() {
                        Self::push_hover_leaf(&mut paths, above);
                        break;
                    }
                    cur = above;
                }
            }
        };
        let mut link = None;
        if let Some(prev) = prev {
            for p in &prev.paths { keep(p); }
            if let Some((p, id)) = prev.link.as_ref()
                && self.hover_live(p).is_some_and(|i| !self.node_disabled(i))
            {
                link = Some((p.clone(), id.clone()));
            }
        } else {
            if let Some(p) = self.pointer.hover_hit.as_deref() { keep(p); }
            if let Some(p) = self.pointer.hovered.as_deref() { keep(p); }
            for p in &self.pointer.hover_within { keep(p); }
            if let Some(l) = self.pointer.link_hover.as_deref()
                && self.hover_live(&l.path).is_some_and(|i| !self.node_disabled(i))
            {
                link = Some((l.path.clone(), l.link_id.clone()));
            }
        }
        if paths.is_empty() && link.is_none() { return None; }
        let unkeyed_hit = self.pointer.hover_hit.as_deref().is_some_and(|p| self.hover_live(p).is_some());
        let key = link.as_ref().map(|(_, id)| id.clone())
            .or_else(|| paths.iter().find_map(|p| crate::el::innermost_hover_key(p)))
            .or_else(|| unkeyed_hit.then(|| paths.first().map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())).flatten());
        Some(crate::el::HoverSnapshot { key, paths, link })
    }
}
