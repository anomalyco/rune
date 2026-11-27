//! [`memo`]: reuse a subtree whose inputs haven't changed instead of building it again.
//!
//! A view runs top to bottom every commit, so a transcript of 500 messages builds 500 messages
//! every frame its counter ticks. Wrapping each in `memo(id, version, || ...)` builds a message
//! once per version: later commits get the element the first build returned, without calling
//! the closure. Children reach the element tree only when the element mounts, so the rows a
//! `virtualize` list leaves offscreen cost a lookup each and nothing else.
//!
//! What a reused subtree keeps, and what invalidates it:
//!
//! - **Identity.** A reused subtree is the same elements under the same key paths, so node ids,
//!   hover and press, focus, scroll offsets and running springs carry on exactly as if it had
//!   been built again. The inspector, `--tree`, `Runtime::elements` and accessibility read the
//!   element tree after the subtree is in it, and see it like any other.
//! - **Handlers** in the subtree are the ones its build made. They see what they captured then,
//!   so anything a handler captures that can change must be part of `version`. Handlers that
//!   only send a message or an id never go stale. A row's position is one of those things: a
//!   handler in a list row should capture the row's id, never its index, which shifts when
//!   rows are inserted or removed above it while the memo keeps the old handler.
//! - **Live values and pixels** stay bound: a `Live`, `LiveText` or `Pixels` is a shared handle,
//!   so a reused subtree shows its newest value or image without a rebuild.
//! - **Environment.** The window's dark mode, scale, reduced-motion preference, system accent
//!   (`Cx::accent`, `Cx::focus_color`) and blend mode (`theme::blends_srgb`, which theme
//!   colours resolve by) are part of every version, so changing any of them builds
//!   again. App state the closure reads (a theme, a locale, a selection) must be part of
//!   `version`. For a text field that's the
//!   `TextEdit` itself, which hashes its text, selection, composition and focus.
//! - **`Cx` reads.** `Cx::layout`, `Cx::layout_with`, `Cx::layout_size`, `Cx::window_layout`,
//!   `Cx::window_layout_with`, `Cx::anchor_placement`, `Cx::caret`, `Cx::selection_rects` and `Cx::field_selection_rects` called inside the closure are recorded with
//!   what they returned. A reuse counts them as read
//!   again, so the view still re-runs when they move, and builds again when one returns
//!   something else. They needn't be in `version`.
//! - **Hot reload** (`rune/hot`) drops every cached subtree, so new code always builds.
//! - **Virtual rows** that scroll out keep their cached subtree as long as the view still
//!   creates them (virtualized rows always are); scrolling back in mounts it again without a
//!   build. A memo the view stops creating is dropped once the commit that stopped creating it
//!   applies (a commit may build its view more than once). Rows `El::virtual_rows` builds count
//!   as part of the view build, so a memo in one is kept while its row stays in the mounted
//!   window and dropped once the row leaves it.
//!
//! The id names the subtree within the window, so it must be unique among the memos of one
//! view build (a message id, say). A memo inside another's closure is named within it, so
//! `memo("b0", ..)` inside each message's memo doesn't collide. A second memo with an id
//! already used in the same build just builds, uncached, and warns once per call site in debug
//! builds (`RUNE_PERF_WARN=0` silences it).
//!
//! Layout isn't stored: a reused subtree still lays out each commit it is mounted, but Taffy
//! keeps each unchanged node's measured size, so this is cheap while width and constraints hold.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::El;
use crate::runtime::HashMap;
use crate::runtime::layout_read::{LayoutRects, SELECTION, anchor_key, field_selection_key, note_read};

mod built;
mod reads;
mod region;
pub(crate) mod snapshot;
#[cfg(test)]
mod tests;

pub(crate) use built::Built;
#[cfg(test)]
pub(crate) use built::Shared;
pub(crate) use built::has_focused_input;
pub(crate) use reads::Reads;
pub(crate) use reads::Replayed;
pub(crate) use reads::read_anchor_placement;
pub(crate) use reads::read_caret;
pub(crate) use reads::read_field_selection;
pub(crate) use reads::read_layout;
pub(crate) use reads::read_layout_with;
pub(crate) use reads::read_selection;
pub(crate) use reads::recording;
use reads::{Read, Seen, dedup_states};

pub use reads::anchor_placement;
pub use reads::layout;
pub use reads::layout_size;
pub use reads::layout_with;
pub use reads::window_layout;
pub use reads::window_layout_with;
pub use region::region;
pub use region::region_warnings;
pub use region::region_with;
pub use snapshot::snapshot;

struct Entry {
    id: Rc<str>,
    version: u64,
    built: Rc<Built>,
    key: Option<String>,
    /// The `Cx` reads its build made, its nested memos' included.
    reads: Vec<Read>,
    /// Memos built inside this one's closure, kept while this one is reused.
    nested: Vec<Rc<str>>,
    /// The view build that last used it.
    pass: u64,
    /// `0` when built in `view`, else [`virtual_row_tag`] of the `virtual_rows` row that built it.
    virtual_row: u64,
}

/// One window's cached subtrees.
#[derive(Default)]
pub(crate) struct MemoCache {
    entries: HashMap<Rc<str>, Entry>,
    pub(crate) flattened: crate::runtime::HashMap<u64, crate::runtime::memo_reuse::FlattenedMemo>,
    pub(crate) pending_scratch: Vec<crate::runtime::memo_reuse::PendingFlat>,
    pub(crate) reused_scratch: Vec<crate::runtime::memo_reuse::ReusedSpan>,
    pub(crate) skip_scratch: Vec<crate::runtime::memo_reuse::CleanApplySpan>,
    pub(crate) split_scratch: Vec<bool>,
    pub(crate) taken_paths_scratch: Vec<(u32, u32)>,
    pub(crate) path_scratch: String,
    id_scratch: String,
    pure_counts: HashMap<(u64, u64, u64), u32>,
    nested_scratch: Vec<(Rc<str>, u64)>,
    pub(crate) present_len: usize,
    pass: u64,
    #[cfg(test)]
    pub(crate) builds: usize,
}

impl MemoCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.flattened.clear();
        self.present_len = 0;
    }

    pub(crate) fn rest(&mut self) {
        fn shrink_vec<T>(v: &mut Vec<T>, floor: usize) {
            let cap = (v.len().saturating_mul(2)).max(floor);
            if v.capacity() > cap { v.shrink_to(cap); }
        }
        fn shrink_map<K: Eq + std::hash::Hash, V, S: std::hash::BuildHasher>(m: &mut std::collections::HashMap<K, V, S>, floor: usize) {
            let cap = (m.len().saturating_mul(2)).max(floor);
            if m.capacity() > cap { m.shrink_to(cap); }
        }
        shrink_map(&mut self.entries, 64);
        shrink_map(&mut self.flattened, 64);
        self.pending_scratch.clear();
        self.reused_scratch.clear();
        self.skip_scratch.clear();
        self.split_scratch.clear();
        self.taken_paths_scratch.clear();
        self.path_scratch.clear();
        self.id_scratch.clear();
        self.pure_counts.clear();
        self.nested_scratch.clear();
        shrink_vec(&mut self.pending_scratch, 32);
        shrink_vec(&mut self.reused_scratch, 32);
        shrink_vec(&mut self.skip_scratch, 32);
        shrink_vec(&mut self.split_scratch, 64);
        shrink_vec(&mut self.taken_paths_scratch, 32);
        if self.path_scratch.capacity() > 256 { self.path_scratch.shrink_to(256); }
        if self.id_scratch.capacity() > 256 { self.id_scratch.shrink_to(256); }
        shrink_map(&mut self.pure_counts, 32);
        shrink_vec(&mut self.nested_scratch, 32);
    }

    /// No memos, counting passes on from this cache's, for a build that must start from scratch.
    #[cfg(feature = "hot")]
    pub(crate) fn emptied(&self) -> MemoCache {
        MemoCache { pass: self.pass, ..MemoCache::default() }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The pass the next view build will be.
    pub(crate) fn next_pass(&self) -> u64 {
        self.pass + 1
    }

    /// Drops the memos whose own id starts with `prefix`, wherever they nest, and those built
    /// inside them.
    pub(crate) fn evict_prefix(&mut self, prefix: &str) {
        let before = self.entries.len();
        self.entries.retain(|id, _| !(id.starts_with(prefix) || id.match_indices('/').any(|(i, _)| id[i + 1..].starts_with(prefix))));
        if self.entries.len() < before {
            let live: crate::runtime::HashSet<u64> = self.entries.values().map(|e| e.built.id).collect();
            self.flattened.retain(|id, _| live.contains(id));
        }
    }

    /// Drops the memos no build since pass `since` (a [`MemoCache::next_pass`]) used: a commit
    /// keeps what any of its view builds used.
    pub(crate) fn keep_since(&mut self, since: u64) {
        // A commit that didn't build the view used no memos, and drops none.
        if self.pass < since { return; }
        let before = self.entries.len();
        self.entries.retain(|_, e| e.pass >= since);
        if self.entries.len() < before && !self.flattened.is_empty() {
            let live: crate::runtime::HashSet<u64> = self.entries.values().map(|e| e.built.id).collect();
            self.flattened.retain(|id, _| live.contains(id));
        }
    }
}

impl crate::runtime::RuntimeCore {
    pub(crate) fn invalidate_memos_by_prefix(&mut self, prefix: &str) {
        self.commit.evict_memo_prefix(prefix);
        self.invalidate();
    }
}

struct Scope {
    cache: MemoCache,
    env: u64,
    scale: f32,
    virtual_row: u64,
    /// The ids of the memos whose closures are running, innermost last.
    path: Vec<Rc<str>>,
    /// The memos each running closure has created so far.
    nested: Vec<Vec<Rc<str>>>,
    /// The `Cx` reads each running closure has made so far.
    reads: Vec<Vec<Read>>,
    seen: Seen,
    replayed: Replayed,
}

impl Scope {
    /// Counts `reads` toward the running closure, if any.
    fn note(&mut self, reads: &[Read]) {
        let Some(frame) = self.reads.last_mut() else { return };
        for read in reads {
            // State reads can number in the hundreds: they're deduplicated once, when the memo ends.
            if matches!(read, Read::State(..)) || !frame.contains(read) {
                frame.push(read.clone());
            }
        }
    }
}

thread_local! {
    static SCOPES: RefCell<Vec<Scope>> = const { RefCell::new(Vec::new()) };
}

/// Makes `cache` the one [`memo`] uses until [`Guard::finish`], for one view build.
/// `caret` and `layout` are what the view's `Cx` returns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn enter(
    mut cache: MemoCache,
    env: crate::Env,
    accent: crate::SystemAccent,
    reduced_motion: bool,
    reduced_transparency: bool,
    languages_gen: u64,
    srgb_blend: bool,
    chrome: [u32; 5],
    caret: Option<(String, [f32; 4])>,
    layout: LayoutRects,
) -> Guard {
    cache.pass += 1;
    cache.pure_counts.clear();
    let accent = [accent.accent(env.system_dark), accent.focus(env.system_dark)].map(|c| [c.r, c.g, c.b, c.a].map(f32::to_bits));
    let scale = env.scale;
    // Theme colours resolve per blend mode (`theme::blends_srgb`), so a switch rebuilds memos.
    // So does a change to what the window's controls take (`Cx::chrome_insets`), to the icon
    // swap (`text::set_icon_swap_size`), and to overridden languages (`Runtime::set_languages`).
    let icons = crate::text::icon::swap_state();
    let env = hash_of(&(env.system_dark, scale.to_bits(), reduced_motion, reduced_transparency, languages_gen, accent, crate::platform::scroll_bars::scoped(), srgb_blend, chrome, icons));
    let scope = Scope {
        cache,
        env,
        scale,
        virtual_row: 0,
        path: Vec::new(),
        nested: Vec::new(),
        reads: Vec::new(),
        seen: Seen { caret, layout },
        replayed: Replayed::default(),
    };
    SCOPES.with_borrow_mut(|s| s.push(scope));
    Guard(true)
}

pub(crate) fn virtual_row_tag(list_path: &str, row_key: &str) -> u64 {
    hash_of(&(list_path, row_key)).max(1)
}

struct VirtualRowGuard(Option<u64>);

impl Drop for VirtualRowGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.0 {
            SCOPES.with_borrow_mut(|scopes| {
                if let Some(s) = scopes.last_mut() {
                    s.virtual_row = prev;
                }
            });
        }
    }
}

fn enter_virtual_row(tag: u64) -> VirtualRowGuard {
    VirtualRowGuard(SCOPES.with_borrow_mut(|scopes| {
        scopes.last_mut().map(|s| std::mem::replace(&mut s.virtual_row, tag))
    }))
}

#[inline]
pub(crate) fn in_virtual_row<R>(tag: u64, f: impl FnOnce() -> R) -> R {
    let _reset = enter_virtual_row(tag);
    f()
}

/// [`enter`] for a commit that skips running `view` and builds only newly entering virtual rows,
/// keeping memos built in `view` (`virtual_row == 0`) or in rows `kept_rows` stays mounted on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn enter_partial(
    mut cache: MemoCache,
    kept_rows: &crate::runtime::HashSet<u64>,
    env: crate::Env,
    accent: crate::SystemAccent,
    reduced_motion: bool,
    reduced_transparency: bool,
    languages_gen: u64,
    srgb_blend: bool,
    chrome: [u32; 5],
    caret: Option<(String, [f32; 4])>,
    layout: LayoutRects,
) -> Guard {
    let next = cache.pass + 1;
    let mut nested = std::mem::take(&mut cache.nested_scratch);
    nested.clear();
    for e in cache.entries.values_mut() {
        if e.virtual_row == 0 || kept_rows.contains(&e.virtual_row) {
            e.pass = next;
            nested.extend(e.nested.iter().map(|id| (id.clone(), e.virtual_row)));
        }
    }
    if !nested.is_empty() {
        keep_nested_rows(&mut cache, &mut nested, next);
    }
    cache.nested_scratch = nested;
    enter(cache, env, accent, reduced_motion, reduced_transparency, languages_gen, srgb_blend, chrome, caret, layout)
}

/// Ends the view build [`enter`] began; dropped without [`Guard::finish`] (a panicking view),
/// it discards the cache.
pub(crate) struct Guard(bool);

impl Guard {
    /// The cache back, and the `Cx` reads of the memos this build reused. Unused memos stay
    /// until [`MemoCache::keep_since`], since a commit may build its view more than once.
    pub(crate) fn finish(mut self) -> (MemoCache, Replayed) {
        self.0 = false;
        let Scope { cache, replayed, .. } = SCOPES.with_borrow_mut(|s| s.pop()).expect("memo scope");
        (cache, replayed)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.0 {
            SCOPES.with_borrow_mut(|s| s.pop());
        }
    }
}

pub(crate) fn hash_of(v: &impl Hash) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    v.hash(&mut h);
    h.finish()
}

/// `el` keyed `key`; debug builds warn once per call site when it already had another key.
#[cfg_attr(debug_assertions, track_caller)]
fn keyed(el: El, key: String) -> El {
    #[cfg(debug_assertions)]
    if let Some(existing) = &el.key {
        crate::tooling::perf_warn::memo_key_replaced(&key, existing, std::panic::Location::caller());
    }
    el.key(key)
}

fn bump_pass() {
    SCOPES.with_borrow_mut(|s| {
        if let Some(s) = s.last_mut() {
            s.cache.pass += 1;
        }
    });
}

/// Runs `f` inside the view build in progress as if it were the next build, and the rest of the
/// build carries on as that one: memos used so far may be used once more, as the following frame
/// would use them. For tooling that measures what a rebuild costs a frame: a surface rebuilt
/// many times in one build to time it, its unchanged inner memos reused each time. Outside a
/// view build it just runs `f`.
#[inline]
pub fn as_next_build<R>(f: impl FnOnce() -> R) -> R {
    bump_pass();
    f()
}

/// The subtree `build` returns, keyed `id`, built again only when `version` (or the window's
/// environment) differs from the last build's. Otherwise `build` isn't called and the
/// previous subtree is reused, with the same node ids and state.
///
/// ```no_run
/// # use rune::{El, GrowthSpec, ScrollSpec, col, memo, text};
/// # struct Message { id: String, revision: u64, body: String }
/// # struct App { messages: Vec<Message>, theme: u64 }
/// # fn message_view(m: &Message) -> El { text(&m.body) }
/// # fn view(s: &App) -> El {
/// col().scroll(ScrollSpec { stick_bottom: true }).grow(1.0).child(
///     col().key("rows").growth(GrowthSpec::default()).virtual_rows(
///         s.messages.len(),
///         72.0,
///         |s: &App, i| s.messages[i].id.as_str().into(),
///         |s: &App, i| {
///             let m = &s.messages[i];
///             memo(format!("msg-{}", m.id), (m.revision, s.theme), || message_view(m))
///         },
///     ),
/// )
/// # }
/// ```
///
/// Anything the closure reads that can change belongs in `version`, apart from `Cx::layout`,
/// `Cx::layout_with`, `Cx::layout_size` and `Cx::caret`, which are tracked; see the
/// [module docs](mod@crate::memo) for handlers, live values and what else invalidates. Outside a view (a test calling a view function directly)
/// it just builds.
#[inline]
#[cfg_attr(debug_assertions, track_caller)]
pub fn memo(id: impl Into<String>, version: impl Hash, build: impl FnOnce() -> El) -> El {
    memo_with_hash(id, hash_of(&version), build)
}

/// [`memo`] with a version already hashed to a `u64`, for a caller that keeps its own
/// hashes (a plugin across a C ABI, where `impl Hash` can't cross). The window's environment
/// still joins it, as it does `memo`'s.
#[inline]
#[cfg_attr(debug_assertions, track_caller)]
pub fn memo_with_hash(id: impl Into<String>, version: u64, build: impl FnOnce() -> El) -> El {
    let mut build = Some(build);
    memo_inner(id.into(), version, false, &mut || build.take().expect("memo builds once")())
}

/// A memo, or with `region` a [`region`]: a region's reads of [`Tracked`](crate::Tracked)
/// values join its reads. A memo's don't: its version says when it builds again, as it always
/// has, so a cell that moves every frame (a clock) it reads doesn't rebuild it.
#[cfg_attr(debug_assertions, track_caller)]
fn memo_inner(key: String, version: u64, region: bool, build: &mut dyn FnMut() -> El) -> El {
    enum Step {
        Reuse(Rc<Built>),
        Build { id: Rc<str>, version: u64, store: bool },
        Plain,
    }
    let step = SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return Step::Plain };
        let version = hash_of(&(version, s.env));
        let name: &str = if let Some(p) = s.path.last() {
            s.cache.id_scratch.clear();
            s.cache.id_scratch.reserve(p.len() + 1 + key.len());
            s.cache.id_scratch.push_str(p);
            s.cache.id_scratch.push('/');
            s.cache.id_scratch.push_str(&key);
            &s.cache.id_scratch
        } else {
            &key
        };
        let pass = s.cache.pass;
        let has_parent = !s.nested.is_empty();
        let mut reused = None;
        let mut store = true;
        if let Some(e) = s.cache.entries.get_mut(name) {
            if e.pass == pass {
                store = false;
            } else if { report_miss(&key, e.version == version, &e.reads, &s.seen); e.version == version && e.reads.iter().all(|r| s.seen.holds(r)) } {
                e.pass = pass;
                e.virtual_row = s.virtual_row;
                for read in &e.reads {
                    match read {
                        Read::Caret(..) => s.replayed.caret = true,
                        Read::Layout(k, _) => note_read(&mut s.replayed.layout, k, None),
                        Read::LayoutWith(k, same, _) => note_read(&mut s.replayed.layout, k, Some(same.clone())),
                        Read::AnchorPlacement(k, _) => note_read(&mut s.replayed.layout, &anchor_key(k), None),
                        Read::Selection(_) => note_read(&mut s.replayed.layout, SELECTION, None),
                        Read::FieldSelection(k, _) => note_read(&mut s.replayed.layout, &field_selection_key(k), None),
                        Read::State(..) => {}
                    }
                }
                let parent_reads = if s.reads.is_empty() || e.reads.is_empty() { Vec::new() } else { e.reads.clone() };
                s.cache.nested_scratch.extend(e.nested.iter().map(|id| (id.clone(), s.virtual_row)));
                reused = Some((has_parent.then(|| e.id.clone()), e.built.clone(), parent_reads));
            }
        }
        if let Some((parent_id, built, reads)) = reused {
            if !s.cache.nested_scratch.is_empty() {
                let mut scratch = std::mem::take(&mut s.cache.nested_scratch);
                keep_nested_rows(&mut s.cache, &mut scratch, pass);
                s.cache.nested_scratch = scratch;
            }
            if !reads.is_empty() {
                s.note(&reads);
            }
            if let (Some(parent), Some(id)) = (s.nested.last_mut(), parent_id) {
                parent.push(id);
            }
            return Step::Reuse(built);
        }
        let id: Rc<str> = name.into();
        if let Some(parent) = s.nested.last_mut() {
            parent.push(id.clone());
        }
        s.path.push(id.clone());
        s.nested.push(Vec::new());
        s.reads.push(Vec::new());
        Step::Build { id, version, store }
    });
    let (id, version, store) = match step {
        Step::Reuse(built) => return built.placeholder(Some(key)),
        Step::Plain => return keyed(build(), key),
        Step::Build { id, version, store } => (id, version, store),
    };
    #[cfg(debug_assertions)]
    if !store {
        crate::tooling::perf_warn::memo_id_reused(&id, std::panic::Location::caller());
    }
    // A region's `Tracked` reads join its reads; a memo's closure records none of its own (a
    // region inside it still does, and they reach it as any nested read does).
    let (el, states) = if region {
        crate::tracked::begin();
        let el = keyed(build(), key);
        (el, crate::tracked::end())
    } else {
        let paused = crate::tracked::pause();
        let el = keyed(build(), key);
        crate::tracked::resume(paused);
        (el, Vec::new())
    };
    SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return el };
        s.path.pop();
        let nested = s.nested.pop().unwrap_or_default();
        let mut reads = s.reads.pop().unwrap_or_default();
        reads.extend(states.into_iter().map(|(id, version)| Read::State(id, version)));
        dedup_states(&mut reads);
        s.note(&reads);
        #[cfg(test)]
        {
            s.cache.builds += 1;
        }
        if !store {
            return el;
        }
        let (built, key) = Built::new(el, s.scale, region);
        let el = built.placeholder(key.clone());
        let pass = s.cache.pass;
        let virtual_row = s.virtual_row;
        let entry_id = id.clone();
        s.cache.entries.insert(id, Entry { id: entry_id, version, built, key, reads, nested, pass, virtual_row });
        el
    })
}

/// With `RUNE_REGION_MISS` set, prints what made a memo or region build again: the first read
/// that no longer holds (`state <cell> v<version>` for a [`Tracked`](crate::Tracked) value,
/// `layout <key>`, `caret <key>`, `selection`), or `version` when its version changed. For
/// finding what keeps a region rebuilding; `Tracked::id` names the cells.
fn report_miss(key: &str, same_version: bool, reads: &[Read], seen: &Seen) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("RUNE_REGION_MISS").is_some()) { return; }
    if !same_version { return eprintln!("rune-miss {key} version"); }
    let Some(r) = reads.iter().find(|r| !seen.holds(r)) else { return };
    let why = match r {
        Read::State(id, v) => format!("state {id} v{v}"),
        Read::Layout(k, _) | Read::LayoutWith(k, _, _) => format!("layout {k}"),
        Read::Caret(k, _) => format!("caret {k}"),
        Read::AnchorPlacement(k, _) => format!("anchor_placement {k}"),
        Read::Selection(_) => "selection".into(),
        Read::FieldSelection(k, _) => format!("field_selection {k}"),
    };
    eprintln!("rune-miss {key} {why}");
}

#[doc(hidden)]
pub fn digest(input: &(impl Hash + ?Sized)) -> (u64, u64) {
    let h1 = hash_of(&input);
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut h2);
    (h1, h2.finish())
}

/// Caches a pure-data subtree by a 128-bit [`digest`] and its occurrence ordinal in the
/// current view pass (or `virtual_rows` row), preserving `el.key` (`None` when unkeyed).
#[inline]
#[doc(hidden)]
pub fn cached(digest: (u64, u64), build: impl FnOnce() -> El) -> El {
    let mut build = Some(build);
    cached_inner(digest, &mut || build.take().expect("cached builds once")())
}

fn cached_inner((h1, h2): (u64, u64), build: &mut dyn FnMut() -> El) -> El {
    enum Step {
        Reuse(Rc<Built>, Option<String>),
        Build { id: Rc<str>, version: u64 },
        Plain,
    }
    let step = SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return Step::Plain };
        let occ = *s
            .cache
            .pure_counts
            .entry((s.virtual_row, h1, h2))
            .and_modify(|c| *c += 1)
            .or_insert(0);
        let version = hash_of(&(h1, h2, s.env));
        {
            use std::fmt::Write;
            s.cache.id_scratch.clear();
            if let Some(p) = s.path.last() {
                s.cache.id_scratch.push_str(p);
                s.cache.id_scratch.push('/');
            }
            write!(s.cache.id_scratch, "\0{}:{h1:016x}:{h2:016x}:{occ}", s.virtual_row).ok();
        }
        let pass = s.cache.pass;
        let has_parent = !s.nested.is_empty();
        let mut reused = None;
        if let Some(e) = s.cache.entries.get_mut(s.cache.id_scratch.as_str())
            && e.pass != pass
            && e.version == version
            && e.reads.iter().all(|r| s.seen.holds(r))
        {
            e.pass = pass;
            e.virtual_row = s.virtual_row;
            for read in &e.reads {
                match read {
                    Read::Caret(..) => s.replayed.caret = true,
                    Read::Layout(k, _) => note_read(&mut s.replayed.layout, k, None),
                    Read::LayoutWith(k, same, _) => note_read(&mut s.replayed.layout, k, Some(same.clone())),
                    Read::AnchorPlacement(k, _) => note_read(&mut s.replayed.layout, &anchor_key(k), None),
                    Read::Selection(_) => note_read(&mut s.replayed.layout, SELECTION, None),
                    Read::FieldSelection(k, _) => note_read(&mut s.replayed.layout, &field_selection_key(k), None),
                    Read::State(..) => {}
                }
            }
            let parent_reads = if s.reads.is_empty() || e.reads.is_empty() { Vec::new() } else { e.reads.clone() };
            s.cache.nested_scratch.extend(e.nested.iter().map(|id| (id.clone(), s.virtual_row)));
            reused = Some((has_parent.then(|| e.id.clone()), e.built.clone(), e.key.clone(), parent_reads));
        }
        if let Some((parent_id, built, key, reads)) = reused {
            if !s.cache.nested_scratch.is_empty() {
                let mut scratch = std::mem::take(&mut s.cache.nested_scratch);
                keep_nested_rows(&mut s.cache, &mut scratch, pass);
                s.cache.nested_scratch = scratch;
            }
            if !reads.is_empty() {
                s.note(&reads);
            }
            if let (Some(parent), Some(id)) = (s.nested.last_mut(), parent_id) {
                parent.push(id);
            }
            return Step::Reuse(built, key);
        }
        let id: Rc<str> = s.cache.id_scratch.as_str().into();
        if let Some(parent) = s.nested.last_mut() {
            parent.push(id.clone());
        }
        s.path.push(id.clone());
        s.nested.push(Vec::new());
        s.reads.push(Vec::new());
        Step::Build { id, version }
    });
    let (id, version) = match step {
        Step::Reuse(built, key) => return built.placeholder(key),
        Step::Plain => return build(),
        Step::Build { id, version } => (id, version),
    };
    let el = build();
    SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return el };
        s.path.pop();
        let nested = s.nested.pop().unwrap_or_default();
        let reads = s.reads.pop().unwrap_or_default();
        s.note(&reads);
        #[cfg(test)]
        {
            s.cache.builds += 1;
        }
        let (built, key) = Built::new(el, s.scale, false);
        let el = built.placeholder(key.clone());
        let pass = s.cache.pass;
        let virtual_row = s.virtual_row;
        let entry_id = id.clone();
        s.cache.entries.insert(id, Entry { id: entry_id, version, built, key, reads, nested, pass, virtual_row });
        el
    })
}

/// Marks the memos built inside a reused one as used, so they survive to its next build.
fn keep_nested_rows(cache: &mut MemoCache, ids: &mut Vec<(Rc<str>, u64)>, pass: u64) {
    while let Some((id, virtual_row)) = ids.pop() {
        if let Some(e) = cache.entries.get_mut(&id)
            && e.pass != pass
        {
            e.pass = pass;
            e.virtual_row = virtual_row;
            ids.extend(e.nested.iter().map(|nested| (nested.clone(), virtual_row)));
        }
    }
}

/// Whether the memo built as `id` is in the last commit's nodes, so its subtree can be reused.
pub(crate) fn flattened(id: u64) -> bool {
    SCOPES.with_borrow(|scopes| scopes.last().is_some_and(|s| s.cache.flattened.contains_key(&id)))
}

/// Marks the memos `virtual_rows` rows tagged `tags` built as used this pass, with the memos
/// built inside them: a list kept whole builds none of its rows, and they stay for when it does.
pub(crate) fn keep_rows(tags: impl Iterator<Item = u64>) {
    SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return };
        let tags: crate::runtime::HashSet<u64> = tags.collect();
        let pass = s.cache.pass;
        let mut nested = std::mem::take(&mut s.cache.nested_scratch);
        nested.clear();
        for e in s.cache.entries.values_mut().filter(|e| tags.contains(&e.virtual_row)) {
            e.pass = pass;
            nested.extend(e.nested.iter().map(|id| (id.clone(), e.virtual_row)));
        }
        keep_nested_rows(&mut s.cache, &mut nested, pass);
        s.cache.nested_scratch = nested;
    });
}
