//! The runtime's side of `perf_warn`'s commit watch: why each of a frame's commits ran, for the
//! frame log and `RUNE_COMMIT_BUDGET_MS`, and what the commit warnings hear once a frame.
use super::*;

impl CommitState {
    #[inline]
    pub(super) fn begin_watch(&mut self, why: u16) {
        self.commit_why |= why;
        if crate::tooling::interaction::on() { self.watch.interaction.committed(why); }
        self.watch.cause = why;
    }

    #[inline]
    pub(super) fn end_watch(&mut self) {
        self.watch.cause = 0;
    }

    /// A committing measure pass ran the app's view (as opposed to a pure relayout or
    /// virtual-row remount).
    #[cfg(debug_assertions)]
    #[inline]
    pub(super) fn note_view_commit(&mut self) {
        self.watch.meter.view_commit();
    }

    /// View-running commits completed so far.
    #[cfg(debug_assertions)]
    #[inline]
    pub(in crate::runtime) fn view_commits(&self) -> u64 {
        self.watch.meter.view_commits()
    }

    /// After a frame's commits, scroll requests and same-frame visible-rows handlers: whether the
    /// view ran since `view_commits0`, while a viewport scrolled; `instant` is the viewport an
    /// instant scroll it asked for moves.
    #[cfg(debug_assertions)]
    pub(in crate::runtime) fn watch_frame(&mut self, view_commits0: u64, scrolling: bool, instant: Option<&str>) {
        self.watch.meter.frame_ends(scrolling, self.watch.meter.view_commits() != view_commits0, instant);
    }
}
