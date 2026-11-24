//! `El::on_mount`: firing mount handlers in the same frame before drawing, with bounded
//! follow-up commits so state changes made on mount render without a one-frame lag.

use super::*;

/// Maximum chained `El::on_mount` follow-up rounds in a single frame.
pub(in crate::runtime) const MAX_MOUNT_ROUNDS: usize = 4;

impl RuntimeCore {
    /// Whether any live, non-exiting, non-unmounted element with `El::on_mount` has not fired yet
    /// (for example after hitting [`MAX_MOUNT_ROUNDS`] in a chained mount).
    pub(in crate::runtime) fn mounts_pending(&self) -> bool {
        if !self.tree.has_mount || self.commit.mounted_gen() == self.commit.commit_gen() {
            return false;
        }
        self.tree.nodes.iter().enumerate().any(|(i, n)| {
            n.handlers.get().is_some_and(|h| h.on_mount.is_some())
                && !self.subtree_exiting(i)
                && !self.subtree_unmounted(i)
                && !self.commit.is_mount_seen(n.id)
        })
    }

    /// Fires `El::on_mount` for newly mounted live elements in tree order and re-commits their
    /// state changes in the same frame before scrolling and painting, bounded to `max_rounds`
    /// (at most [`MAX_MOUNT_ROUNDS`] across a frame). Returns the number of rounds used.
    pub(in crate::runtime) fn deliver_mounts(
        &mut self,
        app: &mut dyn AppDriver,
        tx: &mut Tx,
        commits0: u64,
        open_urls: &mut Vec<crate::open_url::Link>,
        open_paths: &mut Vec<crate::open_path::PathRequest>,
        max_rounds: usize,
    ) -> usize {
        if max_rounds == 0 || (!self.tree.has_mount && self.commit.mounted_seen_empty()) {
            return 0;
        }
        if self.commit.mounted_gen() == self.commit.commit_gen() {
            return 0;
        }
        let mut rounds_used = 0usize;
        for _ in 0..max_rounds {
            let prev = self.commit.take_mounted_seen();
            let mut live_seen = HashSet::with_capacity_and_hasher(prev.len(), Default::default());
            let mut to_fire = Vec::new();
            if self.tree.has_mount {
                for (i, n) in self.tree.nodes.iter().enumerate() {
                    let Some(f) = n.handlers.get().and_then(|h| h.on_mount.clone()) else {
                        continue;
                    };
                    if self.subtree_exiting(i) || self.subtree_unmounted(i) {
                        continue;
                    }
                    live_seen.insert(n.id);
                    if !prev.contains(&n.id) {
                        to_fire.push(f);
                    }
                }
            }
            self.commit.set_mounted_seen(live_seen);
            if to_fire.is_empty() {
                self.commit.set_mounted_gen(self.commit.commit_gen());
                return rounds_used;
            }
            rounds_used += 1;
            let mut round_tx = Tx::default();
            let mut dirty = false;
            for f in to_fire {
                let mut one = self.transaction(app);
                f(app.states(), &mut one);
                if std::mem::take(&mut one.live_only) {
                    one.commit = Commit::Animate;
                    one.motion = None;
                    one.follow_edge = false;
                    one.color_motion = None;
                } else {
                    dirty = true;
                }
                merge(&mut round_tx, one);
            }
            open_urls.extend(std::mem::take(&mut round_tx.open_urls));
            open_paths.extend(std::mem::take(&mut round_tx.open_paths));
            self.latch_requests(app, &mut round_tx);
            self.schedule_wakes(round_tx.wakes.drain(..));

            let needs_commit = dirty
                || self.commit.dirty
                || self.commit.relayout
                || self.kbd.focus_request.is_some();
            if needs_commit {
                self.capture_keeps(&mut round_tx);
            }
            let commit_mode = round_tx.commit;
            let asked_motion = std::mem::take(&mut round_tx.asked_motion);
            let motion = round_tx.motion.take();
            let color_motion = round_tx.color_motion.take();
            let follow_edge = std::mem::take(&mut round_tx.follow_edge);
            let unclipped_flights = std::mem::take(&mut round_tx.unclipped_flights);
            let live_growth = std::mem::take(&mut round_tx.live_growth);
            let tail_rooms = std::mem::take(&mut round_tx.tail_rooms);
            let text_flights = std::mem::take(&mut round_tx.text_flights);
            let launches = std::mem::take(&mut round_tx.launches);
            merge(tx, round_tx);

            if !needs_commit {
                self.commit.set_mounted_gen(self.commit.commit_gen());
                return rounds_used;
            }

            let commit = if self.settled {
                Commit::Settle
            } else if commits0 == 0 || tx.commit.snaps() || tx.unprompted || !asked_motion {
                Commit::Instant.merge(commit_mode)
            } else {
                tx.commit.merge(commit_mode)
            };
            self.settled |= commit.lands();
            let mut follow_tx = Tx {
                commit,
                unprompted: tx.unprompted && !asked_motion,
                asked_motion,
                motion,
                color_motion,
                follow_edge,
                unclipped_flights,
                live_growth,
                tail_rooms,
                text_flights,
                launches,
                ..Tx::default()
            };
            if self.tree.built && (!follow_tx.text_flights.is_empty() || !follow_tx.launches.is_empty()) {
                self.motion.capture_flights(
                    &self.tree.nodes,
                    &mut self.paint,
                    &mut self.text,
                    self.env.scale,
                    self.viewport,
                    self.t,
                    &mut follow_tx,
                );
            }
            self.commit.set_state_unchanged(false);
            let why = CommitWhy::DIRTY | if self.settled { CommitWhy::SETTLE } else { 0 };
            self.commit_for(app, &follow_tx, why);
            self.apply_keeps(app);
        }
        self.prune_mounted_seen_after_cap();
        rounds_used
    }

    /// After exhausting `max_rounds`, removes any node ids that are no longer live with
    /// `El::on_mount` without marking newly mounted (un-fired) nodes as seen. If no un-fired
    /// live mounts remain, marks the current commit generation synchronized.
    fn prune_mounted_seen_after_cap(&mut self) {
        let prev = self.commit.take_mounted_seen();
        let mut kept = HashSet::with_capacity_and_hasher(prev.len(), Default::default());
        let mut any_unfired = false;
        if self.tree.has_mount {
            for (i, n) in self.tree.nodes.iter().enumerate() {
                if n.handlers.get().is_none_or(|h| h.on_mount.is_none()) {
                    continue;
                }
                if self.subtree_exiting(i) || self.subtree_unmounted(i) {
                    continue;
                }
                if prev.contains(&n.id) {
                    kept.insert(n.id);
                } else {
                    any_unfired = true;
                }
            }
        }
        self.commit.set_mounted_seen(kept);
        if !any_unfired {
            self.commit.set_mounted_gen(self.commit.commit_gen());
        }
    }
}

#[cfg(test)]
#[path = "mount_tests.rs"]
mod tests;
