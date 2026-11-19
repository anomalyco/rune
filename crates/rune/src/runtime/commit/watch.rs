//! The runtime's side of `perf_warn`'s commit watch: why each of a frame's commits ran, for the
//! frame log and `RUNE_COMMIT_BUDGET_MS`, and what the commit warnings hear once a frame.
use super::*;

impl RuntimeCore {
    /// A frame's commit for `why` (`CommitWhy` bits).
    pub(in crate::runtime) fn commit_for(&mut self, app: &mut dyn AppDriver, tx: &Tx, why: u16) {
        self.commit.commit_why |= why;
        if crate::tooling::interaction::on() { self.commit.watch.interaction.committed(why); }
        self.commit.watch.cause = why;
        self.commit(app, tx);
        self.commit.watch.cause = 0;
    }

    /// After a frame's commits, before its scroll requests run: whether it committed since the
    /// `commits0`th commit, while a viewport scrolled; `instant` is the viewport an instant scroll
    /// it asked for moves.
    #[cfg(debug_assertions)]
    pub(in crate::runtime) fn watch_frame(&mut self, commits0: u64, instant: Option<&str>) {
        let scrolling = self.scrolling();
        self.commit.watch.meter.frame_ends(scrolling, self.commit.commits != commits0, instant);
    }
}
