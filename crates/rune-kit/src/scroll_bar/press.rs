//! A press on a scroll bar: the thumb dragged, or the track paged toward the pointer, held from
//! the press to its release, or until the release is found lost.

use rune::{Phase, PointerEvent};

use super::{Bar, PAGE_DELAY, PAGE_EVERY, offset_along, thumb_along, track_len};
use crate::grip::Grip;

/// What a press on the bar holds: the thumb, caught `grab` points below its top, or the track,
/// paging toward the pointer (-1 up, 1 down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum BarPress {
    Thumb { grab: f32 },
    Pages { toward: i32 },
}

impl Bar {
    /// Whether the thumb or the track is pressed: the thumb is wide and an overlay bar holds.
    pub(super) fn pressed(&self) -> bool {
        self.press.get().is_held()
    }

    /// Runs `step` on the press, keeping what it leaves.
    fn step<R>(&self, step: impl FnOnce(&mut Grip<BarPress>) -> R) -> R {
        let mut press = self.press.get();
        let out = step(&mut press);
        self.press.set(press);
        out
    }

    /// A press ended (or was lost): the thumb narrows and an overlay bar may fade.
    fn released(&self) {
        self.widen();
        self.let_go();
    }

    /// The pointer moved to `p` in the bar's area. A press whose drag never hears the move lost
    /// its release (the bar unmounted, or a modal took the pointer): it ends here.
    pub(super) fn pointer(&self, p: PointerEvent) {
        if self.step(|g| g.pointer([p.window_x, p.window_y])).is_some() {
            self.released();
        }
        self.point(Some(p));
    }

    /// The thumb or track pressed, dragged or released; `target` is the viewport's path, and
    /// `jumps` says a track press jumps to the spot rather than paging toward it.
    pub(super) fn drag(&self, target: &str, jumps: bool, d: &mut rune::Drag, tx: &mut rune::Tx) {
        tx.live_only();
        let at = self.pos.get();
        let t = self.style().track;
        let (len, shows) = track_len(at, &t);
        if !shows {
            if self.step(Grip::release).is_some() {
                self.widen();
            }
            return;
        }
        let a = t.along();
        let (top, h) = thumb_along(at, len, t.min_thumb, a, t.pixels);
        // Down a vertical bar, or along a horizontal one from its start, its right edge right to
        // left.
        let y = match (t.x, self.rtl()) {
            (false, _) => d.y,
            (true, false) => d.x,
            (true, true) => len - d.x,
        };
        let press = match d.phase {
            Phase::Down => {
                let on_thumb = (top..=top + h).contains(&y);
                // A track press pages toward itself, or with Option (or the preference) jumps
                // there. Pages run down, so a horizontal bar jumps. A press on the thumb holds it
                // where it was caught; a jump centres it there.
                let press = if !on_thumb && jumps == tx.mods().alt && !t.x {
                    BarPress::Pages { toward: if y < top { -1 } else { 1 } }
                } else {
                    BarPress::Thumb { grab: if on_thumb { y - top } else { h / 2.0 } }
                };
                self.step(|g| g.press(press, Some(d.window_point())));
                self.hold();
                self.widen();
                if let BarPress::Pages { toward } = press {
                    tx.scroll_page(target, toward);
                    tx.drag_repeat_after(PAGE_DELAY);
                    return;
                }
                press
            }
            Phase::Up => {
                if self.step(Grip::release).is_some() {
                    self.released();
                }
                return;
            }
            Phase::Move => match self.step(|g| g.moved(d.window_point()).copied()) {
                Some(press) => press,
                None => return,
            },
        };
        let grab = match press {
            BarPress::Thumb { grab } => grab,
            BarPress::Pages { toward } => {
                // Paging goes on while held, until the thumb reaches the pointer.
                if tx.drag_repeat() {
                    tx.drag_repeat_after(PAGE_EVERY);
                    if (toward < 0 && y < top) || (toward > 0 && y > top + h) {
                        tx.scroll_page(target, toward);
                    }
                }
                return;
            }
        };
        let to = offset_along(at, len, y - grab, t.min_thumb, a, t.pixels);
        if t.x {
            tx.scroll_to_instant(target, to, at.offset[1]);
        } else {
            tx.scroll_to_instant(target, at.offset[0], to);
        }
    }
}
