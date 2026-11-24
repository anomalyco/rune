//! A launch's first view given what the last launch's view read from layout.
//!
//! A view that reads a rect ([`Cx::layout`], `layout_with`, `layout_size`) gets nothing in a
//! window's first commit, since nothing is laid out yet, and runs a second time once layout
//! knows the rect: a desktop app's whole first screen, built twice before its first frame. An
//! app that keeps its window's state across launches can keep the reads too
//! ([`App::layout_seed`](crate::App::layout_seed)): Rune hands it the rects the window's FIRST
//! view read, when the window rests, closes or quits, and takes them back before the next
//! launch's first commit. That commit's view is then given them at once, and when layout agrees
//! with them it runs once.
//!
//! The first view's reads, not the last one's: a launch's first frame is drawn before a server
//! or a plugin has answered, and what arrives afterwards moves things (a header's buttons, say)
//! that the next launch's first frame will find where this one's did. A launch after the person
//! changed the layout (a pane opened, a divider moved) has a stale seed once, and its own first
//! frame replaces it.
//!
//! A seed is a guess, checked like any read: the first commit compares what the view was given
//! with its layout, by each read's own test, and runs the view again where they differ, as it
//! always did. So a seed from another state costs what no seed costs, and one for another
//! window size or scale isn't used at all.
use super::layout_read::SELECTION;
use super::*;

/// What a window's first view read from layout, to give to the first view of its next launch
/// ([`App::layout_seed`](crate::App::layout_seed)). Plain data, so an app can keep it wherever
/// it keeps the window's state.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSeed {
    /// The view's viewport in its own points (the window's over the zoom) when it read them.
    pub viewport: [f32; 2],
    /// Device pixels per view point then.
    pub scale: f32,
    /// The view's UI zoom then ([`Cx::zoom`]).
    pub zoom: f32,
    /// Each key the view read and its rect `[x, y, w, h]`, `None` for a key with no element,
    /// ordered by key.
    pub rects: Vec<(String, Option<[f32; 4]>)>,
}

/// Where a runtime's seed stands.
#[derive(Default)]
pub(crate) enum Seeded {
    /// None was offered, or the one offered was for another viewport.
    #[default]
    No,
    /// Offered, for the first commit to take.
    Offered(LayoutSeed),
    /// The first view is given its rects; layout hasn't checked them yet.
    Given,
    /// The first commit's layout checked them.
    Checked,
}

impl RuntimeCore {
    /// What the view's layout reads resolve to now, for [`Self::offer_layout_seed`] at the next
    /// launch. `None` when the view reads no rect.
    pub fn layout_seed(&self) -> Option<LayoutSeed> {
        let seen = &self.commit.layout_seen;
        if seen.is_empty() { return None; }
        let mut rects: Vec<_> = seen.rects.iter().filter(|(k, _)| k.as_str() != SELECTION).map(|(k, r)| (k.clone(), *r)).collect();
        if rects.is_empty() { return None; }
        rects.sort_by(|a, b| a.0.cmp(&b.0));
        Some(LayoutSeed { viewport: [self.viewport.0, self.viewport.1], scale: self.env.scale, zoom: self.host.zoom(), rects })
    }

    /// What the FIRST commit's view read, as its layout settled them: what a launch's first view
    /// reads again at the next launch, before a server or a plugin has answered. `None` before
    /// the first commit, and when that view read no rect.
    pub fn first_layout_seed(&self) -> Option<&LayoutSeed> {
        self.commit.layout_first.as_ref()
    }

    /// Before the first commit: `seed` is what its view's layout reads are given, when the
    /// viewport and scale are the seed's by then. After it, nothing.
    pub fn offer_layout_seed(&mut self, seed: LayoutSeed) {
        if self.commit.commits == 0 && matches!(self.commit.layout_seen.seeded, Seeded::No | Seeded::Offered(_)) {
            self.commit.layout_seen.seeded = Seeded::Offered(seed);
        }
    }

    /// As the first commit measures: takes the offered seed. True while the first view's reads
    /// hold a seed's rects, which no layout stands behind yet.
    pub(super) fn seed_layout_reads(&mut self) -> bool {
        if self.commit.commits > 0 { return false; }
        let Seeded::Offered(_) = self.commit.layout_seen.seeded else {
            return matches!(self.commit.layout_seen.seeded, Seeded::Given);
        };
        let Seeded::Offered(seed) = std::mem::take(&mut self.commit.layout_seen.seeded) else { return false };
        // The zoom the view will ask for, so its first run is laid out at it.
        self.apply_zoom(seed.zoom);
        let fits = seed.scale == self.env.scale
            && (seed.viewport[0] - self.viewport.0).abs() < 0.01
            && (seed.viewport[1] - self.viewport.1).abs() < 0.01;
        if !fits || seed.rects.is_empty() { return false; }
        self.commit.layout_seen.rects.extend(seed.rects);
        self.commit.layout_seen.seeded = Seeded::Given;
        true
    }

    /// The first commit's layout checked the seed's rects: `held` when every read agreed, so
    /// the view ran once. Marks which (`layout.seed.held`, `layout.seed.moved`).
    pub(super) fn seed_checked(&mut self, held: bool) {
        if !matches!(self.commit.layout_seen.seeded, Seeded::Given) { return; }
        self.commit.layout_seen.seeded = Seeded::Checked;
        crate::marks::mark(if held { "layout.seed.held" } else { "layout.seed.moved" });
    }

    /// Whether the first commit's layout has yet to check a seed's rects.
    pub(super) fn seed_unchecked(&self) -> bool {
        matches!(self.commit.layout_seen.seeded, Seeded::Given)
    }
}

/// `RUNE_LAYOUT_SEED=log`: each read the seed got wrong, on stderr.
pub(super) fn log_moved(key: &str, seeded: Option<Option<[f32; 4]>>, now: Option<[f32; 4]>) {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*LOG.get_or_init(|| crate::env::var("RUNE_LAYOUT_SEED").is_some_and(|v| v == "log")) { return; }
    match seeded {
        Some(seeded) => eprintln!("rune: layout seed: {key:?} moved: seeded {seeded:?}, laid out {now:?}"),
        None => eprintln!("rune: layout seed: {key:?} was not in the seed, laid out {now:?}"),
    }
}
