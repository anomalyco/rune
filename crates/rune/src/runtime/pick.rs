//! Point picking (`Runtime::element_at`, `element_at_with`, `element_at_in`, `contains`) and
//! picking options (`Pick`, `Picked`).
use super::*;

/// An element [`Runtime::element_at`] found.
///
/// `PartialEq` and `Eq` compare element identity (`path` and `key`), ignoring the queried
/// geometry (`rect` and `local`).
#[derive(Clone, Debug)]
pub struct Picked {
    path: String,
    key: Option<String>,
    rect: [f32; 4],
    local: [f32; 2],
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
        }
    }
}
