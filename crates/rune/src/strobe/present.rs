//! macOS: when each recorded frame reached the screen, from the Metal `presentedTime` hook a
//! window's layer carries (`present_time::metal`). Rows note their drawable; a run's end
//! fills `present_ns` once the times have arrived.

use std::sync::Arc;

use crate::frame::present_time::{Presented, Source, metal::Log};

/// How long a run's end waits for its last frames' present times.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

pub(crate) struct Presents {
    /// Each row's drawable, 0 for none.
    drawables: Box<[u64]>,
    /// Each surface's log, by surface index.
    logs: Vec<(usize, Arc<Log>)>,
    scratch: Vec<Presented>,
}

impl Presents {
    pub(crate) fn new(rows: usize) -> Self {
        Presents { drawables: vec![0; rows].into_boxed_slice(), logs: Vec::new(), scratch: Vec::new() }
    }

    /// Row `row` was pushed for a frame of surface `surface`, drawn into `drawable` of `log`.
    /// Between runs (`row` None) the times that came in are dropped, so the log stays small.
    pub(crate) fn note(&mut self, surface: usize, row: Option<usize>, drawable: Option<u64>, log: &Arc<Log>) {
        if !self.logs.iter().any(|(s, l)| *s == surface && Arc::ptr_eq(l, log)) {
            self.logs.retain(|(s, _)| *s != surface);
            self.logs.push((surface, log.clone()));
        }
        match row {
            Some(row) => self.drawables[row] = drawable.unwrap_or(0),
            None => { log.take(&mut self.scratch); self.scratch.clear(); }
        }
    }

    pub(crate) fn clear_row(&mut self, row: usize) { self.drawables[row] = 0; }

    /// Drops the times that came in before a run.
    pub(crate) fn discard(&mut self) {
        for (_, log) in &self.logs { log.take(&mut self.scratch); }
        self.scratch.clear();
    }

    /// Fills `rows`' `present_ns` with the host time each frame reached the screen, 0 when
    /// Metal skipped it or its time didn't come within `SETTLE`.
    pub(crate) fn fill(&mut self, rows: &mut [[u64; super::record::COLS]]) {
        let want: Vec<(usize, u64)> = self.logs.iter().map(|(s, _)| {
            let last = rows.iter().enumerate().filter(|(_, r)| r[1] as usize == *s).map(|(i, _)| self.drawables[i]).max().unwrap_or(0);
            (*s, last)
        }).collect();
        let mut got: Vec<(usize, u64, Option<crate::time::Instant>)> = Vec::new();
        let until = crate::time::Instant::now() + SETTLE;
        loop {
            for (s, log) in &self.logs {
                log.take(&mut self.scratch);
                got.extend(self.scratch.drain(..).map(|(seq, at)| (*s, seq, at)));
            }
            let settled = want.iter().all(|&(s, last)| last == 0 || got.iter().any(|&(gs, seq, _)| gs == s && seq >= last));
            if settled || crate::time::Instant::now() >= until { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        got.sort_by_key(|&(s, seq, _)| (s, seq));
        let (now, now_ns) = (crate::time::Instant::now(), super::clock::now_ns());
        for (row, &drawable) in rows.iter_mut().zip(self.drawables.iter()) {
            if drawable == 0 { continue; }
            let key = (row[1] as usize, drawable);
            if let Ok(i) = got.binary_search_by_key(&key, |&(s, seq, _)| (s, seq)) && let Some(at) = got[i].2 {
                row[6] = match now.checked_duration_since(at) {
                    Some(ago) => now_ns.saturating_sub(ago.as_nanos() as u64),
                    None => now_ns + at.duration_since(now).as_nanos() as u64,
                };
            }
        }
    }
}
