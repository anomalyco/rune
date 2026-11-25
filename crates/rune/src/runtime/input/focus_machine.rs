//! The focus machine: which element has keyboard focus, how focus shows, and where Tab starts
//! while nothing has it. `step` decides; `Runtime::set_focus` and the input routes feed it and
//! carry out what it returns (the IME, links and the inputs' own focus flags follow a move).
//!
//! The pieces it keeps apart, as a browser does:
//! - [`Focus`]: nothing focused, or one element. Only an element the pointer lit as a
//!   `focus_highlight` row is `hovered`, and only while nothing is focused is there a starting
//!   point for Tab (`from`) or a focus a press just took away (`blurred`).
//! - [`Modality`]: whether focus shows (CSS `:focus-visible`'s heuristic). It outlives a blur,
//!   so focus a closing modal hands back shows as it did.
//! - [`TypeSelect`]: a list's typed prefix, which a pause ends.
use crate::machine::Effects;

/// Whether the keyboard drove focus last: then a focused control shows its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Modality {
    #[default]
    Pointer,
    Keyboard,
}

/// How focus came to the element that has it: by the keys (a Tab, an arrow in a group, `Tx::focus`
/// with a ring), or otherwise (a press, a hand-over without a ring, nothing focused). A field's
/// keyboard-entry ring follows it, not [`Modality`]: an arrow typed in a field a click focused
/// doesn't make it the keys' (`El::focus_visible_within_ring`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Entry {
    #[default]
    Other,
    Keys,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    /// Nothing has focus. Tab starts from `from` (the element last focused, or pressed without
    /// taking focus, as a browser's sequential focus navigation starting point). `blurred` is
    /// the focus the gesture or commit underway took away: a scrim that blurs on pointer-down
    /// and closes its overlay on pointer-up still returns focus through `El::focus_return`.
    /// `handoff` is whether that focus may go to a neighbour once its element leaves the tree
    /// (the user's Escape or a press): `Tx::blur` says nothing has focus, so it hands nothing on.
    None { from: Option<String>, blurred: Option<String>, handoff: bool },
    /// `path` has focus. `hovered`: the pointer lit it as a `focus_highlight` row, so its
    /// highlight shows without a key. `from`: a press on content that takes no focus queued a
    /// blur, and Tab starts there once it lands.
    On { path: String, hovered: bool, from: Option<String> },
}

impl Default for Focus {
    fn default() -> Self { Self::None { from: None, blurred: None, handoff: false } }
}

pub(crate) enum FocusEvent {
    /// Focus moves to `path`, or nowhere, and shows as `keyboard` says. When `keep_entry` is
    /// set (a commit keeping focus on the same element without an explicit `Tx::focus`), [`Entry`]
    /// stays as it arrived so an in-field arrow key or window return doesn't light a field's ring.
    Set { path: Option<String>, keyboard: bool, keep_entry: bool },
    /// The pointer lit (`true`) or left (`false`) the focused `focus_highlight` row.
    Hover(bool),
    /// A key that navigates (Tab, an arrow): focus shows from now on.
    Navigated,
    /// The window came to the front again with focus where it was: that isn't the keys moving
    /// focus there.
    WindowReturned,
    /// A pointer press, or a key on a highlight the user couldn't see: focus stops showing.
    Pointed,
    /// A press on content that takes no focus: Tab goes on from there.
    StartAt(String),
    /// `Tx::blur`: the app dropped focus itself, so nothing hands the keyboard on when the
    /// element it left goes too, unlike Escape or a press that blurred it.
    Dropped,
    /// A commit settled with no press held, or `Tx::blur`: a blur it carried is final.
    Settled,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FocusEffect {
    /// Focus left the element it was on (or arrived from nowhere): the IME, links and the
    /// selection's keyboard follow.
    Moved,
}

/// The whole focus state the runtime keeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FocusState {
    pub(crate) focus: Focus,
    pub(crate) modality: Modality,
    pub(crate) entry: Entry,
    /// Per landmark (by path), the element focused last inside it, most recent first: where F6
    /// lands when it comes back to that pane.
    scopes: Vec<(String, String)>,
}

/// How many panes F6 remembers a focus for.
const SCOPES: usize = 16;

impl FocusState {
    pub(crate) fn path(&self) -> Option<&str> {
        match &self.focus {
            Focus::On { path, .. } => Some(path),
            Focus::None { .. } => None,
        }
    }

    pub(crate) fn owned(&self) -> Option<String> { self.path().map(str::to_owned) }

    pub(crate) fn is(&self, path: &str) -> bool { self.path() == Some(path) }

    pub(crate) fn keyboard(&self) -> bool { self.modality == Modality::Keyboard }

    /// The keys put focus where it is.
    pub(crate) fn entered_by_keys(&self) -> bool { self.entry == Entry::Keys }

    /// The pointer lit the focused row.
    pub(crate) fn hovered(&self) -> bool { matches!(self.focus, Focus::On { hovered: true, .. }) }

    /// Focus shows, by the keyboard or the pointer's light on a row.
    pub(crate) fn shown(&self) -> bool { self.keyboard() || self.hovered() }

    /// Notes that `path` has focus inside landmark `scope`.
    pub(crate) fn remember_scope(&mut self, scope: String, path: String) {
        if self.scopes.first().is_some_and(|(s, p)| *s == scope && *p == path) { return; }
        self.scopes.retain(|(s, _)| *s != scope);
        self.scopes.insert(0, (scope, path));
        self.scopes.truncate(SCOPES);
    }

    /// The element focused last inside landmark `scope`.
    pub(crate) fn scope_memory(&self, scope: &str) -> Option<&str> {
        self.scopes.iter().find(|(s, _)| s == scope).map(|(_, p)| p.as_str())
    }

    /// Where Tab starts while nothing is focused.
    pub(crate) fn tab_start(&self) -> Option<&str> {
        match &self.focus {
            Focus::None { from, .. } => from.as_deref(),
            Focus::On { .. } => None,
        }
    }

    /// Focus now, else the focus the gesture underway took away.
    pub(crate) fn now_or_blurred(&self) -> Option<&str> {
        match &self.focus {
            Focus::On { path, .. } => Some(path),
            Focus::None { blurred, .. } => blurred.as_deref(),
        }
    }

    /// Whether focus the element lost may move to a neighbour once it leaves the tree.
    pub(crate) fn handoff(&self) -> bool {
        matches!(self.focus, Focus::None { handoff: true, .. })
    }

    pub(crate) fn step(&mut self, event: FocusEvent) -> Effects<FocusEffect> {
        match event {
            FocusEvent::Set { path, keyboard, keep_entry } => {
                self.modality = if keyboard { Modality::Keyboard } else { Modality::Pointer };
                if !keep_entry {
                    self.entry = if keyboard && path.is_some() { Entry::Keys } else { Entry::Other };
                }
                let moved = self.path() != path.as_deref();
                self.focus = match (std::mem::take(&mut self.focus), path) {
                    (Focus::On { path: old, hovered, .. }, Some(path)) if old == path => Focus::On { path, hovered, from: None },
                    (_, Some(path)) => Focus::On { path, hovered: false, from: None },
                    (Focus::On { path: old, from, .. }, None) => Focus::None { from: from.or_else(|| Some(old.clone())), blurred: Some(old), handoff: true },
                    (none @ Focus::None { .. }, None) => none,
                };
                if moved { Effects::one(FocusEffect::Moved) } else { Effects::none() }
            }
            FocusEvent::Hover(lit) => {
                if let Focus::On { hovered, .. } = &mut self.focus { *hovered = lit; }
                Effects::none()
            }
            FocusEvent::Navigated => {
                self.modality = Modality::Keyboard;
                Effects::none()
            }
            FocusEvent::WindowReturned => {
                self.entry = Entry::Other;
                Effects::none()
            }
            FocusEvent::Pointed => {
                self.modality = Modality::Pointer;
                Effects::none()
            }
            FocusEvent::StartAt(path) => {
                match &mut self.focus {
                    Focus::None { from, .. } | Focus::On { from, .. } => *from = Some(path),
                }
                Effects::none()
            }
            FocusEvent::Dropped => {
                if let Focus::None { handoff, .. } = &mut self.focus { *handoff = false; }
                Effects::none()
            }
            FocusEvent::Settled => {
                if let Focus::None { blurred, .. } = &mut self.focus { *blurred = None; }
                Effects::none()
            }
        }
    }

    /// Sets focus aside for a key the playground's stage presses, without a transition: nothing
    /// moves, and [`Self::put_back`] restores it if nothing took focus meanwhile.
    pub(crate) fn set_aside(&mut self) -> Option<Focus> {
        let Focus::On { path, from, .. } = &self.focus else { return None };
        let rest = Focus::None { from: from.clone(), blurred: Some(path.clone()), handoff: false };
        Some(std::mem::replace(&mut self.focus, rest))
    }

    pub(crate) fn put_back(&mut self, aside: Focus) {
        if matches!(self.focus, Focus::None { .. }) { self.focus = aside; }
    }
}

/// How long a list waits for the next typed letter before its prefix starts over (AppKit's).
pub(crate) const TYPE_SELECT_PAUSE: f64 = 1.0;

/// A pending programmatic focus change from `Tx::focus*` or `Tx::blur`, resolved in
/// `sync_requested_focus` once the new view is built so stale input accessors never run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum FocusRequest {
    Blur,
    Focus(String, bool, crate::el::FocusAsk),
}

/// A list's type-select: nothing typed, or a prefix and when its last letter arrived.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum TypeSelect {
    #[default]
    Idle,
    Typing { prefix: String, at: f64 },
}

impl TypeSelect {
    /// The prefix after `typed` arrives at `now`: it starts over after a pause.
    pub(crate) fn typed(&mut self, typed: &str, now: f64) -> &str {
        let prefix = match std::mem::take(self) {
            Self::Typing { prefix, at } if now - at <= TYPE_SELECT_PAUSE => prefix,
            _ => String::new(),
        };
        *self = Self::Typing { prefix: prefix + &typed.to_lowercase(), at: now };
        match self {
            Self::Typing { prefix, .. } => prefix,
            Self::Idle => "",
        }
    }

    /// A key that moves by position (an arrow, Home, End) ends the prefix.
    pub(crate) fn reset(&mut self) { *self = Self::Idle; }
}

/// Keyboard focus, modal focus/scope, type-select, held keys and modifier tracking owned by
/// [`crate::runtime::RuntimeCore`]. Ordered hot-first under `#[repr(C)]` so `modal_scope`
/// (checked per interactive node in `register_node_hits`) sits at offset 0.
#[repr(C)]
pub(crate) struct KeyboardState {
    /// The innermost open modal this commit shows and what input may reach inside it, worked
    /// out from the nodes at each commit; none while no modal shows.
    pub(in crate::runtime) modal_scope: Option<crate::runtime::focus::ModalScope>,
    /// What has focus, how it shows and where Tab starts (`runtime/input/focus_machine.rs`).
    pub(crate) focus: FocusState,
    /// Type-select's typed prefix and when its last key arrived (`now_clock`).
    type_select: TypeSelect,
    /// The activation key (Enter, Escape, Space) held down and the focus it was first pressed
    /// on. Its repeats are dropped once focus has moved, so a held key that closed something
    /// can't act again on whatever took focus next.
    pub(in crate::runtime) held: crate::runtime::physical_dispatch::Activation,
    /// Modifier keys down, for `Key::Modifier` (off unless the app opts in).
    pub(super) modifier_keys: crate::runtime::modifier_keys::ModifierKeys,
    /// Key releases an input method takes, and the focus where the held non-IME key(s) went
    /// down, so a release after focus moves still reaches that element first.
    key_releases: crate::runtime::key_releases::KeyReleases,
    /// The open modals and where focus returns as each closes (`modal_focus`).
    pub(super) modal_focus: crate::runtime::modal_focus::ModalFocus,
    /// Focus when the running commit began, before a new view's focus request applied: what a
    /// modal that opens in this commit returns focus to.
    pub(in crate::runtime) focus_before_commit: Option<String>,
    /// `Tx::focus*` or `Tx::blur`: the focus change the next commit applies.
    pub(in crate::runtime) focus_request: Option<FocusRequest>,
    /// `Tx::focus_when_shown` still waiting for its element: the key and whether its ring shows.
    pub(in crate::runtime) focus_owed: Option<(String, bool)>,
    /// Focused text fields that unmounted without being blurred: while another field holds focus,
    /// remounting one with its old `focused = true` flag does not steal focus back.
    pub(super) unmounted_focused: Vec<std::sync::Weak<crate::edit::History>>,
    /// The modifiers `App::on_modifiers` last heard.
    mods_heard: crate::Mods,
    /// Modifier keys held now, as the window last reported them (`Tx::mods`).
    pub(in crate::runtime) held_mods: crate::Mods,
    /// `keys_lost` queued its `FocusLost` since the window last had focus.
    pub(in crate::runtime) keys_lost: bool,
    /// Tab visits every control; off, only text fields and lists (macOS Full Keyboard
    /// Access off). Native hosts sample the system setting; headless runtimes default on.
    pub(in crate::runtime) full_keyboard_access: bool,
    /// `App::tab_visits_controls`: Tab visits every control whatever `full_keyboard_access` says.
    pub(in crate::runtime) tab_visits_controls: bool,
    /// `App::media_keys`: deliver hardware media transport keys (`Key::Media`). Off by default.
    pub(in crate::runtime) media_keys: bool,
    /// This commit's focus is a `Tx::return_focus` hand-back, which a modal opening in it
    /// doesn't take as where focus was.
    focus_returned: bool,
}

impl KeyboardState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            focus: Default::default(),
            type_select: Default::default(),
            held: Default::default(),
            modifier_keys: Default::default(),
            mods_heard: Default::default(),
            held_mods: Default::default(),
            key_releases: Default::default(),
            keys_lost: false,
            full_keyboard_access: true,
            tab_visits_controls: false,
            media_keys: false,
            modal_focus: crate::runtime::modal_focus::ModalFocus::Closed,
            focus_before_commit: None,
            focus_request: None,
            focus_owed: None,
            focus_returned: false,
            unmounted_focused: Vec::new(),
            modal_scope: None,
        }
    }

    #[inline]
    pub(in crate::runtime) fn has_focus(&self) -> bool {
        self.focus.path().is_some()
    }

    /// The focused element's index. Unlike `RuntimeCore::node`, this finds it while it exits.
    #[inline]
    pub(in crate::runtime) fn focused_index(&self, nodes: &[crate::runtime::Node]) -> Option<usize> {
        let path = self.focus.path()?;
        nodes.iter().position(|n| n.path == path)
    }

    #[inline]
    pub(in crate::runtime) fn highlight_visible(&self, nodes: &[crate::runtime::Node], i: usize) -> bool {
        self.focus.shown() && self.focus.is(&nodes[i].path)
    }

    /// Focus is on node `i` or inside it (CSS `:focus-within`), however it got there.
    #[inline]
    pub(in crate::runtime) fn focus_within(&self, nodes: &[crate::runtime::Node], i: usize) -> bool {
        let path = nodes[i].path.as_str();
        self.focus.path().is_some_and(|f| f.strip_prefix(path).is_some_and(|rest| rest.is_empty() || rest.starts_with('/')))
    }

    #[inline]
    pub(super) fn tab_visits_all(&self) -> bool {
        self.full_keyboard_access || self.tab_visits_controls
    }

    #[inline]
    pub(in crate::runtime) fn ime_took_key(&mut self) {
        self.key_releases.ime_took_key();
    }

    #[inline]
    pub(in crate::runtime) fn clear_ime_key_up(&mut self) {
        self.key_releases.clear_ime_eat();
    }

    #[inline]
    pub(super) fn note_key_down(&mut self, composing: bool, repeat: bool) {
        let focus = &self.focus;
        self.key_releases.down(composing, repeat, || focus.owned());
    }

    #[inline]
    pub(super) fn eat_ime_key_up(&mut self, composing: bool) -> bool {
        self.key_releases.eat(composing)
    }

    #[inline]
    pub(super) fn route_key_release(&mut self) -> Option<String> {
        self.key_releases.route()
    }

    #[inline]
    pub(super) fn composing_release(&mut self) {
        self.key_releases.composing_release();
    }

    #[inline]
    pub(super) fn key_releases_lost(&mut self) {
        self.key_releases.lost();
    }

    #[inline]
    pub(super) fn type_select_typed(&mut self, typed: &str, now: f64) -> &str {
        self.type_select.typed(typed, now)
    }

    #[inline]
    pub(super) fn reset_type_select(&mut self) {
        self.type_select.reset();
    }

    #[cfg(test)]
    pub(super) fn age_type_select(&mut self, delta: f64) {
        if let TypeSelect::Typing { at, .. } = &mut self.type_select {
            *at -= delta;
        }
    }

    #[inline]
    pub(super) fn set_focus_returned(&mut self, returned: bool) {
        self.focus_returned = returned;
    }

    #[inline]
    pub(super) fn take_focus_returned(&mut self) -> bool {
        std::mem::take(&mut self.focus_returned)
    }

    /// Updates the last-reported modifiers and returns them if they changed.
    #[inline]
    pub(super) fn take_mods_changed(&mut self) -> Option<crate::Mods> {
        if self.held_mods == self.mods_heard { return None; }
        self.mods_heard = self.held_mods;
        Some(self.mods_heard)
    }
}
