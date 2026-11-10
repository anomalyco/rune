//! A tab strip's pointer gestures as one explicit state machine, kept in the app's state:
//! `Idle → Pressed → Dragging(Reorder | TearOut) → Dropping → Idle`. Beside it, a tab dragged in
//! from another strip (another window's) opens a gap where it would land. Each phase carries only
//! what's true in it (where the press landed, the torn-out tab's rect, a click to swallow after
//! a drag), as Desktop's `pane-drag-machine.ts` does for its panes' tabs.

use std::cell::Cell;
use std::rc::Rc;

/// Points the pointer travels before a press becomes a reorder (a finger's 8 points are the
/// runtime's touch slop, which holds its moves back until then, as dnd-kit's touch constraint).
const SLOP: f32 = 4.0;

/// The gestures of one [`TabStrip`](super::TabStrip), owned by the app (`TabDragState::default()`)
/// and passed to [`TabStrip::drag_state`](super::TabStrip::drag_state). One instance can serve a
/// strip that moves between layouts; [`TabDragState::cancel`] drops a gesture in flight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabDragState {
    gesture: Gesture,
    /// Holds a horizontal strip's tab width while the pointer closes tabs one after another, so
    /// the next close button lands under the pointer. Set when a tab closes from the pointer;
    /// cleared when the pointer leaves the strip, and the tabs then spring to their new width.
    pub width_lock: Option<f32>,
    hovered: Option<String>,
    /// A tab dragged from another strip hovers here, in window points ([`Self::set_incoming`]).
    incoming: Option<[f32; 2]>,
    /// The slot the strip opened its gap at for the incoming tab, as it last built; shared with
    /// the copy the strip builds from, which finds it.
    landing: Rc<Cell<Option<usize>>>,
    /// In the copy the strip builds from: the incoming tab's slot and the gap's length.
    gap: Option<(usize, f32)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum TabDragPhase {
    #[default]
    Idle,
    /// The primary button went down on tab `index` and hasn't travelled past the slop yet.
    Pressed { index: usize, pinned: bool, tab_pitch: f32 },
    Dragging(TabDragMode),
    /// A reorder drop, while the strip's `on_reorder` moves the tab. Back to `Idle` right after.
    Dropping { from: usize, to: usize },
}

/// The gesture as the strip keeps it: [`TabDragPhase`] and what only each phase has.
#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    /// `swallow_click`: a drag was released over its own tab, whose click that follows does nothing.
    Idle { swallow_click: bool },
    Pressed { index: usize, pinned: bool, tab_pitch: f32, grab: Grab },
    Dragging { mode: TabDragMode, grab: Grab, lifted: Lifted },
    Dropping { from: usize, to: usize, swallow_click: bool },
}

impl Default for Gesture {
    fn default() -> Self { Self::Idle { swallow_click: false } }
}

/// Where a press landed: in the tab, in its own points; how far the strip had scrolled along
/// itself then, so a drag that scrolls it keeps the tab under the pointer; and the tab's pitch,
/// for the gap its neighbours close while it's torn out.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Grab {
    at: [f32; 2],
    scrolled: f32,
    pitch: f32,
}

/// A torn-out tab: its layout rect in window points once reported, and whether it shows in a
/// window of its own ([`TabDragState::hand_off`]). A reorder keeps neither.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Lifted {
    rect: Option<[f32; 4]>,
    handed_off: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabDragMode {
    /// Along the strip: the tab follows the pointer, held inside its partition (pinned or not),
    /// and the tabs it passes step aside.
    Reorder {
        from: usize,
        to: usize,
        /// The clamped pointer travel along the strip, in points.
        delta: f32,
        pitch: f32,
        pinned: bool,
    },
    /// Pulled off the strip: the tab follows the pointer freely and its neighbours close the gap.
    TearOut {
        from: usize,
        /// Pointer travel `[dx, dy]` in points, unclamped.
        offset: [f32; 2],
    },
}

/// A strip's tear-out gesture: dragging a tab this far across the strip detaches it, and
/// bringing it back within `reattach_distance` docks it again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TearOutSpec {
    /// Points across the strip before a drag detaches (36 for a horizontal strip, 48 vertical).
    pub detach_distance: f32,
    /// Points across the strip within which a torn-out tab rejoins the reorder (24).
    pub reattach_distance: f32,
    /// Whether the strip's only tab tears out too (false). Set it when a window's last tab can
    /// go to another window's strip, as a browser's can, and the window it leaves then closes.
    pub lone: bool,
}

impl Default for TearOutSpec {
    fn default() -> Self {
        Self { detach_distance: 36.0, reattach_distance: 24.0, lone: false }
    }
}

/// One tab's place in the gesture: its index, the range it reorders in, and its pitch.
#[derive(Clone, Copy, Debug)]
pub(super) struct Lane {
    pub index: usize,
    /// The partition it stays in: `lo..hi`.
    pub lo: usize,
    pub hi: usize,
    pub pitch: f32,
    pub pinned: bool,
    pub tear: Option<TearOutSpec>,
}

/// What a release did.
pub(super) enum Release {
    None,
    /// A press that never became a drag.
    Click,
    Reorder { from: usize, to: usize },
    TearOut { from: usize, offset: [f32; 2] },
}

impl TabDragState {
    pub fn new() -> Self {
        Self::default()
    }
    /// Where the gesture stands.
    pub fn phase(&self) -> TabDragPhase {
        match self.gesture {
            Gesture::Idle { .. } => TabDragPhase::Idle,
            Gesture::Pressed { index, pinned, tab_pitch, .. } => TabDragPhase::Pressed { index, pinned, tab_pitch },
            Gesture::Dragging { mode, .. } => TabDragPhase::Dragging(mode),
            Gesture::Dropping { from, to, .. } => TabDragPhase::Dropping { from, to },
        }
    }
    /// The tab lifted by a drag (reordering or torn out), by its index.
    pub fn dragging(&self) -> Option<usize> {
        match self.gesture {
            Gesture::Dragging { mode: TabDragMode::Reorder { from, .. } | TabDragMode::TearOut { from, .. }, .. } => Some(from),
            _ => None,
        }
    }
    /// Whether a tab is torn out of the strip.
    pub fn torn_out(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging { mode: TabDragMode::TearOut { .. }, .. })
    }
    /// The tab under the pointer, by id.
    pub fn hovered(&self) -> Option<&str> {
        self.hovered.as_deref()
    }
    /// Drops a gesture in flight (a layout switch, a rename starting): the tab springs back.
    pub fn cancel(&mut self) {
        if !matches!(self.gesture, Gesture::Idle { .. }) { self.gesture = Gesture::default(); }
    }
    /// The torn-out tab now shows in a window of its own that follows the pointer (on_tear_move):
    /// the strip stops drawing it until the gesture ends, or until it docks back into the strip.
    pub fn hand_off(&mut self) {
        if let Gesture::Dragging { mode: TabDragMode::TearOut { .. }, lifted, .. } = &mut self.gesture { lifted.handed_off = true; }
    }
    /// Whether the torn-out tab shows in a window of its own ([Self::hand_off]).
    pub fn handed_off(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging { lifted: Lifted { handed_off: true, .. }, .. })
    }
    /// The click that follows a drag released over its own tab: true once, and the click does nothing.
    pub(super) fn take_swallowed_click(&mut self) -> bool {
        match &mut self.gesture {
            Gesture::Idle { swallow_click } => std::mem::take(swallow_click),
            _ => false,
        }
    }
    /// The strip's `on_reorder` moved the dropped tab: the gesture is over.
    pub(super) fn dropped(&mut self) {
        if let Gesture::Dropping { swallow_click, .. } = self.gesture { self.gesture = Gesture::Idle { swallow_click }; }
    }
    /// A tab dragged from another strip (another window's) is over this one at `point`, in this
    /// window's points, or has left it (`None`): the tabs past the slot it would land in step
    /// aside on the strip's reorder spring, as they do for a tab dragged within the strip, and
    /// close again when it leaves.
    pub fn set_incoming(&mut self, point: Option<[f32; 2]>) {
        self.incoming = point;
        if point.is_none() {
            self.landing.set(None);
        }
    }
    /// Where the incoming tab lands if it's let go now: the slot the strip opened its gap at, from
    /// the tabs' own lengths and the strip's scroll, as the strip last built. `None` before the
    /// strip built with it, or once it left.
    pub fn incoming_slot(&self) -> Option<usize> {
        self.incoming.and(self.landing.get())
    }
    pub(super) fn incoming(&self) -> Option<[f32; 2]> {
        self.incoming.filter(|_| matches!(self.gesture, Gesture::Idle { .. }))
    }
    /// Whether the strip opened a gap for an incoming tab: its tabs step aside on the reorder spring.
    pub(super) fn gap_open(&self) -> bool {
        self.gap.is_some()
    }
    /// The incoming tab's slot and length, in the copy the strip builds from.
    pub(super) fn gap(&self) -> Option<(usize, f32)> {
        self.gap
    }
    /// In the copy the strip builds from: the incoming tab lands at `slot`, `pitch` long.
    pub(super) fn open_gap(&mut self, slot: usize, pitch: f32) {
        self.landing.set(Some(slot));
        self.gap = Some((slot, pitch));
    }

    pub(super) fn set_hovered(&mut self, id: &str, inside: bool) {
        if inside {
            self.hovered = Some(id.to_owned());
        } else if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
    }
    pub(super) fn leave(&mut self) {
        self.hovered = None;
        self.width_lock = None;
    }
    pub(super) fn report_rect(&mut self, rect: [f32; 4]) {
        if let Gesture::Dragging { mode: TabDragMode::TearOut { .. }, lifted, .. } = &mut self.gesture { lifted.rect = Some(rect); }
    }
    /// The lifted tab's layout rect [x, y, w, h] in window points, once reported.
    pub(super) fn lifted_rect(&self) -> Option<[f32; 4]> {
        match self.gesture {
            Gesture::Dragging { lifted, .. } => lifted.rect,
            _ => None,
        }
    }

    fn grip(&self) -> Option<Grab> {
        match self.gesture {
            Gesture::Pressed { grab, .. } | Gesture::Dragging { grab, .. } => Some(grab),
            _ => None,
        }
    }

    /// Where the press landed in the tab, in its own points.
    pub(super) fn grab(&self) -> [f32; 2] {
        self.grip().map_or([0.0; 2], |g| g.at)
    }

    /// The press landed at `grab` in the tab, with the strip scrolled `scrolled` along itself.
    pub(super) fn press(&mut self, lane: &Lane, grab: [f32; 2], scrolled: f32) {
        let grab = Grab { at: grab, scrolled, pitch: lane.pitch };
        self.gesture = Gesture::Pressed { index: lane.index, pinned: lane.pinned, tab_pitch: lane.pitch, grab };
    }

    /// How much further the strip has scrolled since the press, at `scrolled` now.
    pub(super) fn scrolled_since(&self, scrolled: f32) -> f32 {
        self.grip().map_or(0.0, |g| scrolled - g.scrolled)
    }

    /// Whether a tab is being dragged along the strip.
    pub(super) fn reordering(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging { mode: TabDragMode::Reorder { .. }, .. })
    }

    /// The pointer moved `along` and `across` the strip since the press (points), `offset` in
    /// both axes. Returns whether the phase changed or the lifted tab moved.
    pub(super) fn follow(&mut self, lane: &Lane, along: f32, across: f32, offset: [f32; 2]) -> bool {
        let torn = self.torn_out();
        let Some(grab) = self.grip() else { return false };
        let lifted = match self.gesture {
            Gesture::Dragging { mode: TabDragMode::TearOut { .. }, lifted, .. } => lifted,
            _ => Lifted::default(),
        };
        // A torn-out tab docks again only back over the strip: near it across, and along it
        // within a tab of its ends, not anywhere level with it.
        let pitch = lane.pitch.max(1.0);
        let over = (((lane.lo as f32 - lane.index as f32) - 1.0) * pitch..=((lane.hi as f32 - lane.index as f32) * pitch)).contains(&along);
        let detach = lane.tear.is_some_and(|t| if torn { across.abs() > t.reattach_distance || !over } else { across.abs() >= t.detach_distance });
        if detach {
            self.gesture = Gesture::Dragging { mode: TabDragMode::TearOut { from: lane.index, offset }, grab, lifted };
            return true;
        }
        let reordering = torn || self.reordering();
        // dnd-kit's Distance constraint measures the travel either way.
        if !reordering && along.hypot(across) < SLOP {
            return false;
        }
        let min = (lane.lo as f32 - lane.index as f32) * pitch;
        let max = (lane.hi.saturating_sub(1) as f32 - lane.index as f32) * pitch;
        let delta = along.clamp(min.min(0.0), max.max(0.0));
        let to = (lane.index as f32 + (delta / pitch).round()).max(0.0) as usize;
        let mode = TabDragMode::Reorder { from: lane.index, to, delta, pitch, pinned: lane.pinned };
        self.gesture = Gesture::Dragging { mode, grab, lifted: Lifted::default() };
        true
    }

    /// The button came up, `cancelled` by Escape or a lost window, `on_tab` with the pointer
    /// still over the pressed tab (a click follows the release then).
    /// `swallow` decides, for a press that never became a drag, whether its click does nothing
    /// (the press already selected the tab).
    pub(super) fn release(&mut self, cancelled: bool, on_tab: bool, swallow: bool) -> Release {
        let (gesture, release) = match std::mem::take(&mut self.gesture) {
            Gesture::Pressed { .. } => (Gesture::Idle { swallow_click: swallow }, Release::Click),
            Gesture::Dragging { mode: TabDragMode::Reorder { from, to, .. }, .. } if !cancelled && from != to =>
                (Gesture::Dropping { from, to, swallow_click: on_tab }, Release::Reorder { from, to }),
            Gesture::Dragging { mode: TabDragMode::TearOut { from, offset }, .. } if !cancelled =>
                (Gesture::Idle { swallow_click: on_tab }, Release::TearOut { from, offset }),
            Gesture::Dragging { .. } => (Gesture::Idle { swallow_click: on_tab }, Release::None),
            other => (other, Release::None),
        };
        self.gesture = gesture;
        release
    }

    /// How far tab `index` sits from its place, `[x, y]`.
    pub(super) fn shift(&self, index: usize, vertical: bool) -> [f32; 2] {
        let axis = |along: f32| if vertical { [0.0, along] } else { [along, 0.0] };
        match self.gesture {
            Gesture::Dragging { mode: TabDragMode::Reorder { from, to, delta, pitch, .. }, .. } => {
                let along = if index == from {
                    delta
                } else if from < index && index <= to {
                    -pitch
                } else if to <= index && index < from {
                    pitch
                } else {
                    0.0
                };
                axis(along)
            }
            Gesture::Dragging { mode: TabDragMode::TearOut { from, offset }, grab, .. } => match index {
                i if i == from => offset,
                i if i > from => axis(-grab.pitch),
                _ => [0.0; 2],
            },
            _ => [0.0; 2],
        }
    }
}
