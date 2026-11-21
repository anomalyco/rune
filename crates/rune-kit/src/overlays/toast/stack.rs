//! The toasts a [`ToastRegion`](crate::ToastRegion) shows and how their stack behaves, as
//! Desktop's toast registry (`ui/src/feedback/toast/machine.ts`) and the Sonner toaster under it
//! (solid-sonner's `Toaster`, styled by `ui/src/feedback/toast/toast.css`) run them.
//!
//! - **Registry:** a toast shown again while it's still in front updates in place and pulses
//!   rather than stacking a copy, and a buried copy moves to the front as a new toast.
//! - **Stack:** collapsed, the newest toast is in front and up to two more peek above it,
//!   [`PEEK`] higher and [`SHRINK`] smaller each, as tall as the front one, their contents hidden.
//!   The pointer over the stack expands it: each toast at its own height, [`GAP`] apart. It
//!   collapses as the pointer leaves, unless a press inside is still held (then as that press
//!   lets go outside), or when one toast is left.
//! - **Timers:** a toast that closes on its own counts its time only while the stack is
//!   collapsed, no press is held in it and the window shows, as Sonner pauses every toast while
//!   `expanded || interacting || isDocumentHidden` and resumes each with what it had left.
//!
//! The app owns a [`ToastStack`], steps it with what happens and renders [`ToastStack::entries`]
//! where [`ToastStack::place`] says.

use rune::machine::Effects;

/// Points between two toasts of an expanded stack (Desktop's `Toast.Region` `gap={12}`).
pub const GAP: f32 = 12.0;
/// Points each toast behind the front one rises in a collapsed stack: Sonner's own
/// `--lift-amount * --toasts-before`, a gap each, which wins the cascade over `toast.css`'s
/// 9.5 px (measured in Desktop: `translateY(-12px) scale(0.95)` for the toast behind).
pub const PEEK: f32 = GAP;
/// How much smaller each toast behind the front one draws in a collapsed stack
/// (`--toasts-before * 0.05`).
pub const SHRINK: f32 = 0.05;
/// Toasts that show; those behind them are hidden (Sonner's `visibleToasts`).
pub const VISIBLE: usize = 3;
/// Seconds a toast moves and resizes over as the stack expands or collapses, on
/// `cubic-bezier(0.2, 0, 0, 1)` (`toast.css`: `transform 280ms`, `height 280ms`).
pub const MOVE_SECS: f32 = 0.28;
/// Seconds a toast's own opacity eases over, ease-out (`opacity 160ms ease-out`).
pub const FADE_SECS: f32 = 0.16;
/// Seconds a toast's contents fade over as it moves behind or out from behind the front one,
/// on CSS `ease` (Sonner's `[data-sonner-toast] > * { transition: opacity 400ms }`).
pub const CONTENT_FADE_SECS: f32 = 0.4;

/// The toasts, oldest first, and the stack's state. See the module docs.
#[derive(Clone, Debug)]
pub struct ToastStack<O> {
    issued: u64,
    entries: Vec<ToastEntry<O>>,
    stack: ToastStackState,
    page: ToastPage,
}

/// One toast: its id, the key that coalesces copies (its title, description, variant and
/// actions, say), what it shows and its timer.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastEntry<O> {
    pub id: u64,
    pub key: String,
    pub options: O,
    pub timer: ToastTimer,
}

/// When a toast closes on its own. Times are the app's clock, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToastTimer {
    /// It stays until dismissed (`persistent`).
    Forever,
    /// It closes at `due`.
    Running { due: f64 },
    /// The stack is expanded or held, or the window is hidden: `left` seconds wait for the
    /// stack to collapse in a window that shows.
    Paused { left: f64 },
}

/// Whether the window the stack is in shows (Sonner's `isDocumentHidden`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastPage {
    #[default]
    Shown,
    /// Minimised or otherwise off screen: no toast's time runs.
    Hidden,
}

/// Sonner's toaster: `expanded` and `interacting` as one state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastStackState {
    /// Newest in front, the rest peeking behind it.
    #[default]
    Collapsed,
    /// The pointer is over the stack: every toast at its own height.
    Expanded,
    /// A press that began on the stack is held; `left` once the pointer has gone outside it.
    Held { left: bool },
}

impl ToastStackState {
    /// The toasts lay out one above the other.
    pub fn expanded(self) -> bool { self != ToastStackState::Collapsed }
}

pub enum ToastEvent<O> {
    /// A toast shows (`toast.shown`): `secs` until it closes on its own, or `None` to stay.
    Shown { key: String, options: O, secs: Option<f64>, now: f64 },
    /// One toast closed (its close button, an action, the app), or every toast (`None`).
    Dismissed { id: Option<u64>, now: f64 },
    /// The pointer came onto the stack or moved over it (`mouseenter`, `mousemove`).
    PointerOver { now: f64 },
    /// The pointer left the stack (`mouseleave`).
    PointerLeft { now: f64 },
    /// A press began on the stack (`pointerdown`).
    Pressed { now: f64 },
    /// That press let go (`pointerup`, and the `mouseleave` a release outside brings).
    Released { now: f64 },
    /// The stack's hotkey expands it (Sonner's default Alt+T).
    Hotkey { now: f64 },
    /// Escape with focus in the stack collapses it.
    Escape { now: f64 },
    /// The clock reached a timer: toasts whose time is up close.
    Tick { now: f64 },
    /// The window went off screen or came back (`visibilitychange`).
    Page { page: ToastPage, now: f64 },
}

#[derive(Debug, PartialEq)]
pub enum ToastEffect {
    /// The toast in front was shown again: pulse it.
    Pulse(u64),
    /// This toast's time ran out and it closed (`onAutoClose`).
    Closed(u64),
    /// Wake at this clock time, when the next timer is due.
    WakeAt(f64),
}

/// Where a toast sits in its stack, from the front toast's place at the stack's foot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastPlacement {
    /// Points above the front toast's bottom edge to this toast's bottom edge.
    pub lift: f32,
    /// Its scale about its top centre.
    pub scale: f32,
    /// Its height: its own, or the front toast's while it's behind it collapsed.
    pub height: f32,
    /// Its contents show (not behind the front one in a collapsed stack).
    pub content: bool,
    /// It shows at all (one of the first [`VISIBLE`]).
    pub visible: bool,
    /// Its order from the front: 0 for the newest.
    pub index: usize,
}

impl<O> Default for ToastStack<O> {
    fn default() -> Self { Self { issued: 0, entries: Vec::new(), stack: ToastStackState::Collapsed, page: ToastPage::Shown } }
}

impl<O> ToastStack<O> {
    /// Oldest first; the last is in front.
    pub fn entries(&self) -> &[ToastEntry<O>] { &self.entries }
    pub fn entries_mut(&mut self) -> &mut [ToastEntry<O>] { &mut self.entries }
    /// The toast in front.
    pub fn front(&self) -> Option<&ToastEntry<O>> { self.entries.last() }
    pub fn stack(&self) -> ToastStackState { self.stack }
    /// The timer that comes due first.
    pub fn next_due(&self) -> Option<f64> {
        self.entries.iter().filter_map(|e| match e.timer { ToastTimer::Running { due } => Some(due), _ => None }).reduce(f64::min)
    }

    /// Where toast `id` sits, given each toast's own height by id (`height`).
    pub fn place(&self, id: u64, height: impl Fn(u64) -> f32) -> Option<ToastPlacement> {
        let at = self.entries.iter().position(|e| e.id == id)?;
        let index = self.entries.len() - 1 - at;
        let visible = index < VISIBLE;
        let own = height(id);
        if self.stack.expanded() {
            // Sonner's `--offset`: the heights of the toasts in front, and a gap after each.
            let lift = self.entries[at + 1..].iter().map(|e| height(e.id) + GAP).sum();
            return Some(ToastPlacement { lift, scale: 1.0, height: own, content: true, visible, index });
        }
        if index == 0 {
            return Some(ToastPlacement { lift: 0.0, scale: 1.0, height: own, content: true, visible, index });
        }
        let front = self.front().map_or(own, |f| height(f.id));
        let k = index as f32;
        Some(ToastPlacement { lift: k * PEEK, scale: 1.0 - k * SHRINK, height: front, content: false, visible, index })
    }

    /// Whether the window shows.
    pub fn page(&self) -> ToastPage { self.page }

    /// No toast's time runs: the stack is expanded or held, or the window is hidden.
    fn paused(&self) -> bool { self.stack.expanded() || self.page == ToastPage::Hidden }

    pub fn step(&mut self, event: ToastEvent<O>) -> Effects<ToastEffect> {
        let (was, before) = (self.paused(), self.entries.len());
        let (mut effects, now): (_, f64) = match event {
            ToastEvent::Dismissed { id, now } => {
                match id {
                    Some(id) => self.entries.retain(|e| e.id != id),
                    None => self.entries.clear(),
                }
                (Effects::none(), now)
            }
            ToastEvent::Shown { key, options, secs, now } => (self.shown(key, options, secs, now), now),
            ToastEvent::PointerOver { now } => {
                self.stack = match self.stack {
                    ToastStackState::Held { .. } => ToastStackState::Held { left: false },
                    _ => ToastStackState::Expanded,
                };
                (Effects::none(), now)
            }
            ToastEvent::PointerLeft { now } => {
                self.stack = match self.stack {
                    ToastStackState::Held { .. } => ToastStackState::Held { left: true },
                    _ => ToastStackState::Collapsed,
                };
                (Effects::none(), now)
            }
            ToastEvent::Pressed { now } => {
                self.stack = ToastStackState::Held { left: false };
                (Effects::none(), now)
            }
            ToastEvent::Released { now } => {
                self.stack = match self.stack {
                    ToastStackState::Held { left: false } => ToastStackState::Expanded,
                    ToastStackState::Held { left: true } => ToastStackState::Collapsed,
                    other => other,
                };
                (Effects::none(), now)
            }
            ToastEvent::Hotkey { now } => {
                if self.stack == ToastStackState::Collapsed { self.stack = ToastStackState::Expanded; }
                (Effects::none(), now)
            }
            ToastEvent::Escape { now } => {
                if self.stack == ToastStackState::Expanded { self.stack = ToastStackState::Collapsed; }
                (Effects::none(), now)
            }
            ToastEvent::Tick { now } => {
                let mut closed = Effects::none();
                self.entries.retain(|e| {
                    let due = matches!(e.timer, ToastTimer::Running { due } if now + 1e-3 >= due);
                    if due { closed.push(ToastEffect::Closed(e.id)); }
                    !due
                });
                (closed, now)
            }
            ToastEvent::Page { page, now } => {
                self.page = page;
                (Effects::none(), now)
            }
        };
        // One toast left collapses the stack (Sonner's `toasts.length <= 1` effect); a held
        // press keeps it until it lets go.
        if self.entries.len() <= 1 && self.entries.len() != before && self.stack == ToastStackState::Expanded {
            self.stack = ToastStackState::Collapsed;
        }
        self.retime(was, now);
        if let Some(due) = self.next_due() { effects.push(ToastEffect::WakeAt(due)); }
        effects
    }

    fn shown(&mut self, key: String, options: O, secs: Option<f64>, now: f64) -> Effects<ToastEffect> {
        let timer = match secs {
            None => ToastTimer::Forever,
            Some(s) if self.paused() => ToastTimer::Paused { left: s },
            Some(s) => ToastTimer::Running { due: now + s },
        };
        if let Some(front) = self.entries.last_mut().filter(|e| e.key == key) {
            front.options = options;
            front.timer = timer;
            return Effects::one(ToastEffect::Pulse(front.id));
        }
        self.entries.retain(|e| e.key != key);
        self.issued += 1;
        self.entries.push(ToastEntry { id: self.issued, key, options, timer });
        Effects::none()
    }

    /// Pauses every timer as the stack expands, a press holds it or the window hides, and
    /// resumes each with what it had left once none of those holds.
    fn retime(&mut self, was: bool, now: f64) {
        match (was, self.paused()) {
            (false, true) => {
                for e in &mut self.entries {
                    if let ToastTimer::Running { due } = e.timer { e.timer = ToastTimer::Paused { left: (due - now).max(0.0) }; }
                }
            }
            (true, false) => {
                for e in &mut self.entries {
                    if let ToastTimer::Paused { left } = e.timer { e.timer = ToastTimer::Running { due: now + left }; }
                }
            }
            _ => {}
        }
    }
}
