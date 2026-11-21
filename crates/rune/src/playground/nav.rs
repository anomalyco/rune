//! The open playground's keys.

use super::*;

impl LiveCore {
    /// What `ev` does in the open playground, with the keyboard in `region`. The stage's keys
    /// are its own, so it only gets here from the list or the side column.
    pub(crate) fn nav(&self, ev: &KeyEvent, region: Region) -> Nav {
        let m = ev.mods;
        // Control on macOS, the Windows key elsewhere: never the playground's.
        let other = m.alt || (cfg!(target_os = "macos") && m.ctrl) || (cfg!(not(target_os = "macos")) && m.logo);
        if !self.is_open() || other || region == Region::Stage { return Nav::Pass; }
        let letter = match &ev.key {
            Key::Text(t) if !m.cmd => t.chars().next().filter(|_| t.chars().count() == 1).map(|c| c.to_ascii_lowercase()),
            _ => None,
        };
        let in_side = region == Region::Side;
        if in_side {
            match ev.key {
                Key::Tab | Key::Escape | Key::Left => return Nav::OutOfSide,
                Key::Up if !m.cmd => return Nav::SideStep(-1),
                Key::Down if !m.cmd => return Nav::SideStep(1),
                _ => {}
            }
            // Digits pick a variant only here, where the keyboard is on the story's controls.
            if let Some(request) = letter.and_then(|c| self.variant_key(c)) { return Nav::Request(request); }
        }
        // In the side column, Space presses the focused control.
        if let Some(c) = letter && !(in_side && c == ' ') && let Some(request) = self.trigger_request(c) {
            return Nav::Request(request);
        }
        if in_side { return Nav::Pass; }
        if letter == Some('/') { return Nav::Filter; }
        match self.list_nav(ev, letter) {
            // The list's arrows and Return, even where they do nothing. Every other key it
            // leaves, ⌘-chords and digits included, is the stage's.
            Nav::Pass if !m.cmd && matches!(ev.key, Key::Up | Key::Down | Key::Left | Key::Right | Key::Enter) => Nav::Stay,
            nav => nav,
        }
    }

    /// Keys in the story list: along it, into and out of groups, and on to the other columns.
    fn list_nav(&self, ev: &KeyEvent, letter: Option<char>) -> Nav {
        let m = ev.mods;
        match ev.key {
            Key::Tab if m.shift => return Nav::IntoStage,
            Key::Tab => return Nav::IntoSide,
            _ => {}
        }
        let rows = self.rows();
        let is_story = |r: &Row| matches!(r, Row::Story(..));
        let Some(&last) = rows.iter().rev().find(|r| is_story(r)) else { return Nav::Pass };
        let at = rows.iter().position(|r| match (*r, self.header) {
            (Row::Header(g, ..), Some(h)) => g == h,
            (Row::Story(i, _), None) => i == self.selected,
            _ => false,
        });
        let n = rows.len();
        let first = rows.iter().position(is_story).unwrap_or(0);
        let to = match (&ev.key, letter, self.header) {
            // Filtering, ↑ from the first match goes back up to the field.
            (Key::Up, _, _) if !m.cmd && !self.query().is_empty() && at.is_none_or(|a| a <= first) => return Nav::Filter,
            (Key::Up, _, _) if m.cmd => rows.iter().copied().find(is_story).unwrap_or(last),
            (Key::Down, _, _) if m.cmd => last,
            (_, _, _) if m.cmd => return Nav::Pass,
            (Key::Up, _, _) => rows[at.map_or(0, |a| (a + n - 1) % n)],
            (Key::Down, _, _) => rows[at.map_or(0, |a| (a + 1) % n)],
            // J and K skip the headers.
            (_, Some(c @ ('j' | 'k')), _) => {
                let step = if c == 'j' { 1 } else { n - 1 };
                let from = at.unwrap_or(0);
                (1..=n).map(|k| rows[(from + k * step) % n]).find(is_story).unwrap_or(last)
            }
            // ← folds an open group, else goes up to the group around.
            (Key::Left, _, Some(g)) if !self.is_folded(g) && self.query().is_empty() => return Nav::Request(Request::Fold(g, Some(true))),
            (Key::Left, _, Some(g)) => match Self::parent(g) {
                Some(up) => return Nav::Request(Request::Header(up)),
                None => return Nav::Pass,
            },
            (Key::Left, _, None) if self.selected < self.count() => return Nav::Request(Request::Header(self.group(self.selected))),
            // → opens a folded group, else steps into it.
            (Key::Right, _, Some(g)) if !m.shift && self.is_folded(g) => return Nav::Request(Request::Fold(g, Some(false))),
            (Key::Right, _, Some(_)) if !m.shift => match at.and_then(|a| rows.get(a + 1)) {
                Some(&next) if !matches!(next, Row::Header(_, _, d) if d == 0) => next,
                _ => return Nav::Pass,
            },
            (Key::Enter, _, Some(g)) => return Nav::Request(Request::Fold(g, None)),
            (Key::Right, _, None) if !m.shift => return Nav::IntoSide,
            _ => return Nav::Pass,
        };
        Nav::Request(match to {
            Row::Header(g, ..) => Request::Header(g),
            Row::Story(i, _) => Request::Select(i),
        })
    }

    /// Whether the open story's side controls take keys: `Story::side_keys`, or
    /// `Playground::side_keys` for a scene.
    pub(super) fn side_keys(&self) -> bool {
        self.open && self.stories.get(self.selected).is_some_and(|s| s.side_keys)
    }

    /// Keys while the keyboard is on an own-state story's side controls that take keys.
    pub(super) fn side_key_own(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let took = self.own_slot_mut(self.selected)
            .and_then(|own| Some((own.story.as_ref(), own.stage.as_mut()?)))
            .is_some_and(|(story, stage)| story.side_key(stage.as_mut(), ev, tx));
        took || ev.key == Key::Left
    }

    /// What the open story's trigger on `c` asks for, as its letter does: R replays.
    pub(super) fn trigger_request(&self, c: char) -> Option<Request> {
        self.triggers().iter().any(|(k, _)| *k == c).then_some(if c == 'r' { Request::Replay } else { Request::Trigger(c) })
    }

    /// [`Self::trigger_request`] for tests, panicking with the story's triggers when missing.
    pub(super) fn require_trigger(&self, key: char) -> Request {
        assert!(self.open, "the playground isn't open: call Runtime::open_playground first");
        self.trigger_request(key).unwrap_or_else(|| {
            let keys: Vec<String> = self.triggers().iter().map(|(k, label)| format!("{k:?} ({label})")).collect();
            panic!("{:?} has no trigger on {key:?}; it has: {}", self.path(self.selected), keys.join(", "));
        })
    }
}

impl<S: 'static> Live<S> {
    /// Keys while the keyboard is on the open story's side controls that take keys: true when
    /// the story's hook took `ev`, or it's ←, which stays rather than leaving.
    pub(crate) fn side_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.core.open { return false; }
        if let Some(scene) = &mut self.stage.scene {
            let key = self.stage.sides.iter().find_map(|(s, side)| match side {
                Side::Keys(_, key) if *s == self.core.selected => Some(key),
                _ => None,
            });
            return key.is_some_and(|key| key(&mut scene.state, ev, tx)) || ev.key == Key::Left;
        }
        self.core.side_key_own(ev, tx)
    }
}
