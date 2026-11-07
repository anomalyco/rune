//! A tab strip's pointer gestures as one explicit state machine, kept in the app's state:
//! `Idle → Pressed → Dragging(Reorder | TearOut) → Dropping → Idle`.

/// Points the pointer travels before a press becomes a reorder (a finger's 8 points are the
/// runtime's touch slop, which holds its moves back until then, as dnd-kit's touch constraint).
const SLOP: f32 = 4.0;

/// The gestures of one [`TabStrip`](super::TabStrip), owned by the app (`TabDragState::default()`)
/// and passed to [`TabStrip::drag_state`](super::TabStrip::drag_state). One instance can serve a
/// strip that moves between layouts; [`TabDragState::cancel`] drops a gesture in flight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabDragState {
    pub phase: TabDragPhase,
    /// Holds a horizontal strip's tab width while the pointer closes tabs one after another, so
    /// the next close button lands under the pointer. Set when a tab closes from the pointer;
    /// cleared when the pointer leaves the strip, and the tabs then spring to their new width.
    pub width_lock: Option<f32>,
    /// Swallows the click that follows a drag released over its own tab.
    pub suppress_click: bool,
    hovered: Option<String>,
    /// The torn-out or lifted tab's pitch, for the gap its neighbours close.
    lift_pitch: f32,
    /// Where the press landed in the tab, in its own points.
    grab: [f32; 2],
    /// The lifted tab's layout rect in window points, reported while it's torn out.
    rect: Option<[f32; 4]>,
    /// The torn-out tab shows in a window of its own now ([TabDragState::hand_off]).
    handed_off: bool,
    /// How far the strip had scrolled along itself when the press landed, so a drag that
    /// scrolls it keeps the tab under the pointer.
    scrolled: f32,
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
}

impl Default for TearOutSpec {
    fn default() -> Self {
        Self { detach_distance: 36.0, reattach_distance: 24.0 }
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
    /// The tab lifted by a drag (reordering or torn out), by its index.
    pub fn dragging(&self) -> Option<usize> {
        match self.phase {
            TabDragPhase::Dragging(TabDragMode::Reorder { from, .. } | TabDragMode::TearOut { from, .. }) => Some(from),
            _ => None,
        }
    }
    /// Whether a tab is torn out of the strip.
    pub fn torn_out(&self) -> bool {
        matches!(self.phase, TabDragPhase::Dragging(TabDragMode::TearOut { .. }))
    }
    /// The tab under the pointer, by id.
    pub fn hovered(&self) -> Option<&str> {
        self.hovered.as_deref()
    }
    /// Drops a gesture in flight (a layout switch, a rename starting): the tab springs back.
    pub fn cancel(&mut self) {
        self.phase = TabDragPhase::Idle;
        self.rect = None;
        self.handed_off = false;
    }
    /// The torn-out tab now shows in a window of its own that follows the pointer (on_tear_move):
    /// the strip stops drawing it until the gesture ends, or until it docks back into the strip.
    pub fn hand_off(&mut self) {
        self.handed_off = self.torn_out();
    }
    /// Whether the torn-out tab shows in a window of its own ([Self::hand_off]).
    pub fn handed_off(&self) -> bool {
        self.handed_off
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
        self.rect = Some(rect);
    }
    /// The lifted tab's layout rect [x, y, w, h] in window points, once reported.
    pub(super) fn lifted_rect(&self) -> Option<[f32; 4]> {
        self.rect
    }

    /// Where the press landed in the tab, in its own points.
    pub(super) fn grab(&self) -> [f32; 2] {
        self.grab
    }

    /// The press landed at `grab` in the tab, with the strip scrolled `scrolled` along itself.
    pub(super) fn press(&mut self, lane: &Lane, grab: [f32; 2], scrolled: f32) {
        self.phase = TabDragPhase::Pressed { index: lane.index, pinned: lane.pinned, tab_pitch: lane.pitch };
        self.suppress_click = false;
        self.lift_pitch = lane.pitch;
        self.grab = grab;
        self.scrolled = scrolled;
    }

    /// How much further the strip has scrolled since the press, at `scrolled` now.
    pub(super) fn scrolled_since(&self, scrolled: f32) -> f32 {
        scrolled - self.scrolled
    }

    /// Whether a tab is being dragged along the strip.
    pub(super) fn reordering(&self) -> bool {
        matches!(self.phase, TabDragPhase::Dragging(TabDragMode::Reorder { .. }))
    }

    /// The pointer moved `along` and `across` the strip since the press (points), `offset` in
    /// both axes. Returns whether the phase changed or the lifted tab moved.
    pub(super) fn follow(&mut self, lane: &Lane, along: f32, across: f32, offset: [f32; 2]) -> bool {
        let torn = self.torn_out();
        match self.phase {
            TabDragPhase::Pressed { .. } | TabDragPhase::Dragging(_) => {}
            _ => return false,
        }
        // A torn-out tab docks again only back over the strip: near it across, and along it
        // within a tab of its ends, not anywhere level with it.
        let pitch = lane.pitch.max(1.0);
        let over = (((lane.lo as f32 - lane.index as f32) - 1.0) * pitch..=((lane.hi as f32 - lane.index as f32) * pitch)).contains(&along);
        let detach = lane.tear.is_some_and(|t| if torn { across.abs() > t.reattach_distance || !over } else { across.abs() >= t.detach_distance });
        if detach {
            self.phase = TabDragPhase::Dragging(TabDragMode::TearOut { from: lane.index, offset });
            return true;
        }
        let reordering = torn || matches!(self.phase, TabDragPhase::Dragging(TabDragMode::Reorder { .. }));
        // dnd-kit's Distance constraint measures the travel either way.
        if !reordering && along.hypot(across) < SLOP {
            return false;
        }
        let min = (lane.lo as f32 - lane.index as f32) * pitch;
        let max = (lane.hi.saturating_sub(1) as f32 - lane.index as f32) * pitch;
        let delta = along.clamp(min.min(0.0), max.max(0.0));
        let to = (lane.index as f32 + (delta / pitch).round()).max(0.0) as usize;
        self.rect = None;
        self.handed_off = false;
        self.phase = TabDragPhase::Dragging(TabDragMode::Reorder { from: lane.index, to, delta, pitch, pinned: lane.pinned });
        true
    }

    /// The button came up, `cancelled` by Escape or a lost window, `on_tab` with the pointer
    /// still over the pressed tab (a click follows the release then).
    pub(super) fn release(&mut self, cancelled: bool, on_tab: bool) -> Release {
        let phase = std::mem::take(&mut self.phase);
        self.rect = None;
        self.handed_off = false;
        match phase {
            TabDragPhase::Pressed { .. } => Release::Click,
            TabDragPhase::Dragging(TabDragMode::Reorder { from, to, .. }) => {
                self.suppress_click = on_tab;
                if cancelled || from == to {
                    return Release::None;
                }
                self.phase = TabDragPhase::Dropping { from, to };
                Release::Reorder { from, to }
            }
            TabDragPhase::Dragging(TabDragMode::TearOut { from, offset }) => {
                self.suppress_click = on_tab;
                if cancelled {
                    return Release::None;
                }
                Release::TearOut { from, offset }
            }
            _ => Release::None,
        }
    }

    /// How far tab `index` sits from its place, `[x, y]`.
    pub(super) fn shift(&self, index: usize, vertical: bool) -> [f32; 2] {
        let axis = |along: f32| if vertical { [0.0, along] } else { [along, 0.0] };
        match self.phase {
            TabDragPhase::Dragging(TabDragMode::Reorder { from, to, delta, pitch, .. }) => {
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
            TabDragPhase::Dragging(TabDragMode::TearOut { from, offset }) => match index {
                i if i == from => offset,
                i if i > from => axis(-self.lift_pitch),
                _ => [0.0; 2],
            },
            _ => [0.0; 2],
        }
    }
}
