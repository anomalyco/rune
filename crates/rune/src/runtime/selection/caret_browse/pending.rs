//! Explicit state machine for caret-browsing moves that must scroll and mount an unmounted
//! `virtual_rows` / `virtualize` row before landing the caret or extending the selection.
//!
//! No timers and no idle wakes: every transition is driven by a key event (`Request`), a
//! post-scroll mount check (`Check`), or an explicit cancellation (`Cancel` on blur, clear,
//! pointer press, or replacement move).

use crate::machine::Effects;

/// Maximum consecutive scroll/remount rounds per key move (handles estimated-vs-measured height
/// adjustments and skipping consecutive non-selectable rows without looping).
pub(crate) const MAX_SEEK_HOPS: u8 = 8;

/// How the caret lands once the destination virtual row is mounted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Landing {
    /// Start of the first selectable text in the row (Right from previous row, DocStart).
    Start,
    /// End of the last selectable text in the row (Left from next row, DocEnd).
    End,
    /// Crossing a row boundary forward by word (`Option+Right` on macOS takes the first word;
    /// `Ctrl+Right` elsewhere lands at byte 0).
    WordForward,
    /// Crossing a row boundary backward by word (`Ctrl/Option+Left` takes the last word's start).
    WordBackward,
    /// First visual line of the row's first selectable text at unscrolled `goal_x` (Down).
    TopAtX(f32),
    /// Last visual line of the row's last selectable text at unscrolled `goal_x` (Up).
    BottomAtX(f32),
    /// Unscrolled point `(goal_x, list_y)` inside the virtual list (PageUp / PageDown).
    PageAt { goal_x: f32, list_y: f32, down: bool },
    /// Exact byte offset inside `Target::rel_path` (collapsing to an offscreen anchor or
    /// returning to an offscreen focus row).
    ExactOffset { offset: usize },
}

/// Which selection owner a pending virtual-row move belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scope {
    /// An opt-in `El::caret_browsing(true)` host that currently holds keyboard focus.
    #[default]
    FocusedCaretHost,
    /// An active selection on plain `El::selectable(true)` text or a `selection_group`.
    ActiveSelection,
}

/// Destination of a pending virtual-row caret move.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Target {
    pub scope: Scope,
    pub host_path: String,
    pub list_path: String,
    pub row_index: usize,
    pub row_key: String,
    /// Relative path inside the row for `Landing::ExactOffset`.
    pub rel_path: Option<String>,
    pub landing: Landing,
    pub shift: bool,
    /// Sticky unscrolled horizontal goal preserved across vertical/page steps.
    pub goal_x: Option<f32>,
    /// Direction to scan if the mounted target row holds no selectable text (`1`, `-1`, or `0`).
    pub scan_dir: i8,
}

/// Pending virtual-row caret move state.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum State {
    /// No virtual-row caret move in flight.
    #[default]
    Idle,
    /// Scrolling `target.list_path` to mount `target.row_key` before landing.
    Seeking {
        target: Target,
        hops: u8,
    },
}

impl State {
    #[inline]
    pub(crate) fn is_seeking(&self) -> bool {
        matches!(self, Self::Seeking { .. })
    }

    #[inline]
    pub(crate) fn target(&self) -> Option<&Target> {
        match self {
            Self::Idle => None,
            Self::Seeking { target, .. } => Some(target),
        }
    }
}

/// Input events to [`step`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// Start seeking a new unmounted destination (replaces any prior seek).
    Request(Target),
    /// Result of inspecting the virtual list after a scroll/remount round.
    Check {
        /// Whether the caret-browsing host still holds focus and the virtual list is live.
        host_valid: bool,
        /// Current index of `target.row_key` in the virtual list (`None` if deleted/rekeyed).
        current_index: Option<usize>,
        /// Whether that row is currently mounted in the runtime tree.
        mounted: bool,
        /// Whether the mounted row has at least one live selectable text belonging to the host.
        has_selectable: bool,
        /// Next candidate row `(index, key)` in `target.scan_dir` when `!has_selectable`.
        next_in_scan: Option<(usize, String)>,
    },
    /// Blur, clear, pointer press, or replacement mounted move: drop any pending seek.
    Cancel,
}

/// Side effects requested by [`step`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Scroll the virtual list's viewport to reveal `row_index` (or `page_y`) and remount its
    /// virtual window.
    ScrollAndMount {
        list_path: String,
        row_index: usize,
        row_key: String,
        page_y: Option<f32>,
        down: bool,
    },
    /// The target row is mounted and selectable: land the caret/selection on it now.
    Land(Target),
}

fn scroll_effect(target: &Target) -> Effect {
    let (page_y, down) = match target.landing {
        Landing::PageAt { list_y, down, .. } => (Some(list_y), down),
        Landing::Start | Landing::WordForward | Landing::TopAtX(_) => (None, true),
        Landing::End | Landing::WordBackward | Landing::BottomAtX(_) => (None, false),
        Landing::ExactOffset { .. } => (None, target.scan_dir >= 0),
    };
    Effect::ScrollAndMount {
        list_path: target.list_path.clone(),
        row_index: target.row_index,
        row_key: target.row_key.clone(),
        page_y,
        down,
    }
}

/// Pure transition function for pending virtual-row caret moves.
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::Request(target) => {
            let eff = scroll_effect(&target);
            *state = State::Seeking { target, hops: 0 };
            Effects::one(eff)
        }
        Event::Check { host_valid, current_index, mounted, has_selectable, next_in_scan } => {
            let State::Seeking { target, hops } = state else {
                return Effects::none();
            };
            let Some(idx) = current_index.filter(|_| host_valid) else {
                *state = State::Idle;
                return Effects::none();
            };
            target.row_index = idx;
            if mounted && has_selectable {
                let State::Seeking { target, .. } = std::mem::take(state) else {
                    return Effects::none();
                };
                return Effects::one(Effect::Land(target));
            }
            if mounted && !has_selectable {
                if *hops < MAX_SEEK_HOPS && let Some((ni, nk)) = next_in_scan {
                    *hops += 1;
                    target.row_index = ni;
                    target.row_key = nk;
                    let eff = scroll_effect(target);
                    return Effects::one(eff);
                }
                *state = State::Idle;
                return Effects::none();
            }
            // Still not mounted (e.g. estimated heights shifted): retry up to MAX_SEEK_HOPS.
            if *hops < MAX_SEEK_HOPS {
                *hops += 1;
                let eff = scroll_effect(target);
                return Effects::one(eff);
            }
            *state = State::Idle;
            Effects::none()
        }
        Event::Cancel => {
            *state = State::Idle;
            Effects::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(row_index: usize, key: &str, scan_dir: i8) -> Target {
        Target {
            scope: Scope::FocusedCaretHost,
            host_path: "#0/doc".into(),
            list_path: "#0/doc/list".into(),
            row_index,
            row_key: key.into(),
            rel_path: None,
            landing: Landing::TopAtX(42.0),
            shift: true,
            goal_x: Some(42.0),
            scan_dir,
        }
    }

    #[test]
    fn request_scrolls_and_lands_once_mounted_and_selectable() {
        let mut st = State::Idle;
        let t = target(50, "r50", 1);
        let effs = step(&mut st, Event::Request(t.clone()));
        assert!(st.is_seeking());
        assert_eq!(effs.into_iter().collect::<Vec<_>>(), vec![Effect::ScrollAndMount {
            list_path: "#0/doc/list".into(),
            row_index: 50,
            row_key: "r50".into(),
            page_y: None,
            down: true,
        }]);

        let effs = step(&mut st, Event::Check {
            host_valid: true,
            current_index: Some(50),
            mounted: true,
            has_selectable: true,
            next_in_scan: None,
        });
        assert_eq!(st, State::Idle);
        assert_eq!(effs.into_iter().collect::<Vec<_>>(), vec![Effect::Land(t)]);
    }

    #[test]
    fn skips_non_selectable_row_and_cancels_on_deleted_row_or_hop_limit() {
        let mut st = State::Idle;
        let _ = step(&mut st, Event::Request(target(10, "r10", 1)));
        let effs = step(&mut st, Event::Check {
            host_valid: true,
            current_index: Some(10),
            mounted: true,
            has_selectable: false,
            next_in_scan: Some((11, "r11".into())),
        });
        assert!(st.is_seeking());
        assert_eq!(effs.into_iter().collect::<Vec<_>>(), vec![Effect::ScrollAndMount {
            list_path: "#0/doc/list".into(),
            row_index: 11,
            row_key: "r11".into(),
            page_y: None,
            down: true,
        }]);

        // Deleted row drops cleanly to Idle.
        let effs = step(&mut st, Event::Check {
            host_valid: true,
            current_index: None,
            mounted: false,
            has_selectable: false,
            next_in_scan: None,
        });
        assert_eq!(st, State::Idle);
        assert!(effs.is_empty());

        // Bounded hops never loop forever.
        let _ = step(&mut st, Event::Request(target(99, "r99", 1)));
        for _ in 0..=MAX_SEEK_HOPS {
            let _ = step(&mut st, Event::Check {
                host_valid: true,
                current_index: Some(99),
                mounted: false,
                has_selectable: false,
                next_in_scan: None,
            });
        }
        assert_eq!(st, State::Idle);
    }

    #[test]
    fn cancel_and_replacement_request_clear_prior_state() {
        let mut st = State::Idle;
        let _ = step(&mut st, Event::Request(target(5, "r5", 1)));
        let _ = step(&mut st, Event::Request(target(999, "r999", -1)));
        assert_eq!(st.target().map(|t| t.row_index), Some(999));
        let effs = step(&mut st, Event::Cancel);
        assert_eq!(st, State::Idle);
        assert!(effs.is_empty());
    }
}
