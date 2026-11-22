//! Layout direction: `El::dir` lays a subtree out right to left (or left to right inside one
//! that is), as HTML's `dir` attribute does, and `El::mirror_in_rtl` flips a directional icon.
//!
//! Layout runs left to right; a right-to-left element then mirrors where its children sit inside
//! it, so a row's first child is at its right, start and end alignment swap, and an absolute
//! child's left inset is its right. Its own padding, border sides, corner radii, soft clip
//! edges and linear gradients swap left and right with it, as CSS's logical `start` and `end`
//! resolve: in this model `left` means start (`El::unmirrored` keeps an element physical).
//! An `El::anchored` overlay's `Left` and `Right` sides and a `Top` or `Bottom` overlay's
//! `Start` and `End` alignment swap too. Shadows, `El::offset` and other visual transforms
//! stay physical, as CSS's do. Text lays out with a right-to-left paragraph
//! direction (bidi base level), so its `Start` alignment is its right edge.
//!
//! Each element's direction resolves as one step down the tree ([`step`]), from its parent's
//! [`Flow`] and its own [`DirEvent`]. Nothing is mirrored in a window that sets no
//! right-to-left direction: layout reads one small enum per element.
use super::*;

pub use rune_text::Direction;
pub(crate) use rune_text::Flow;
use rune_text::first_strong_char;

impl El {
    /// Lays this element and its subtree out in `dir` (see [`Direction`]): `Rtl` mirrors rows,
    /// start and end alignment, padding, border sides, corner radii, anchored overlays and
    /// horizontal scrolling, and gives text a right-to-left paragraph direction; `Ltr` inside a
    /// right-to-left subtree keeps an island in its own order (a path, a command, code), and
    /// `Auto` takes the direction of the text it holds (a title, a name, a message). Elements
    /// that set none take their parent's; the window's root runs left to right.
    pub fn dir(mut self, dir: Direction) -> Self {
        self.flags.set_dir(dir);
        self
    }

    /// Keeps this element where left-to-right layout puts it inside a right-to-left parent,
    /// unmirrored, as a CSS physical `left` or `right` inset stays put under `dir="rtl"`: room
    /// kept clear of the window's own controls (Windows' caption buttons stay on the right,
    /// macOS's traffic lights on the left), or a pin that must not swap sides. Its own subtree
    /// still lays out in its direction.
    pub fn unmirrored(mut self) -> Self {
        self.flags.set(ElFlags::UNMIRRORED, true);
        self
    }

    /// Flips this element's icon left to right when it lays out right to left, for an icon
    /// that points along the line (a chevron, an arrow, a back button), as `@opencode/ui`'s
    /// `data-directional` icons do. Icons that don't point keep their drawing. A turned element
    /// or icon (`El::rotate`, `El::icon_rotation`) turns the other way, as a CSS `scaleX(-1)`
    /// outside its rotation: a collapsed disclosure's chevron turned to point right points left.
    pub fn mirror_in_rtl(mut self) -> Self {
        self.vis.cold_mut().mirror_rtl = true;
        self
    }
}

/// Where an element sits in its parent: where its parent's flow puts it, or where left-to-right
/// layout does whatever the flow (`El::unmirrored`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pin {
    #[default]
    Flowing,
    Physical,
}

impl Pin {
    pub(crate) fn of(el: &El) -> Pin {
        if el.flags.has(ElFlags::UNMIRRORED) { Pin::Physical } else { Pin::Flowing }
    }
}

/// What an element says about its own direction, the event that steps its parent's flow to its.
pub(crate) enum DirEvent<'a> {
    /// No `El::dir`: it takes its parent's.
    Inherit,
    /// `El::dir(Ltr)` or `El::dir(Rtl)`.
    Set(Flow),
    /// `El::dir(Auto)`: its first strong character decides, its parent's flow when none does.
    Auto(&'a El),
}

impl<'a> DirEvent<'a> {
    pub(crate) fn of(el: &'a El) -> DirEvent<'a> {
        // A field Ctrl+Shift set lays out that way, its padding and placeholder with it.
        if let Some(flow) = el.cold.input.as_ref().and_then(|i| i.edit.forced_dir()) {
            return DirEvent::Set(flow);
        }
        match el.flags.dir() {
            None => DirEvent::Inherit,
            Some(Direction::Ltr) => DirEvent::Set(Flow::Ltr),
            Some(Direction::Rtl) => DirEvent::Set(Flow::Rtl),
            Some(Direction::Auto) => DirEvent::Auto(el),
        }
    }
}

/// What flattening does to an element once its flow is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirEffect {
    /// Left to right: as authored.
    Keep,
    /// Right to left: its own left and right sides (`mirror_visual`) and its anchored
    /// overlay's side (`mirror_anchor`) swap.
    Mirror,
}

/// One step down the tree: the parent's flow and the element's own direction give its flow,
/// and what it has to mirror.
pub(crate) fn step(parent: Flow, event: DirEvent<'_>) -> (Flow, DirEffect) {
    let flow = match event {
        DirEvent::Inherit => parent,
        DirEvent::Set(flow) => flow,
        // With no strong character (empty, digits, a path alone) it reads the way its parent
        // does: an empty field in an Arabic window starts at the right.
        DirEvent::Auto(el) => match first_strong(el) {
            Some(Direction::Rtl) => Flow::Rtl,
            Some(_) => Flow::Ltr,
            None => parent,
        },
    };
    let effect = match flow {
        Flow::Ltr => DirEffect::Keep,
        Flow::Rtl => DirEffect::Mirror,
    };
    (flow, effect)
}

/// The first strong direction in `el`'s text, then its children's in order, skipping children
/// that set their own direction.
fn first_strong(el: &El) -> Option<Direction> {
    // A by-paragraph field reads as a `contenteditable` with `dir="auto"`: its text across its
    // lines, each mention pill (a `dir="auto"` span) skipped, and none while it's empty (its
    // placeholder is another element).
    if let (Some(t), Some(_)) = (&el.text, el.cold.input.as_ref())
        && matches!(t.dir, TextDir::Plaintext(_))
    {
        let mut at = 0;
        for p in t.cold.pills.iter().map(|p| (p.start as usize, p.end as usize)).chain([(t.content.len(), t.content.len())]) {
            if let Some(d) = t.content.get(at..p.0).and_then(|s| first_strong_char(s, true)) {
                return Some(d);
            }
            at = at.max(p.1);
        }
        return None;
    }
    if let Some(t) = &el.text
        && let Some(d) = Direction::of(&t.content)
    {
        return Some(d);
    }
    // An empty field reads its placeholder, as the placeholder shows in its place.
    if let Some(i) = el.cold.input.as_ref().filter(|i| !i.edit.is_secure())
        && let Some(d) = Direction::of(if i.edit.text.is_empty() { &i.placeholder } else { &i.edit.text })
    {
        return Some(d);
    }
    el.all_children().filter(|c| c.flags.dir().is_none()).find_map(first_strong)
}

/// Mirrors what a right-to-left element draws on its own left and right: corner radii, border
/// sides, soft clip edges and an angled linear gradient's run.
pub(crate) fn mirror_visual(vis: &mut Visual) {
    // Uniform corners (most boxes) and no extras: nothing to mirror, nothing touched.
    if vis.radius.is_nan() {
        let [tl, tr, br, bl] = vis.radii();
        vis.set_radii([tr, tl, bl, br]);
    }
    let Some(cold) = vis.cold.get() else { return };
    let turns = cold.mirror_rtl && (cold.rotate != 0.0 || cold.icon_turn != 0.0);
    let flips = turns || cold.border_sides.is_some() || cold.clip.is_some() || cold.clip_corners.is_some()
        || cold.radial.as_ref().is_some_and(|r| matches!(r.geometry, GradientGeometry::Linear { .. }));
    if !flips {
        return;
    }
    let cold = vis.cold_mut();
    if cold.mirror_rtl {
        // A mirrored drawing turns the other way.
        cold.rotate = -cold.rotate;
        cold.icon_turn = -cold.icon_turn;
    }
    if let Some(r) = &mut cold.radial
        && let GradientGeometry::Linear { angle } = &mut r.geometry
    {
        // CSS angles run clockwise from up: a left-to-right ramp runs right to left.
        *angle = -*angle;
    }
    if let Some([t, r, b, l]) = cold.border_sides {
        cold.border_sides = Some([t, l, b, r]);
    }
    if let Some([l, t, r, b]) = cold.clip {
        cold.clip = Some([r, t, l, b]);
    }
    if let Some([tl, tr, br, bl]) = cold.clip_corners {
        cold.clip_corners = Some([tr, tl, bl, br]);
    }
}
/// Mirrors `Side::Left` and `Side::Right` in right-to-left layout.
pub(crate) fn mirror_side(s: Side) -> Side {
    match s {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
        other => other,
    }
}

/// An anchored overlay's placement as a right-to-left element means it: `Left` and `Right`
/// swap, and along a `Top` or `Bottom` side so do `Start` and `End`.
pub(crate) fn mirror_anchor(a: &mut Anchor) {
    a.side = mirror_side(a.side);
    if matches!(a.side, Side::Top | Side::Bottom) {
        a.align = match a.align {
            Align::Start => Align::End,
            Align::End => Align::Start,
            Align::Center => Align::Center,
        };
    }
}
