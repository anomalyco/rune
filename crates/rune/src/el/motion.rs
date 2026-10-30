//! Motion builders: springs, offsets, raises, leads, layout animation and transitions.

use super::*;

impl El {
    /// The spring for this subtree's layout and colour changes (inherited).
    /// Presence uses the transition's own enter/exit springs.
    pub fn motion(mut self, s: Spring) -> Self {
        self.motion = Some(s);
        self
    }
    /// Shift this element and its subtree by `(x, y)` points without moving it in layout:
    /// siblings keep their places, and hit tests follow the shift. A change springs on the
    /// element's layout spring (or [`Self::offset_motion`]); under Reduce Motion, and in an
    /// instant transaction, it lands at once. A reordered row slides aside this way while a
    /// dragged one passes it. When a commit both changes the offset and moves the element in a
    /// growth or virtual list (whose rows lay out at once), the offset starts from where the
    /// element was drawn, so a dropped row that clears its offset slides into its new place.
    /// Not for an element with a `live_offset` or a sliding loop, which own the same shift.
    pub fn offset(mut self, x: f32, y: f32) -> Self {
        let ok = |v: f32| if v.is_finite() { v } else { 0.0 };
        self.vis.offset = [ok(x), ok(y)];
        self
    }
    /// The curve [`Self::offset`] changes on.
    pub fn offset_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.cold_mut().offset_motion = Some(Box::new(anim.into()));
        self
    }
    /// Paint this element and its subtree above everything else in the window, and give it
    /// pointer input first, as if it came last in the tree. Its layout, focus order and
    /// accessibility order stay where it is, and its ancestors' clips, offsets and opacity
    /// still apply, though an ancestor that composites its children as a group (a partial
    /// opacity, a blur) no longer groups it. Raised elements stack in tree order. A floating
    /// sidebar sliding over the content beside it, or a dragged row over its neighbours.
    pub fn raise(mut self) -> Self {
        self.vis.raise = true;
        self
    }
    /// Override the curve this element's own opacity changes on, with an optional delay
    /// ([`AnimExt::delay`](crate::AnimExt::delay)). The first paint and explicit
    /// instant/settle transactions still land immediately. A new element's entrance runs on
    /// its [`transition`](Self::transition) and multiplies this opacity; hold all of it with
    /// [`enter_delay`](Self::enter_delay).
    pub fn opacity_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.opacity_motion = Some(Box::new(anim.into()));
        self
    }
    /// When this element travels, its leading edge moves on `lead` and its trailing edge on `trail`
    /// (width leads).
    pub fn lead(mut self, lead: Spring, trail: Spring) -> Self {
        self.lead = Some((lead, trail));
        self
    }
    /// The X axis springs faster (`0.78×` duration) when this element resizes or travels.
    pub fn width_leads(mut self) -> Self {
        self.width_leads = true;
        self
    }
    /// Spring this subtree's horizontal layout (x and width) inside a growth-stack row, and
    /// fade in children it gains while the row is in place. A row's contents otherwise snap
    /// while the row's height carries the motion, so a label that changes width in place jumps
    /// its neighbours. Vertical place stays with the row's growth.
    pub fn animate_layout(mut self) -> Self {
        self.animate_layout = true;
        self
    }
    /// Land every change inside this element at once, as a [`Tx::instant`] commit would, even
    /// in a commit that animates the rest of the window: its own and its subtree's layout,
    /// opacity and transforms, the elements it gains or loses, and growth rows. Its own entrance
    /// and exit keep their transition, and colours (a hover highlight) keep their springs. A
    /// popup of search results under a field whose edits animate the layout around it.
    pub fn instant_changes(mut self) -> Self {
        self.instant_changes = true;
        self
    }
    /// How this element enters and exits when mounted or removed after the first frame
    /// (default: [`Transition::opacity()`] — a 0.2 s enter and 0.15 s exit fade). Because
    /// every element has this default fade, removing an element in an animated commit
    /// (`AppSpec::on_key`, `El::on_key`, `on_submit`, `on_press`) fades it out over 0.15 s,
    /// whereas commits that set [`Tx::instant`] or [`Tx::settle`] (such as typing in a
    /// focused input or clicking into a text field) unmount removed elements in the same
    /// frame. Call `tx.instant()` or `tx.settle()` in the handler when a key or button
    /// action should unmount elements at once. Rows inserted into a growth stack only grow
    /// open unless they set one, so `.transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))`
    /// on a row also slides and fades it in. Rows a virtual list remounts on scroll never replay it.
    /// [`enter_delay`](Self::enter_delay) holds the whole entrance, as a staggered list does.
    pub fn transition(mut self, t: Transition) -> Self {
        self.transition = Some(Box::new(t));
        self
    }
    /// Shared-element motion (SwiftUI `matchedGeometryEffect`, Motion `layoutId`). When an
    /// element with `id` leaves the tree in the same commit that another with `id` mounts
    /// somewhere else, the new one starts exactly where the old one was seen, with its
    /// velocity, corner radius, fill and opacity, and springs to its own layout on one straight
    /// path. The old one disappears at once; neither plays its transition.
    ///
    /// Ids are per window. While it flies the element draws above the whole window at its own
    /// opacity, inside its destination's clips (a scroll viewport included), and its children
    /// keep their final layout, scaled uniformly to cover the moving frame and clipped to it,
    /// so a `pixels_fit(.., SurfaceFit::Fill)` picture zooms without distorting. Text moves
    /// with its box but keeps its size, as it does under any scale. The spring is
    /// `tok::motion::REFLOW` unless the commit's `Tx::motion` or this element's
    /// [`motion`](Self::motion) sets one. Under Reduce Motion nothing is matched: the old one
    /// exits and the new one enters by their own transitions, in place.
    /// When two elements share an id in one tree, the last one in tree order is matched.
    pub fn matched(mut self, id: impl Into<String>) -> Self {
        self.matched = Some(Rc::from(id.into()));
        self
    }
}
