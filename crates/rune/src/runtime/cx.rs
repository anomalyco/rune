//! View environment (`Env`), background wake handle (`Waker`) and view context (`Cx`).
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Env {
    pub system_dark: bool,
    pub scale: f32,
}
impl Env {
    pub const fn new(system_dark: bool, scale: f32) -> Self {
        Self { system_dark, scale }
    }
}
impl Default for Env {
    fn default() -> Self { Self::new(true, 1.0) }
}

/// Wakes the shell from another thread (new data arrived); a no-op headless.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    pub(super) static COMMITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many commits (runs of a view that go on to the screen) have started on this thread,
/// across all its windows; 0 before the first. A cache of what views draw can keep whatever
/// the last commit or two used and evict the rest.
pub fn commit_generation() -> u64 {
    COMMITS.get()
}

/// Passed to view functions: the environment, plus runtime settings a view may declare.
pub struct Cx<S> {
    pub(super) env: Env,
    pub(super) titlebar_inset: f32,
    pub(super) system_accent: crate::SystemAccent,
    pub(super) system_reduced_motion: bool,
    pub(super) reduced_motion: bool,
    pub(super) time_scale: f32,
    pub(super) caret: Option<(String, [f32; 4])>,
    pub(super) caret_read: std::cell::Cell<bool>,
    pub(super) layout: layout_read::LayoutRects,
    pub(super) layout_reads: std::cell::RefCell<layout_read::LayoutReads>,
    pub(super) _s: PhantomData<fn(&S)>,
}

impl<S> Cx<S> {
    pub(crate) fn retype<T>(&self) -> Cx<T> {
        Cx {
            env: self.env, titlebar_inset: self.titlebar_inset, system_accent: self.system_accent,
            system_reduced_motion: self.system_reduced_motion, reduced_motion: self.reduced_motion, time_scale: self.time_scale,
            caret: self.caret.clone(), caret_read: self.caret_read.clone(),
            layout: self.layout.clone(), layout_reads: self.layout_reads.clone(), _s: PhantomData,
        }
    }

    /// Takes back a [`Self::retype`]d context a nested view ran with: its overrides, and the
    /// caret and layout reads it added.
    pub(crate) fn absorb<T>(&mut self, other: Cx<T>) {
        self.reduced_motion = other.reduced_motion;
        self.time_scale = other.time_scale;
        self.caret_read.set(self.caret_read.get() || other.caret_read.get());
        let mut reads = self.layout_reads.borrow_mut();
        for read in other.layout_reads.into_inner() {
            layout_read::note_read(&mut reads, &read.key, read.same);
        }
    }

    pub(crate) fn set_env(&mut self, env: Env) {
        self.env = env;
    }

    pub(crate) fn set_titlebar_inset(&mut self, inset: f32) {
        self.titlebar_inset = inset;
    }

    #[cfg(feature = "hot")]
    pub(crate) fn settings_from<T>(&mut self, other: Cx<T>) {
        self.reduced_motion = other.reduced_motion;
        self.time_scale = other.time_scale;
        self.caret_read = other.caret_read;
        self.layout_reads = other.layout_reads;
    }

    pub fn env(&self) -> Env {
        self.env
    }
    pub fn system_dark(&self) -> bool {
        self.env.system_dark
    }
    /// Points from the window's leading edge to the trailing edge of the macOS traffic lights
    /// drawn over the content, for a top bar's leading inset: 0 when nothing is drawn there (full
    /// screen, a titled window whose content starts below its titlebar, other platforms). The
    /// view re-runs when it changes, as entering and leaving full screen do. Headless runs report
    /// [`Chrome::titlebar_inset`](crate::Chrome::titlebar_inset). A `memo` closure that reads
    /// it needs it in its version.
    pub fn titlebar_inset(&self) -> f32 {
        self.titlebar_inset
    }
    /// The user's accent colour (System Settings › Appearance on macOS) for this window's
    /// appearance, [`SystemAccent::DEFAULT`](crate::SystemAccent::DEFAULT)'s blue elsewhere and
    /// headless. The view re-runs, memos included, when the user picks another accent.
    pub fn accent(&self) -> Color {
        self.system_accent.accent(self.env.system_dark)
    }
    /// The keyboard focus ring colour for this window's appearance: AppKit's
    /// `keyboardFocusIndicatorColor`, the accent at 50% elsewhere. Elements with no ring of
    /// their own draw their default focus ring in it.
    pub fn focus_color(&self) -> Color {
        self.system_accent.focus(self.env.system_dark)
    }
    /// Browser safe-area insets in points, in top/right/bottom/left order. Zero on native.
    /// Apply these inside a full-canvas root so backgrounds can extend to the screen edges.
    pub fn safe_area(&self) -> [f32; 4] {
        #[cfg(target_arch = "wasm32")]
        { crate::web::safe_area() }
        #[cfg(not(target_arch = "wasm32"))]
        { [0.0; 4] }
    }
    /// The host's OS accessibility preference, independent of this commit's override.
    pub fn system_reduced_motion(&self) -> bool {
        self.system_reduced_motion
    }
    /// Whether this commit reduces motion: the host preference, or the view's override from
    /// [`Self::reduced_motion`]. Pass it to builders that pick a different motion under it.
    pub fn is_reduced_motion(&self) -> bool {
        self.reduced_motion
    }
    /// Override the host preference for this commit: travel jumps, opacity/colour briefly fade.
    pub fn reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }
    /// Scale the window's animation clock (slow motion below 1, paused at 0). To slow or freeze
    /// only part of the window, use [`El::time_scale`](crate::El::time_scale), which multiplies
    /// with this.
    pub fn time_scale(&mut self, scale: f32) {
        self.time_scale = scale.clamp(0.0, 8.0);
    }
    /// The focused text field's caret line in window points `[x, y, w, h]`, as last drawn (after
    /// scrolling, at the cursor inside an IME composition), when that field is keyed `key` or sits
    /// inside an element keyed `key`. A view that reads it re-runs when the caret moves, so a
    /// popup placed from it lands the frame after each edit.
    pub fn caret(&self, key: &str) -> Option<[f32; 4]> {
        self.caret_read.set(true);
        let rect = caret_of(&self.caret, key);
        crate::memo::read_caret(key, rect);
        rect
    }
}

/// [`Cx::caret`] for the caret `caret` (the focused field's path and rect).
pub(crate) fn caret_of(caret: &Option<(String, [f32; 4])>, key: &str) -> Option<[f32; 4]> {
    let (path, rect) = caret.as_ref()?;
    path.split('/').any(|seg| seg == key).then_some(*rect)
}
