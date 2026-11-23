//! A launch's menu bar waits for its first window's first frame. Making it is AppKit work on
//! the main thread before anything shows (winit's default menu, the standard items, the app's
//! own: about 5,000 µs of a launch by `open`), and nobody can reach a menu before the window
//! is on screen. The three parts go in the order they always did, in the same turn of the
//! launch, so AppKit ends its launch on the same menu bar. Plain state, testable on every
//! platform; `mod.rs` does the AppKit work.

/// What the menu bar owes while it waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BarWait {
    waits: bool,
    /// A window start asked for the standard items: `App::standard_menus` of any of them.
    standard: Option<bool>,
}

/// What the wait's end makes, in this order: winit's default menu, then the standard items if
/// a window start asked (`opt_in` as `standard_macos::install` takes it), then each window's
/// own items as its guard drains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owed {
    pub standard: Option<bool>,
}

impl BarWait {
    pub(crate) const fn new() -> Self {
        Self { waits: false, standard: None }
    }

    /// The event loop starts with winit's default menu turned off: the bar waits.
    pub(crate) fn begin(&mut self) {
        self.waits = true;
    }

    pub(crate) fn waits(&self) -> bool {
        self.waits
    }

    /// A window start asks for the standard items. True while the bar waits: they come at
    /// its end.
    pub(crate) fn standard(&mut self, opt_in: bool) -> bool {
        if self.waits { self.standard = Some(self.standard.unwrap_or(false) || opt_in); }
        self.waits
    }

    /// The first frame is on its way, or something needs the menu bar now. Once.
    pub(crate) fn end(&mut self) -> Option<Owed> {
        std::mem::take(&mut self.waits).then(|| Owed { standard: self.standard.take() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_that_never_waited_owes_nothing() {
        let mut bar = BarWait::new();
        assert!(!bar.waits());
        assert!(!bar.standard(true), "the standard items are made at once");
        assert_eq!(bar.end(), None);
    }

    #[test]
    fn the_wait_ends_once_with_what_the_window_starts_asked() {
        let mut bar = BarWait::new();
        bar.begin();
        assert!(bar.waits());
        assert!(bar.standard(false));
        assert!(bar.standard(true), "a second window's start, before the first one's frame");
        assert!(bar.standard(false));
        assert_eq!(bar.end(), Some(Owed { standard: Some(true) }));
        assert!(!bar.waits());
        assert_eq!(bar.end(), None, "the menu bar is made once");
        assert!(!bar.standard(true), "a window opened later makes its items at once");
    }

    #[test]
    fn a_launch_with_no_window_still_owes_the_default_menu() {
        let mut bar = BarWait::new();
        bar.begin();
        assert_eq!(bar.end(), Some(Owed { standard: None }));
    }

    #[test]
    fn windows_that_did_not_ask_keep_the_standard_items_to_the_default_menu_check() {
        let mut bar = BarWait::new();
        bar.begin();
        bar.standard(false);
        assert_eq!(bar.end(), Some(Owed { standard: Some(false) }));
    }
}
