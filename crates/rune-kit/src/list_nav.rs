//! Shared vertical keyboard navigation and type-ahead helpers for list-like widgets
//! (`Combobox`, `Table`, `Tree`).

use rune::{Key, KeyEvent, Mods};

/// Seconds of pause after which typing starts a new type-ahead prefix (AppKit's type select).
const TYPE_PAUSE: f64 = 1.0;

/// Rows a Page Up / Page Down jumps across in a viewport of `viewport` points with `row_height`
/// rows: one less than a full screen so context stays visible, and at least one.
pub(crate) fn page_size(viewport: f32, row_height: f32) -> usize {
    ((viewport / row_height.max(1.0)).floor() as usize).saturating_sub(1).max(1)
}

/// Returns true when modifiers do not carry Alt, non-command Ctrl, or non-command Logo.
pub(crate) fn allows_list_mods(m: Mods) -> bool {
    !(m.alt || m.ctrl && !m.cmd || m.logo && !m.cmd)
}

/// A vertical index step within a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavStep {
    Up,
    Down,
    PageUp(usize),
    PageDown(usize),
    Home,
    End,
}

impl NavStep {
    /// Resolves the target row index in a list of `len` items given the current index `at`.
    /// Returns `None` when `len == 0`. When `wrap` is true, `Up` and `Down` loop around the
    /// ends; `PageUp`, `PageDown`, `Home` and `End` always stop at the ends.
    pub(crate) fn target(self, at: Option<usize>, len: usize, wrap: bool) -> Option<usize> {
        if len == 0 {
            return None;
        }
        Some(match self {
            NavStep::Up if wrap => (at.unwrap_or(0) as isize - 1).rem_euclid(len as isize) as usize,
            NavStep::Down if wrap => (at.unwrap_or(0) + 1) % len,
            NavStep::Up => at.map_or(len - 1, |r| r.saturating_sub(1)),
            NavStep::Down => at.map_or(0, |r| (r + 1).min(len - 1)),
            NavStep::Home => 0,
            NavStep::End => len - 1,
            NavStep::PageUp(page) => at.map_or(0, |r| r.saturating_sub(page)),
            NavStep::PageDown(page) => at.map_or(0, |r| r.saturating_add(page).min(len - 1)),
        })
    }
}

/// What a key event asks a list widget to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavAction {
    /// Move the cursor or highlight by `step`. `extend` is true when Shift is held.
    Move { step: NavStep, extend: bool },
    /// Select all rows (`Cmd+A`).
    SelectAll,
    /// Activate or choose the current row (`Enter`, or `Space` when enabled).
    Activate,
    /// Dismiss or collapse (`Escape`).
    Escape,
}

/// Keybinding rules for decoding a [`KeyEvent`] into a [`NavAction`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct NavPolicy {
    select_all: bool,
    space_activates: bool,
    search_field: bool,
    home_end: bool,
}

impl NavPolicy {
    /// Standalone focused list widget (`Table`, `Tree`): stops at the ends, Shift extends, and
    /// optional `Cmd+A` selects all.
    pub(crate) fn list(multi_select: bool) -> Self {
        Self { select_all: multi_select, space_activates: false, search_field: false, home_end: true }
    }

    /// Search-backed popup list (`Combobox`): bare Escape closes, Ctrl+N/P step like arrows,
    /// editor modifiers pass through, and Home/End jump only while `search_empty` is true.
    pub(crate) fn search_list(search_empty: bool) -> Self {
        Self { select_all: false, space_activates: false, search_field: true, home_end: search_empty }
    }

    /// Also treat Space as [`NavAction::Activate`].
    pub(crate) fn space_activates(mut self) -> Self {
        self.space_activates = true;
        self
    }

    /// Decodes `ev` into a list navigation action, or `None` if this key does not match.
    pub(crate) fn decode(self, ev: &KeyEvent, page: usize) -> Option<NavAction> {
        let m = ev.mods;
        if self.search_field {
            if ev.key == Key::Escape {
                return Some(NavAction::Escape);
            }
            if m.alt || m.logo {
                return None;
            }
            if ev.key == Key::Enter {
                return Some(NavAction::Activate);
            }
            let control = m.ctrl && !m.shift;
            let step = match &ev.key {
                Key::PageDown | Key::PageUp | Key::Home | Key::End if m.cmd || m.ctrl || m.shift => return None,
                Key::PageDown => NavStep::PageDown(page),
                Key::PageUp => NavStep::PageUp(page),
                Key::End if self.home_end => NavStep::End,
                Key::Home if self.home_end => NavStep::Home,
                Key::Down if !m.cmd && !m.ctrl => NavStep::Down,
                Key::Up if !m.cmd && !m.ctrl => NavStep::Up,
                Key::Text(c) if control && c == "n" => NavStep::Down,
                Key::Text(c) if control && c == "p" => NavStep::Up,
                Key::Command('n') if control => NavStep::Down,
                Key::Command('p') if control => NavStep::Up,
                _ => return None,
            };
            return Some(NavAction::Move { step, extend: false });
        }

        if !allows_list_mods(m) {
            return None;
        }
        if ev.is_command('a') && self.select_all {
            return Some(NavAction::SelectAll);
        }
        if m.cmd {
            return None;
        }
        let step = match &ev.key {
            Key::Up => NavStep::Up,
            Key::Down => NavStep::Down,
            Key::Home if self.home_end => NavStep::Home,
            Key::End if self.home_end => NavStep::End,
            Key::PageUp => NavStep::PageUp(page),
            Key::PageDown => NavStep::PageDown(page),
            Key::Enter => return Some(NavAction::Activate),
            Key::Text(t) if self.space_activates && t == " " => return Some(NavAction::Activate),
            Key::Escape => return Some(NavAction::Escape),
            _ => return None,
        };
        Some(NavAction::Move { step, extend: m.shift })
    }
}

/// Type-ahead prefix buffer: idle, or accumulating typed characters until a pause resets it.
#[derive(Default)]
pub(crate) enum TypeAhead {
    #[default]
    Idle,
    Typing { prefix: String, at: f64 },
}

impl TypeAhead {
    /// Adds `text`, typed at `now`, to the prefix: a key more than [`TYPE_PAUSE`] after the last
    /// starts a new name. Returns the lowercased prefix to jump to.
    pub(crate) fn step(&mut self, text: &str, now: f64) -> String {
        let paused = match self {
            TypeAhead::Typing { at, .. } => now - *at > TYPE_PAUSE,
            TypeAhead::Idle => true,
        };
        let mut prefix = match self {
            TypeAhead::Typing { prefix, .. } if !paused => std::mem::take(prefix),
            _ => String::new(),
        };
        prefix.push_str(&text.to_lowercase());
        *self = TypeAhead::Typing { prefix: prefix.clone(), at: now };
        prefix
    }
}

/// The row index typing `prefix` jumps to: the next item whose label starts with everything
/// typed since the last pause, and the same letter repeated cycles through items starting with it.
pub(crate) fn type_select<'a>(
    prefix: &str,
    at: Option<usize>,
    len: usize,
    label_at: impl Fn(usize) -> &'a str,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let first = prefix.chars().next()?;
    let cycle = prefix.chars().all(|c| c == first);
    let needle = if cycle { first.to_string() } else { prefix.to_owned() };
    // A longer name keeps matching the row it's on; a repeated letter moves on.
    let start = match at {
        Some(r) if cycle => r + 1,
        Some(r) => r,
        None => 0,
    };
    (0..len).map(|k| (start + k) % len).find(|&r| label_at(r).to_lowercase().starts_with(&needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent { key, mods }
    }

    #[test]
    fn page_size_leaves_one_context_row_and_at_least_one() {
        assert_eq!(page_size(0.0, 28.0), 1);
        assert_eq!(page_size(28.0, 28.0), 1);
        assert_eq!(page_size(192.0, 28.0), 5);
        assert_eq!(page_size(300.0, 28.0), 9);
    }

    #[test]
    fn nav_step_clamps_or_wraps_as_configured() {
        assert_eq!(NavStep::Up.target(None, 0, false), None);
        assert_eq!(NavStep::Up.target(None, 5, false), Some(4));
        assert_eq!(NavStep::Up.target(Some(0), 5, false), Some(0));
        assert_eq!(NavStep::Up.target(Some(0), 5, true), Some(4));
        assert_eq!(NavStep::Down.target(None, 5, false), Some(0));
        assert_eq!(NavStep::Down.target(Some(4), 5, false), Some(4));
        assert_eq!(NavStep::Down.target(Some(4), 5, true), Some(0));
        assert_eq!(NavStep::Home.target(Some(3), 5, true), Some(0));
        assert_eq!(NavStep::End.target(Some(1), 5, true), Some(4));
        assert_eq!(NavStep::PageDown(4).target(Some(2), 5, true), Some(4));
        assert_eq!(NavStep::PageDown(4).target(None, 5, false), Some(0));
        assert_eq!(NavStep::PageUp(4).target(Some(2), 5, true), Some(0));
    }

    #[test]
    fn list_policy_handles_shift_cmd_a_space_and_modifier_guards() {
        let multi = NavPolicy::list(true).space_activates();
        let plain = Mods::default();
        let shift = Mods { shift: true, ..plain };
        let cmd = Mods { cmd: true, ..plain };
        let alt = Mods { alt: true, ..plain };

        assert_eq!(multi.decode(&ev(Key::Down, shift), 5), Some(NavAction::Move { step: NavStep::Down, extend: true }));
        assert_eq!(multi.decode(&ev(Key::Command('a'), cmd), 5), Some(NavAction::SelectAll));
        assert_eq!(NavPolicy::list(false).decode(&ev(Key::Command('a'), cmd), 5), None);
        assert_eq!(multi.decode(&ev(Key::Text(" ".into()), plain), 5), Some(NavAction::Activate));
        assert_eq!(NavPolicy::list(false).decode(&ev(Key::Text(" ".into()), plain), 5), None);
        assert_eq!(multi.decode(&ev(Key::Down, alt), 5), None);
        assert_eq!(multi.decode(&ev(Key::Escape, plain), 5), Some(NavAction::Escape));
    }

    #[test]
    fn search_list_policy_handles_ctrl_n_p_and_home_end_gating() {
        let empty = NavPolicy::search_list(true);
        let filled = NavPolicy::search_list(false);
        let plain = Mods::default();
        let ctrl = Mods { ctrl: true, ..plain };
        let shift = Mods { shift: true, ..plain };
        let alt = Mods { alt: true, ..plain };

        assert_eq!(empty.decode(&ev(Key::Home, plain), 5), Some(NavAction::Move { step: NavStep::Home, extend: false }));
        assert_eq!(filled.decode(&ev(Key::Home, plain), 5), None);
        assert_eq!(empty.decode(&ev(Key::Home, shift), 5), None);
        assert_eq!(filled.decode(&ev(Key::Text("n".into()), ctrl), 5), Some(NavAction::Move { step: NavStep::Down, extend: false }));
        assert_eq!(filled.decode(&ev(Key::Command('p'), ctrl), 5), Some(NavAction::Move { step: NavStep::Up, extend: false }));
        assert_eq!(filled.decode(&ev(Key::Down, alt), 5), None);
        assert_eq!(filled.decode(&ev(Key::Escape, alt), 5), Some(NavAction::Escape));
    }

    #[test]
    fn type_ahead_accumulates_pauses_and_cycles_repeated_letters() {
        let rows = ["alpha", "beta", "bravo"];
        let mut ta = TypeAhead::default();
        let p = ta.step("b", 0.0);
        assert_eq!(type_select(&p, None, rows.len(), |i| rows[i]), Some(1));
        let p = ta.step("b", 0.1);
        assert_eq!(type_select(&p, Some(1), rows.len(), |i| rows[i]), Some(2));
        let p = ta.step("r", 0.2);
        assert_eq!(type_select(&p, Some(2), rows.len(), |i| rows[i]), None);
        let p = ta.step("b", 2.0);
        assert_eq!(type_select(&p, Some(2), rows.len(), |i| rows[i]), Some(1));
    }
}
