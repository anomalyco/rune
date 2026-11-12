//! What selectable text holds: nothing, a selection, or a selection a held press is extending.
//! A press, a drag, a release and the ways a selection is made at once or cleared are events, and
//! [`step`] moves between the three; what resolving the selection's texts asks for afterwards
//! (pinning its virtual rows, sealing the texts between its ends) comes back as effects.
use std::collections::HashMap;

use crate::machine::Effects;

use super::Unit;
use super::virtual_selection::VirtualEnd;

/// The selectable-text side of the runtime: the selection's phase, the run of presses that makes
/// double and triple clicks, Shift as the host last reported it, and where Select All goes with
/// no selection.
#[derive(Default)]
pub(in crate::runtime) struct TextSel {
    pub(in crate::runtime) phase: Phase,
    /// The run of presses on selectable text.
    pub(in crate::runtime) presses: crate::runtime::press_run::PressRun,
    /// Shift is held (from the host's modifier events), so a press extends the selection.
    pub(in crate::runtime) shift: bool,
    pub(in crate::runtime) pick: crate::runtime::select_all::Pick, // where Select All goes with no selection
    /// Hashes of mounted rows strictly between the selection's anchor and focus in a virtual
    /// list. It sits beside the phase so a new selection clears it in place and keeps its
    /// buckets; with nothing selected it's empty.
    pub(in crate::runtime) mounted_between: HashMap<String, u64>,
}

#[derive(Default)]
pub(in crate::runtime) enum Phase {
    /// No text holds a selection.
    #[default]
    Empty,
    /// A selection no press is making.
    Placed(Selection),
    /// The held press is extending this selection by `unit`, keeping whole the unit it began on
    /// (`Selection::origin`).
    Dragging { selection: Selection, unit: Unit },
}

impl Phase {
    pub(in crate::runtime) fn selection(&self) -> Option<&Selection> {
        match self {
            Phase::Empty => None,
            Phase::Placed(s) | Phase::Dragging { selection: s, .. } => Some(s),
        }
    }

    pub(in crate::runtime) fn selection_mut(&mut self) -> Option<&mut Selection> {
        match self {
            Phase::Empty => None,
            Phase::Placed(s) | Phase::Dragging { selection: s, .. } => Some(s),
        }
    }

    /// The held press is extending the selection.
    pub(in crate::runtime) fn dragging(&self) -> bool { matches!(self, Phase::Dragging { .. }) }
}

/// A selection: its anchor in one text and its focus in the same text or, in `far`, another text
/// of its selection group.
pub(in crate::runtime) struct Selection {
    /// The text holding the anchor, and its content when last seen.
    pub(in crate::runtime) path: String,
    pub(in crate::runtime) content: String,
    pub(in crate::runtime) anchor: usize,
    /// An offset into the anchor's text, or into `far`'s when that's set.
    pub(in crate::runtime) focus: usize,
    /// The range of the unit the press that made it began on, which a drag keeps whole.
    pub(in crate::runtime) origin: (usize, usize),
    /// Virtual-list row identity of the anchor when inside a virtual list.
    pub(in crate::runtime) anchor_end: Option<VirtualEnd>,
    /// The text holding the focus when it's another text of the anchor's selection group.
    pub(in crate::runtime) far: Option<Far>,
    /// A hash of the texts strictly between the anchor's and the focus's (their paths and
    /// content), taken when the selection changes, so a commit that rewrites, adds or removes
    /// one drops it.
    pub(in crate::runtime) between: u64,
}

impl Selection {
    /// `anchor..focus` in text `path` reading `content`, made from the unit range `origin`.
    pub(in crate::runtime) fn new(path: String, content: String, (anchor, focus): (usize, usize), origin: (usize, usize)) -> Self {
        Self { path, content, anchor, focus, origin, anchor_end: None, far: None, between: 0 }
    }
}

/// The text of the anchor's group holding a selection's focus.
pub(in crate::runtime) struct Far {
    /// Its path and content when last seen.
    pub(in crate::runtime) path: String,
    pub(in crate::runtime) content: String,
    /// Its virtual-list row identity when inside a virtual list.
    pub(in crate::runtime) end: Option<VirtualEnd>,
}

impl Far {
    pub(in crate::runtime) fn new(path: String, content: String) -> Self { Self { path, content, end: None } }
}

/// Where a drag's focus landed: the anchor's text, or another text of its group.
pub(in crate::runtime) enum Reach<'a> {
    Anchor,
    Far { path: &'a str, content: &'a str },
}

pub(in crate::runtime) enum Event<'a> {
    /// A primary press placed a new selection, `origin` in text `path`, to be extended by `unit`.
    Press { path: String, content: String, unit: Unit, origin: (usize, usize) },
    /// A Shift-press keeps the anchor and moves the focus to `focus`, in `far` or the anchor's
    /// text; the drag that follows extends by character.
    Extend { focus: usize, far: Option<Far> },
    /// The held press moved the selection's ends to `anchor` and `focus`, the focus in `reach`.
    Drag { anchor: usize, focus: usize, reach: Reach<'a> },
    /// The press ended: released, cancelled, lost, or taken by a draggable row.
    Release,
    /// A selection made at once: Select All, `Tx::select_text`, a secondary press's word.
    Select(Selection),
    /// The selection goes.
    Clear,
}

/// What the runtime does after a step, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum Effect {
    /// Re-pin the virtual rows the selection's ends are in.
    SyncPins,
    /// The selection changed shape: re-pin and hash the texts between its ends.
    Seal,
}

pub(in crate::runtime) fn step(phase: &mut Phase, event: Event<'_>) -> Effects<Effect> {
    match event {
        Event::Press { path, content, unit, origin } => {
            *phase = Phase::Dragging { selection: Selection::new(path, content, origin, origin), unit };
            Effects::one(Effect::Seal)
        }
        Event::Extend { focus, far } => {
            let (Phase::Placed(selection) | Phase::Dragging { selection, .. }) = std::mem::take(phase) else {
                return Effects::none();
            };
            let mut selection = selection;
            selection.origin = (selection.anchor, selection.anchor);
            selection.focus = focus;
            selection.far = far;
            *phase = Phase::Dragging { selection, unit: Unit::Char };
            Effects::one(Effect::Seal)
        }
        Event::Drag { anchor, focus, reach } => {
            let Phase::Dragging { selection: sel, .. } = phase else { return Effects::none() };
            let same_far = match reach {
                Reach::Anchor => sel.far.is_none(),
                Reach::Far { path, .. } => sel.far.as_ref().is_some_and(|f| f.path == path),
            };
            if (sel.anchor, sel.focus) == (anchor, focus) && same_far { return Effects::none(); }
            (sel.anchor, sel.focus) = (anchor, focus);
            match reach {
                Reach::Anchor => sel.far = None,
                Reach::Far { path, content } if !same_far => sel.far = Some(Far::new(path.to_owned(), content.to_owned())),
                Reach::Far { .. } => {}
            }
            Effects::one(if same_far { Effect::SyncPins } else { Effect::Seal })
        }
        Event::Release => {
            if let Phase::Dragging { .. } = phase
                && let Phase::Dragging { selection, .. } = std::mem::take(phase)
            {
                *phase = Phase::Placed(selection);
            }
            Effects::none()
        }
        Event::Select(selection) => {
            *phase = Phase::Placed(selection);
            Effects::one(Effect::Seal)
        }
        Event::Clear => {
            *phase = Phase::Empty;
            Effects::one(Effect::SyncPins)
        }
    }
}
