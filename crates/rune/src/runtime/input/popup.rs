use super::*;
use crate::popup_menu::{PopupAnchor, PopupMenu, PopupMenuError, Request, Show};
use std::collections::VecDeque;

/// The pop-up menus asked for (`Tx::popup_menu`) and not yet answered, oldest first. Menus show
/// one at a time, so only the first can be open, and an open one is always the first.
pub(crate) enum Popups {
    /// None is open: the first, if any, waits for the host to open it.
    Queued(VecDeque<Request>),
    /// `open` is showing (or couldn't, and waits for its `None`); `next` wait behind it.
    Showing { open: Request, next: VecDeque<Request> },
}

impl Default for Popups {
    fn default() -> Self { Popups::Queued(VecDeque::new()) }
}

impl Popups {
    pub(crate) fn front(&self) -> Option<&Request> {
        match self {
            Popups::Queued(queue) => queue.front(),
            Popups::Showing { open, .. } => Some(open),
        }
    }
    pub(crate) fn push_back(&mut self, request: Request) {
        match self {
            Popups::Queued(queue) | Popups::Showing { next: queue, .. } => queue.push_back(request),
        }
    }
    pub(crate) fn extend(&mut self, requests: impl IntoIterator<Item = Request>) {
        for r in requests { self.push_back(r); }
    }
    pub(crate) fn len(&self) -> usize {
        match self {
            Popups::Queued(queue) => queue.len(),
            Popups::Showing { next, .. } => next.len() + 1,
        }
    }
    pub(crate) fn is_empty(&self) -> bool { self.len() == 0 }
    /// The first is showing.
    pub(crate) fn is_open(&self) -> bool { matches!(self, Popups::Showing { .. }) }

    /// The host opens the first: it shows from now until it's answered.
    fn open_first(&mut self) -> Option<&Request> {
        if let Popups::Queued(queue) = self {
            let open = queue.pop_front()?;
            *self = Popups::Showing { open, next: std::mem::take(queue) };
        }
        self.front()
    }

    /// The first is answered: it leaves, and the next waits for the host to open it.
    fn answer_first(&mut self) -> Option<Request> {
        match std::mem::take(self) {
            Popups::Queued(mut queue) => {
                let first = queue.pop_front();
                *self = Popups::Queued(queue);
                first
            }
            Popups::Showing { open, next } => {
                *self = Popups::Queued(next);
                Some(open)
            }
        }
    }
}

impl RuntimeCore {
    /// The pop-up menu showing, or the next to show, from [`Tx::popup_menu`].
    pub(crate) fn popup_menu(&self) -> Option<&PopupMenu> { self.host.popups.front().map(|r| &r.menu) }

    /// Answers the pop-up menu showing (or next to show) as if the user picked `picked`, or
    /// dismissed it with `None`. Its callback runs now and the next frame commits it, as for
    /// [`Runtime::update`]. For tests and headless drivers; native menus answer themselves.
    pub(crate) fn resolve_popup(&mut self, app: &mut dyn AppDriver, picked: Option<&str>) -> Result<(), PopupMenuError> {
        let request = self.host.popups.front().ok_or(PopupMenuError::NotOpen)?;
        if let Some(id) = picked {
            let item = request.menu.find(id).ok_or_else(|| PopupMenuError::UnknownItem(id.into()))?;
            if !item.enabled { return Err(PopupMenuError::Disabled(id.into())); }
        }
        let Some(request) = self.host.popups.answer_first() else { return Err(PopupMenuError::NotOpen) };
        match picked {
            Some(id) => self.trace_event(format_args!("popup-menu:pick:{id}")),
            None => self.trace_event("popup-menu:cancel"),
        }
        let mut one = self.transaction(app);
        if let (Some(field), Some(id)) = (request.field, picked) {
            self.field_menu_pick(app, *field, id, &mut one);
        }
        (request.on_pick)(app.states(), picked, &mut one);
        self.latch_requests(app, &mut one);
        merge(&mut self.frame.pending, one);
        self.commit.dirty = true;
        Ok(())
    }

    /// The next menu for the shell to open, marked open. `Some(None)` is a request that can't
    /// open (its anchor element is gone, or its ids repeat): answer it with `None`. Nothing while
    /// a menu is open, so menus show one at a time.
    pub(crate) fn take_popup(&mut self) -> Option<Option<Show>> {
        if self.host.popups.is_open() { return None; }
        let rect = match &self.host.popups.front()?.anchor {
            PopupAnchor::Key(key) => self.anchor_rect(key).ok_or_else(|| format!("no element keyed {key:?} to anchor to")),
            &PopupAnchor::Point(x, y) => Ok([x, y, 0.0, 0.0]),
        };
        // The host places it in window points: the view's times its zoom.
        let rect = rect.map(|r| self.to_window(r));
        let request = self.host.popups.open_first()?;
        match request.validate().and(rect) {
            Ok(rect) => {
                let show = Show::new(request.menu.clone(), &request.anchor, rect);
                self.trace_event("popup-menu:open");
                // Asked for outside a frame (an update): the next frame drops the hover.
                if self.release_pointer_for_popup() { self.commit.dirty = true; }
                Some(Some(show))
            }
            Err(problem) => {
                eprintln!("rune: pop-up menu: {problem}");
                Some(None)
            }
        }
    }

    /// The next menu anchors to an element and the runtime has changes the next frame commits
    /// (a pick that asked for another menu over what it mounted): open it after that frame.
    pub(crate) fn popup_waits_for_layout(&self) -> bool {
        !self.host.popups.is_open() && (self.commit.dirty || self.commit.relayout)
            && self.host.popups.front().is_some_and(|r| matches!(r.anchor, PopupAnchor::Key(_)))
    }

    /// A menu is about to take the mouse: press, capture and hover end now, as if the pointer
    /// left, rather than when it next moves. True when that changed anything.
    pub(crate) fn release_pointer_for_popup(&mut self) -> bool {
        if self.host.popups.is_empty() || (self.pointer.pos.is_none() && !self.pointer_captured()) {
            return false;
        }
        self.cancel_pointer();
        true
    }

    /// Answers every waiting menu with `None`, as its window goes away. A callback that asks
    /// for another menu is answered too, up to a bound against one that always asks again.
    pub(crate) fn cancel_popups(&mut self, app: &mut dyn AppDriver) {
        for _ in 0..64 {
            if self.resolve_popup(app, None).is_err() { return; }
        }
        eprintln!("rune: pop-up menu callbacks kept asking for menus while their window went away; dropping {}", self.host.popups.len());
        self.host.popups = Popups::default();
    }

    /// A headless run's stand-in for the window's menu: the next menu opens and waits for
    /// [`Runtime::resolve_popup`], and one that can't open answers `None` now.
    pub(crate) fn open_popup_headless(&mut self, app: &mut dyn AppDriver) {
        while let Some(None) = self.take_popup() {
            self.resolve_popup(app, None).ok();
        }
    }

    /// Drawn rect of the element a menu anchors to, matched as [`Runtime::find`] matches.
    fn anchor_rect(&self, key: &str) -> Option<[f32; 4]> {
        let i = self.live_named(None, key)?;
        self.paint.drawn.get(i).copied().filter(|r| r[2] > 0.0 || r[3] > 0.0)
    }

    pub(crate) fn popup_json(&self) -> Option<serde_json::Value> {
        let request = self.host.popups.front()?;
        let mut v = request.menu.to_json();
        v["open"] = self.host.popups.is_open().into();
        v["anchor"] = match &request.anchor {
            PopupAnchor::Key(key) => serde_json::json!({ "key": key }),
            PopupAnchor::Point(x, y) => serde_json::json!({ "x": x, "y": y }),
        };
        v["queued"] = (self.host.popups.len() - 1).into();
        Some(v)
    }
}
