//! Measuring text from a view or setup callback, for code that places things itself (table
//! columns, diagram nodes). While a window's or overlay's `setup` or view runs, its measures use
//! the window's own text system (`lend`).
//! Elsewhere each thread keeps one of its own, resolved in order: an active [`with_families`] or
//! [`with_fonts`] guard on this thread, else the fonts and families this thread's own most recent
//! [`TextSystem`] was given, else the process-wide ones from the latest unguarded [`TextSystem`]
//! (so worker threads that never build a [`TextSystem`] still inherit the app's `AppSpec::fonts`
//! and follow `Tx::set_font_families`). Same shaper, same fonts, same numbers.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{InlineBox, InlineBoxRun, RichLayoutOptions, Span, TextSystem};

mod ink;
pub use ink::{TextInk, ink};
use rune_core::tok::TextStyle;

/// The primary font's `(ascent, descent)` per em for `style`, or `None` if unavailable.
#[doc(hidden)]
pub fn primary_extents(style: TextStyle) -> Option<(f32, f32)> {
    with_text(|t| t.primary_extents(style.family, style.weight, style.italic))
}

/// Printable-ASCII (`0x20..0x7f`) glyph advances in points at `scale = 1.0`, the widest advance,
/// and whether the resolved font face is fixed-pitch; `None` if printable ASCII spans more than
/// one font face. Reuses the lent window `TextSystem`'s cached `AsciiLine`.
#[doc(hidden)]
pub fn ascii_metrics(style: TextStyle) -> Option<([f32; 95], f32, bool)> {
    with_text(|t| t.ascii_line(style, 1.0, false).map(|a| (a.advance, a.widest, a.fixed_pitch)))
}

struct Fonts {
    /// Bumped by any change, of fonts, families, or preferred emoji family.
    generation: u64,
    /// The `generation` that last changed the fonts.
    fonts_generation: u64,
    fonts: Vec<&'static [u8]>,
    families: (String, String),
    emoji_family: Option<String>,
    /// Zero for the process-wide state; a thread's own [`THREAD_FONTS`] or a [`with_families`] /
    /// [`with_fonts`] scope's own non-zero number otherwise.
    scope: u64,
}

static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
/// Lock-free mirror of the global (non-overridden, non-thread-local) font epoch so [`font_epoch`]
/// never acquires `FONTS` on the render hot path.
static GLOBAL_EPOCH: AtomicU64 = AtomicU64::new(0);
static SCOPES: AtomicU64 = AtomicU64::new(1);
/// Process-wide counter for `Fonts::fonts_generation` so a [`with_families`] or [`with_fonts`]
/// scope (or another thread's `TextSystem`) that loads different fonts never collides with the
/// global state's or another scope's font generation.
static FONTS_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_fonts_generation() -> u64 {
    FONTS_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Layouts kept between evictions (each eviction keeps those used since the one before).
const EVICT_EVERY: u32 = 512;

struct Measurer {
    generation: u64,
    fonts_generation: u64,
    text: TextSystem,
    calls: u32,
}

thread_local! {
    static MEASURER: RefCell<Option<Measurer>> = const { RefCell::new(None) };
    /// This thread's [`with_families`] or [`with_fonts`] state: a private copy that notes land in.
    static OVERRIDE: RefCell<Option<Fonts>> = const { RefCell::new(None) };
    /// The fonts and families this thread's own [`TextSystem`]s were built or updated with.
    static THREAD_FONTS: RefCell<Option<Fonts>> = const { RefCell::new(None) };
    /// The text system of the window or overlay whose setup or view is running on this thread (`lend`), if any.
    static LENT: RefCell<Option<TextSystem>> = const { RefCell::new(None) };
}

/// Trims this thread's fallback measurement `TextSystem` (when one was created) at rest.
pub(crate) fn rest() {
    MEASURER.try_with(|cell| {
        if let Ok(mut slot) = cell.try_borrow_mut()
            && let Some(m) = slot.as_mut()
        {
            m.text.rest();
        }
    }).ok();
}

/// Runs `f` (a window's or overlay's `setup` or view) with [`measure`] and the rest using
/// `text`, the runtime's own text system, rather than this thread's copy. The copy loads every
/// font and fallback again (a title with one Arabic or emoji character costs 6-30 ms its first
/// time), and its shapes aren't the ones the window lays out. Moves `text` into thread-local
/// storage for the duration of `f` and restores it on return or unwind.
#[doc(hidden)]
pub fn lend<R>(text: &mut TextSystem, f: impl FnOnce() -> R) -> R {
    struct Restore<'a> {
        dest: &'a mut TextSystem,
        previous: Option<TextSystem>,
    }
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            let previous = self.previous.take();
            let returned = LENT.with_borrow_mut(|slot| std::mem::replace(slot, previous));
            if let Some(lent) = returned {
                *self.dest = lent;
            }
        }
    }
    let previous = LENT.with_borrow_mut(|slot| slot.replace(std::mem::replace(text, TextSystem::empty())));
    let _restore = Restore { dest: text, previous };
    f()
}

fn with_global<R>(f: impl FnOnce(&mut Option<Fonts>) -> R) -> R {
    match FONTS.lock() {
        Ok(mut slot) => {
            let before = slot.as_ref().map(epoch_of);
            let r = f(&mut slot);
            if let Some(after) = slot.as_ref().map(epoch_of) && Some(after) != before {
                GLOBAL_EPOCH.store(after, Ordering::Release);
            }
            r
        }
        Err(_) => f(&mut None),
    }
}

/// Runs `f` on the effective state this thread measures with: its override when a
/// [`with_families`] or [`with_fonts`] guard is alive, else this thread's own [`TextSystem`]
/// state once it has built one, else the process-wide one.
fn with_state<R>(f: impl FnOnce(&mut Option<Fonts>) -> R) -> R {
    if OVERRIDE.with_borrow(Option::is_some) {
        return OVERRIDE.with_borrow_mut(|slot| f(slot));
    }
    if THREAD_FONTS.with_borrow(Option::is_some) {
        return THREAD_FONTS.with_borrow_mut(|slot| f(slot));
    }
    with_global(f)
}

fn ensure_fonts(slot: &mut Option<Fonts>) -> &mut Fonts {
    slot.get_or_insert_with(|| Fonts {
        generation: 1,
        fonts_generation: next_fonts_generation(),
        fonts: Vec::new(),
        families: ("Inter".to_owned(), "ui-monospace".to_owned()),
        emoji_family: None,
        scope: 0,
    })
}

fn apply_fonts(
    slot: &mut Option<Fonts>,
    fonts: &[&'static [u8]],
    families: (&str, &str),
    emoji_family: Option<&str>,
    preferred_fg: Option<u64>,
    new_scope: impl FnOnce() -> u64,
) -> u64 {
    let same_fonts = slot.as_ref().is_some_and(|f| f.fonts == fonts);
    if same_fonts
        && slot
            .as_ref()
            .is_some_and(|f| f.families.0 == families.0 && f.families.1 == families.1 && f.emoji_family.as_deref() == emoji_family)
    {
        return slot.as_ref().expect("same_fonts implies Some").fonts_generation;
    }
    let generation = slot.as_ref().map_or(1, |f| f.generation + 1);
    let fonts_generation = match slot.as_ref() {
        Some(f) if same_fonts => f.fonts_generation,
        _ => preferred_fg.unwrap_or_else(next_fonts_generation),
    };
    let scope = slot.as_ref().map_or_else(new_scope, |f| f.scope);
    let families = (families.0.to_owned(), families.1.to_owned());
    let emoji_family = emoji_family.map(str::to_owned);
    *slot = Some(Fonts { generation, fonts_generation, fonts: fonts.to_vec(), families, emoji_family, scope });
    fonts_generation
}

/// Called by every [`TextSystem::new`]; the next measurement uses these fonts.
pub(crate) fn note_fonts(fonts: &[&'static [u8]], families: (&str, &str)) {
    note_fonts_with_emoji(fonts, families, None);
}

/// Called by [`TextSystem::for_window`]; the next measurement uses these fonts, families, and
/// chosen emoji family without double-bumping `font_epoch`.
pub(crate) fn note_fonts_with_emoji(fonts: &[&'static [u8]], families: (&str, &str), emoji_family: Option<&str>) {
    if OVERRIDE.with_borrow(Option::is_some) {
        OVERRIDE.with_borrow_mut(|slot| {
            apply_fonts(slot, fonts, families, emoji_family, None, || 0);
        });
        return;
    }
    let global_fg = with_global(|slot| apply_fonts(slot, fonts, families, emoji_family, None, || 0));
    THREAD_FONTS.with_borrow_mut(|slot| {
        apply_fonts(slot, fonts, families, emoji_family, Some(global_fg), || SCOPES.fetch_add(1, Ordering::Relaxed));
    });
}

fn apply_families(slot: &mut Option<Fonts>, sans: &str, mono: &str) {
    let Some(f) = slot.as_mut() else { return };
    if f.families.0 != sans || f.families.1 != mono {
        f.generation += 1;
        f.families = (sans.to_owned(), mono.to_owned());
    }
}

/// Called by [`TextSystem::set_families`]; the next measurement uses these families.
pub(crate) fn note_families(sans: &str, mono: &str) {
    if OVERRIDE.with_borrow(Option::is_some) {
        OVERRIDE.with_borrow_mut(|slot| apply_families(slot, sans, mono));
        return;
    }
    THREAD_FONTS.with_borrow_mut(|slot| apply_families(slot, sans, mono));
    with_global(|slot| apply_families(slot, sans, mono));
}

fn apply_emoji_family(slot: &mut Option<Fonts>, emoji_family: Option<&str>) {
    let Some(f) = slot.as_mut() else { return };
    if f.emoji_family.as_deref() != emoji_family {
        f.generation += 1;
        f.emoji_family = emoji_family.map(str::to_owned);
    }
}

/// Called by [`TextSystem::set_emoji_family`]; the next measurement uses this emoji family.
pub(crate) fn note_emoji_family(emoji_family: Option<&str>) {
    if OVERRIDE.with_borrow(Option::is_some) {
        OVERRIDE.with_borrow_mut(|slot| apply_emoji_family(slot, emoji_family));
        return;
    }
    THREAD_FONTS.with_borrow_mut(|slot| apply_emoji_family(slot, emoji_family));
    with_global(|slot| {
        let f = ensure_fonts(slot);
        if f.emoji_family.as_deref() != emoji_family {
            f.generation += 1;
            f.emoji_family = emoji_family.map(str::to_owned);
        }
    });
}

/// Changes whenever the fonts, families, or preferred emoji family [`measure`] uses on this thread
/// change, as when an app calls [`crate::add_font`], `Tx::add_font`, `Tx::set_font_families`,
/// `Tx::set_emoji_family`, or this thread builds a [`TextSystem`] with different fonts. A cache of
/// measured sizes mixes it into its key.
///
/// A thread that has built its own [`TextSystem`] or holds a [`with_families`] or [`with_fonts`]
/// guard has an epoch in its own scope, distinct from the process-wide epoch and from other
/// guards, so caches keyed on it never mix scopes or invalidate on another thread's `TextSystem`.
pub fn font_epoch() -> u64 {
    let faces = crate::fonts::full_faces::count() as u64;
    let base = OVERRIDE.with_borrow(|o| match o {
        Some(f) => epoch_of(f),
        None => THREAD_FONTS.with_borrow(|t| match t {
            Some(f) => epoch_of(f),
            None => GLOBAL_EPOCH.load(Ordering::Acquire),
        }),
    });
    combine_face_epoch(base, faces)
}

fn epoch_of(f: &Fonts) -> u64 {
    if f.scope == 0 { f.generation } else { 1 << 63 | f.scope << 32 | (f.generation & 0xFFFF_FFFF) }
}

/// Mixes the process-wide runtime face count into `base` without colliding across scopes.
fn combine_face_epoch(base: u64, faces: u64) -> u64 {
    if base & (1 << 63) == 0 {
        base.wrapping_add(faces) & 0x7FFF_FFFF_FFFF_FFFF
    } else {
        (base & 0xFFFF_FFFF_0000_0000) | (base.wrapping_add(faces) & 0xFFFF_FFFF)
    }
}

/// Holds a thread-scoped family override; see [`with_families`].
#[must_use = "the override ends when the guard is dropped"]
pub struct FamiliesGuard {
    previous: Option<Fonts>,
    /// The override lives on one thread, so the guard must drop there.
    _not_send: PhantomData<*const ()>,
}

impl Drop for FamiliesGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = OVERRIDE.try_with(|o| *o.borrow_mut() = previous);
    }
}

/// Makes [`measure`], [`advance`], [`ink`] and [`font_epoch`] on this thread use `sans` and
/// `mono` (CSS family lists, as `Tx::set_font_families` takes them) for as long as the returned
/// guard lives, whatever the rest of the process has chosen. It is for tests: ones that switch
/// families run in parallel threads without seeing each other's.
///
/// ```
/// # use rune_core::tok::TextStyle;
/// # let style = TextStyle::new(14.0, 400.0);
/// let _families = rune_text::with_families("Inter", "IBM Plex Mono");
/// let (w, _) = rune_text::measure("let x = 1;", &[], style.mono(), None);
/// # assert!(w > 0.0);
/// ```
///
/// - The fonts are this thread's current ones (its active guard, else its own [`TextSystem`]'s
///   fonts, else the process-wide ones); only the families are replaced. Use [`with_fonts`] to
///   replace the fonts as well.
/// - Dropping the guard restores what was in force before it, so guards nest with each other and
///   with [`with_fonts`]. Drop them in the reverse order they were made.
/// - While it lives, anything on this thread that would change what `measure` uses lands in the
///   override and nowhere else: a headless window's `Tx::set_font_families` (or a
///   `TextSystem::new` or `set_families`) moves this thread's `measure` and `font_epoch`, and
///   touches neither this thread's unscoped `TextSystem` state nor any other thread's. The window
///   itself follows the families chosen for windows as it always does. Outside a guard, a switch
///   updates this thread and the process-wide fallback seen by threads that never built a
///   [`TextSystem`].
/// - `font_epoch` differs from the unscoped epoch, and from that of every other guard, so
///   caches keyed on it stay apart.
///
/// Prefer a guard over a closure so a test can hold it for its whole body, and it restores on
/// a panic.
pub fn with_families(sans: &str, mono: &str) -> FamiliesGuard {
    let scope = SCOPES.fetch_add(1, Ordering::Relaxed);
    let seeded = with_state(|slot| {
        let f = ensure_fonts(slot);
        Fonts {
            generation: f.generation + 1,
            fonts_generation: f.fonts_generation,
            fonts: f.fonts.clone(),
            families: (sans.to_owned(), mono.to_owned()),
            emoji_family: f.emoji_family.clone(),
            scope,
        }
    });
    let previous = OVERRIDE.with_borrow_mut(|o| o.replace(seeded));
    FamiliesGuard { previous, _not_send: PhantomData }
}

/// Holds a thread-scoped font and family override; see [`with_fonts`].
#[must_use = "the override ends when the guard is dropped"]
pub struct FontsGuard {
    previous: Option<Fonts>,
    /// The override lives on one thread, so the guard must drop there.
    _not_send: PhantomData<*const ()>,
}

impl Drop for FontsGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = OVERRIDE.try_with(|o| *o.borrow_mut() = previous);
    }
}

/// Makes [`measure`], [`advance`], [`ink`] and [`font_epoch`] on this thread use `fonts` and
/// `families` (the same arguments [`TextSystem::new`] takes: font slices replacing the built-in
/// Inter when non-empty, and CSS family lists for the sans and mono slots) for as long as the
/// returned guard lives, whatever the rest of the process has built. It is for tests and tools
/// that measure without building a [`TextSystem`], or that want a temporary font scope that
/// restores on return or panic.
///
/// ```
/// # use rune_core::tok::TextStyle;
/// # const MY_SANS: &[u8] = &[];
/// # const MY_MONO: &[u8] = &[];
/// # let style = TextStyle::new(14.0, 400.0);
/// let _fonts = rune_text::with_fonts(&[MY_SANS, MY_MONO], ("My Sans", "My Mono"));
/// let (w, _) = rune_text::measure("Hello", &[], style, None);
/// # assert!(w >= 0.0);
/// ```
///
/// - Dropping the guard restores what was in force before it, so guards nest with each other and
///   with [`with_families`]. Drop them in the reverse order they were made.
/// - While it lives, anything on this thread that would change what `measure` uses lands in the
///   override and nowhere else: a `TextSystem::new`, `set_families`, or headless window's
///   `Tx::set_font_families` moves this thread's `measure` and `font_epoch`, and touches neither
///   this thread's unscoped `TextSystem` state nor any other thread's.
/// - `font_epoch` differs from the unscoped epoch and from every other guard's, so caches keyed
///   on it stay apart.
pub fn with_fonts(fonts: &[&'static [u8]], families: (&str, &str)) -> FontsGuard {
    let scope = SCOPES.fetch_add(1, Ordering::Relaxed);
    let fonts_generation = with_state(|slot| match slot.as_ref() {
        Some(f) if f.fonts == fonts => f.fonts_generation,
        _ => next_fonts_generation(),
    });
    let seeded = Fonts {
        generation: 1,
        fonts_generation,
        fonts: fonts.to_vec(),
        families: (families.0.to_owned(), families.1.to_owned()),
        emoji_family: None,
        scope,
    };
    let previous = OVERRIDE.with_borrow_mut(|o| o.replace(seeded));
    FontsGuard { previous, _not_send: PhantomData }
}

/// The size in points of `text` (with `spans`, as `rune::rich` takes them) set in `style`,
/// wrapped at `max_width` points when given, as the runtime will lay it out. Widths round up
/// to a 64th of a point, as Chromium keeps a text run's width; heights are the lines' own, as a text lays out unless it asks for
/// `El::rounded_lines`. `max_width: Some(0.0)` gives the widest unbreakable run (CSS min-content).
///
/// The first call on a thread loads the fonts (a few milliseconds); later calls cost a hash
/// lookup when the same text was measured recently, or one shaping pass when not.
pub fn measure(text: &str, spans: &[Span], style: TextStyle, max_width: Option<f32>) -> (f32, f32) {
    measure_with(text, spans, style, max_width, RichLayoutOptions::default())
}

/// [`measure`] laid out with [`RichLayoutOptions`], so text that an `El` draws with
/// `wrap_anywhere` measures to the lines it will actually take:
/// `measure_with(text, &[], style, Some(w), RichLayoutOptions::new().wrap_anywhere())`.
pub fn measure_with(
    text: &str,
    spans: &[Span],
    style: TextStyle,
    max_width: Option<f32>,
    options: RichLayoutOptions,
) -> (f32, f32) {
    with_text(|t| {
        let l = t.layout_rich_with(text, spans, style, 1.0, max_width, options);
        // Spaces hanging past a wrapped line's end take no room, as the runtime measures them.
        let w = if max_width.is_some() { l.width() } else { l.full_width() };
        (crate::text_width(w), super::line_metrics::spec_height(&l, style, 1.0, true))
    })
}

/// Like [`measure`], reserving fixed-size in-flow [`InlineBox`] slots at UTF-8 byte offsets
/// `boxes`, matching `El::inline`.
///
/// ```
/// use rune_core::tok::TextStyle;
/// use rune_text::{InlineAlign, InlineBox, measure_inline};
///
/// let style = TextStyle::new(14.0, 400.0);
/// let slot = InlineBox::new(40.0, 16.0).align(InlineAlign::TextCenter);
/// let (w, h) = measure_inline("Open  and edit it.", &[], &[(5, slot)], style, Some(240.0));
/// assert!(w > 40.0 && h >= 16.0);
/// ```
pub fn measure_inline(
    text: &str,
    spans: &[Span],
    boxes: &[(usize, InlineBox)],
    style: TextStyle,
    max_width: Option<f32>,
) -> (f32, f32) {
    measure_inline_with(text, spans, boxes, style, max_width, RichLayoutOptions::default())
}

/// Like [`measure_inline`], laid out with [`RichLayoutOptions`] (alignment, `wrap_anywhere`,
/// base direction).
///
/// ```
/// use rune_core::tok::TextStyle;
/// use rune_text::{InlineBox, RichLayoutOptions, measure_inline_with};
///
/// let style = TextStyle::new(14.0, 400.0);
/// let slot = InlineBox::new(32.0, 16.0);
/// let (w, h) = measure_inline_with(
///     "Open  now",
///     &[],
///     &[(5, slot)],
///     style,
///     Some(120.0),
///     RichLayoutOptions::new().wrap_anywhere(),
/// );
/// assert!(w > 0.0 && h >= 16.0);
/// ```
pub fn measure_inline_with(
    text: &str,
    spans: &[Span],
    boxes: &[(usize, InlineBox)],
    style: TextStyle,
    max_width: Option<f32>,
    options: RichLayoutOptions,
) -> (f32, f32) {
    if boxes.is_empty() {
        return measure_with(text, spans, style, max_width, options);
    }
    let runs: Vec<InlineBoxRun> = boxes
        .iter()
        .map(|&(at, slot)| InlineBoxRun {
            at: at.min(u32::MAX as usize) as u32,
            slot,
            child: 0,
        })
        .collect();
    with_text(|t| {
        let l = t.layout_inline_with(text, spans, &runs, style, 1.0, max_width, options);
        let w = if max_width.is_some() { l.width() } else { l.full_width() };
        (crate::text_width(w), super::line_metrics::spec_height(&l, style, 1.0, true))
    })
}

/// CSS's min-content and max-content widths of `text` (with `spans`) in `style`, in points and
/// unrounded, as a browser's automatic table layout reads a cell: its widest unbreakable run,
/// and its width on one line, neither counting the spaces that end a line.
pub fn content_widths(text: &str, spans: &[Span], style: TextStyle) -> (f32, f32) {
    with_text(|t| t.content_widths(text, spans, style))
}

/// The advance in points of `text` set on one line in `style`, unrounded: how much wider a row
/// of `El::advance_in` cells gets for it, to within a device pixel.
pub fn advance(text: &str, style: TextStyle) -> f32 {
    with_text(|t| t.layout_rich(text, &[], style, 1.0, None).full_width())
}

/// Where each line of `text` starts (byte offsets), set in the sans family at `size` points and
/// `weight` and wrapped at `width` points, as the runtime lays it out: for a page's harness to
/// compare with the browser's own lines (`platform/web.rs`, `__runeLines`).
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub fn line_starts(text: &str, size: f32, weight: f32, width: f32) -> Vec<usize> {
    with_text(|t| {
        let l = t.layout_rich_with(text, &[], TextStyle::new(size, weight), 1.0, Some(width), RichLayoutOptions::default());
        l.lines().map(|line| line.text_range().start).collect()
    })
}

/// Runs `f` on this thread's text system, built from the fonts the app was given.
pub(super) fn with_text<R>(f: impl FnOnce(&mut TextSystem) -> R) -> R {
    // A view's measures use its window's text system, unless a `with_families` or `with_fonts`
    // guard asks for different ones. Borrowed in place; a re-entrant call sees the active borrow
    // and falls through to `MEASURER` instead of aliasing.
    let f = if OVERRIDE.with_borrow(Option::is_none) {
        match LENT.with(|cell| match cell.try_borrow_mut() {
            Ok(mut slot) => match slot.as_mut() {
                Some(text) => {
                    text.follow_full_faces();
                    Ok(f(text))
                }
                None => Err(f),
            },
            Err(_) => Err(f),
        }) {
            Ok(result) => return result,
            Err(f) => f,
        }
    } else {
        f
    };
    MEASURER.with_borrow_mut(|m| {
        let current = font_epoch();
        if m.as_ref().is_none_or(|m| m.generation != current) {
            let faces = crate::fonts::full_faces::count() as u64;
            let (base_epoch, fonts_generation, fonts, (sans, mono), emoji_family) = with_state(|slot| {
                let f = ensure_fonts(slot);
                (epoch_of(f), f.fonts_generation, f.fonts.clone(), f.families.clone(), f.emoji_family.clone())
            });
            let epoch = combine_face_epoch(base_epoch, faces);
            match m.as_mut() {
                // Only the families, emoji family, or runtime-added faces changed: the loaded fonts stay.
                Some(m) if m.fonts_generation == fonts_generation => {
                    m.text.follow_full_faces();
                    m.text.apply_families(&sans, &mono);
                    m.text.apply_emoji_family(emoji_family.as_deref());
                    m.generation = epoch;
                }
                _ => {
                    let mut text = TextSystem::new_local(&fonts, (&sans, &mono));
                    text.apply_emoji_family(emoji_family.as_deref());
                    *m = Some(Measurer { generation: epoch, fonts_generation, text, calls: 0 });
                }
            }
        }
        let m = m.as_mut().expect("set above");
        m.calls += 1;
        if m.calls % EVICT_EVERY == 0 {
            m.text.end_commit();
        }
        f(&mut m.text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_text_and_wraps() {
        let _guard = with_fonts(&[], ("Inter", "ui-monospace"));
        let style = TextStyle::new(14.0, 400.0);
        let (w, h) = measure("Hello world", &[], style, None);
        assert!(w > 50.0 && h >= 17.0, "{w} {h}");
        let (min, tall) = measure("Hello world", &[], style, Some(0.0));
        assert!(min < w && tall > h, "min-content breaks at every opportunity: {min} {tall}");
        let mut bold = Span::new(0, 5);
        bold.weight = Some(700.0);
        assert!(measure("Hello world", &[bold], style, None).0 > w);
    }

    #[test]
    fn content_widths_matches_cached_layouts_and_caches_nothing() {
        let style = TextStyle::new(14.0, 400.0);
        let mut mono = Span::new(0, 10);
        mono.mono = true;
        let mut bold = Span::new(11, 16);
        bold.weight = Some(700.0);
        let cases: &[(&str, &[Span])] = &[
            ("", &[]),
            ("Key", &[]),
            ("Verified invariant for row 42 in section 7", &[]),
            ("key_12_3   Title with spaces", &[mono, bold]),
            ("unbreakable_super_long_identifier_token", &[]),
            ("a\tb\tc", &[]),
        ];

        let mut text = TextSystem::default();
        let mut ref_text = TextSystem::default();
        for &(s, spans) in cases {
            let shaped0 = text.shaped;
            let got = lend(&mut text, || content_widths(s, spans, style));
            let want_min = ref_text.layout_rich_with(s, spans, style, 1.0, Some(0.0), RichLayoutOptions::default()).width();
            let want_max = ref_text.layout_rich_with(s, spans, style, 1.0, None, RichLayoutOptions::default()).width();
            assert_eq!(got.0.to_bits(), want_min.to_bits(), "min-content width mismatch for {s:?}: {got:?} vs ({want_min}, {want_max})");
            assert_eq!(got.1.to_bits(), want_max.to_bits(), "max-content width mismatch for {s:?}: {got:?} vs ({want_min}, {want_max})");
            assert_eq!(text.cached_layout_count(), 0, "content_widths must not insert into TextSystem::layouts for {s:?}");
            assert_eq!(text.shaped - shaped0, 1, "content_widths must shape once for {s:?}");
        }
    }

    #[test]
    fn wrap_anywhere_measures_a_long_token_onto_more_lines() {
        let _guard = with_fonts(&[], ("Inter", "ui-monospace"));
        let style = TextStyle::new(14.0, 400.0);
        let link = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let (plain_w, plain_h) = measure(link, &[], style, Some(120.0));
        let (w, h) = measure_with(link, &[], style, Some(120.0), RichLayoutOptions::new().wrap_anywhere());
        assert!(plain_w > 120.0, "without wrap_anywhere the token runs past the width: {plain_w}");
        assert!(w <= 120.0 && h > plain_h * 2.0, "{w} {h} vs {plain_w} {plain_h}");
        assert_eq!(measure(link, &[], style, Some(120.0)), measure_with(link, &[], style, Some(120.0), RichLayoutOptions::new()));
    }

    #[test]
    fn lend_uses_lent_text_system_and_restores_on_return_and_panic() {
        let style = TextStyle::new(14.0, 400.0);
        let mut text = TextSystem::default();
        assert_eq!(text.shaped, 0);

        let (w, h) = lend(&mut text, || measure("Lend normal return", &[], style, None));
        assert!(w > 50.0 && h >= 17.0, "{w} {h}");
        assert!(text.shaped > 0, "measure inside lend should shape on the lent TextSystem");
        let after_normal = text.shaped;

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lend(&mut text, || {
                let _ = measure("Lend panic unwind", &[], style, None);
                panic!("boom");
            });
        }));
        assert!(panicked.is_err());
        assert!(text.shaped > after_normal, "lent TextSystem must be restored intact after panic unwind");
        assert!(LENT.with_borrow(Option::is_none), "LENT slot must be cleared after unwind");
        // Subsequent measure and lend still work cleanly.
        let after_panic = text.shaped;
        let _ = lend(&mut text, || measure("Lend after panic", &[], style, None));
        assert!(text.shaped > after_panic);
    }

    #[test]
    fn lend_nests_and_restores_outer_on_return_and_panic() {
        let style = TextStyle::new(14.0, 400.0);
        let mut outer = TextSystem::default();
        let mut inner = TextSystem::default();

        lend(&mut outer, || {
            let _ = measure("Outer before", &[], style, None);
            lend(&mut inner, || {
                let _ = measure("Inner normal", &[], style, None);
            });
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lend(&mut inner, || {
                    let _ = measure("Inner panic", &[], style, None);
                    panic!("inner boom");
                });
            }));
            assert!(caught.is_err());
            let _ = measure("Outer after", &[], style, None);
        });

        assert!(outer.shaped >= 2, "outer should receive both outer measures: {}", outer.shaped);
        assert!(inner.shaped >= 2, "inner should receive both inner measures: {}", inner.shaped);
        assert!(LENT.with_borrow(Option::is_none));
    }

    #[test]
    fn with_text_reentrancy_and_override_bypass_lent() {
        let style = TextStyle::new(14.0, 400.0);
        let mut text = TextSystem::default();
        let mut saw_nested = false;

        lend(&mut text, || {
            with_text(|outer| {
                let before = outer.shaped;
                // Re-entrant call sees active RefCell borrow and falls back to MEASURER without panicking.
                let (w, _) = measure("Reentrant fallback", &[], style, None);
                assert!(w > 50.0);
                assert_eq!(outer.shaped, before, "re-entrant measure must not touch actively borrowed lent system");
                saw_nested = true;
            });
            // A `with_families` override bypasses LENT without disturbing it.
            let _guard = with_families("Inter", "IBM Plex Mono");
            let (w, _) = measure("Override bypass", &[], style, None);
            assert!(w > 50.0);
        });

        assert!(saw_nested);
        assert_eq!(text.shaped, 0, "neither re-entrant nor overridden measure should shape on lent text");
        assert!(LENT.with_borrow(Option::is_none));
    }

    #[test]
    fn with_text_does_not_tag_measurer_with_newer_fonts_generation() {
        const PLEX: &[u8] = include_bytes!("../../rune/assets/IBMPlexMono-Regular.ttf");
        let base = crate::fonts::default_fonts::DEFAULT_FONTS;
        let with_plex = [base[0], base[1], base[2], PLEX];
        let mono_style = TextStyle::new(14.0, 400.0).mono();
        let expected_plex = TextSystem::new(&with_plex, ("Inter", "IBM Plex Mono"))
            .layout_rich("iiiiiiiiii", &[], mono_style, 1.0, None)
            .full_width();

        // Race a thread building its measurer from default fonts against another thread registering
        // a new font set. The measurer must never be tagged with the newer fonts_generation while
        // still holding the older fonts.
        for round in 0..24 {
            note_fonts(&[], ("Inter", "Inter"));
            let start = std::sync::Arc::new(std::sync::Barrier::new(2));
            let done = std::sync::Arc::new(std::sync::Barrier::new(2));
            let writer_start = start.clone();
            let writer_done = done.clone();
            let writer = std::thread::spawn(move || {
                writer_start.wait();
                // Spin-wait (Windows sleep has coarse ~1-15ms granularity) so the update lands
                // right inside TextSystem construction across rounds.
                let delay = std::time::Duration::from_micros((round % 16) * 40);
                let t0 = rune_core::time::now();
                while t0.elapsed() < delay {
                    std::hint::spin_loop();
                }
                note_fonts(&with_plex, ("Inter", "IBM Plex Mono"));
                let fg = with_global(|f| f.as_ref().map(|f| f.fonts_generation)).unwrap_or(0);
                let ep = combine_face_epoch(
                    GLOBAL_EPOCH.load(Ordering::Acquire),
                    crate::fonts::full_faces::count() as u64,
                );
                writer_done.wait();
                (fg, ep)
            });

            let reader = std::thread::spawn(move || {
                start.wait();
                with_text(|_| {});
                done.wait();
                let snapshot = MEASURER.with_borrow_mut(|m| {
                    let m = m.as_mut().expect("measurer set");
                    (m.generation, m.fonts_generation, m.text.font_cx.collection.family_id("IBM Plex Mono").is_some())
                });
                let _guard = with_families("Inter", "IBM Plex Mono");
                let override_has_plex = with_state(|f| f.as_ref().is_some_and(|f| f.fonts == with_plex));
                let adv = advance("iiiiiiiiii", mono_style);
                let after_has_plex = with_text(|t| t.font_cx.collection.family_id("IBM Plex Mono").is_some());
                (snapshot, override_has_plex, adv, after_has_plex)
            });

            let (plex_fg, plex_epoch) = writer.join().unwrap();
            let ((m_gen, m_fg, before_has_plex), override_has_plex, adv, after_has_plex) = reader.join().unwrap();
            if !before_has_plex {
                assert_ne!(
                    m_fg, plex_fg,
                    "round {round}: measurer loaded older fonts but was tagged with newer fonts_generation {plex_fg}",
                );
                assert_ne!(
                    m_gen, plex_epoch,
                    "round {round}: measurer loaded older fonts but was tagged with newer epoch {plex_epoch}",
                );
            }
            if override_has_plex {
                assert!(after_has_plex, "round {round}: measurer kept stale fonts after with_families");
                assert_eq!(adv, expected_plex, "round {round}: advance measured stale fonts");
            }
        }
        note_fonts(&[], ("Inter", "ui-monospace"));
    }

    #[test]
    fn scoped_fonts_generations_do_not_collide_across_guards() {
        const PLEX: &[u8] = include_bytes!("../../rune/assets/IBMPlexMono-Regular.ttf");
        let base = crate::fonts::default_fonts::DEFAULT_FONTS;
        let with_plex = [base[0], base[1], base[2], PLEX];

        {
            let _g1 = with_families("Inter", "Inter");
            note_fonts(&with_plex, ("Inter", "IBM Plex Mono"));
            let has_plex = with_text(|t| t.font_cx.collection.family_id("IBM Plex Mono").is_some());
            assert!(has_plex);
        }
        {
            let _g2 = with_families("Inter", "Inter");
            note_fonts(&[base[0]], ("Inter", "Inter"));
            let has_plex = with_text(|t| t.font_cx.collection.family_id("IBM Plex Mono").is_some());
            assert!(!has_plex, "second guard loaded different fonts and must not reuse the first guard's measurer");
        }
    }

    #[test]
    fn thread_keeps_its_own_text_system_fonts_across_other_threads() {
        const PLEX: &[u8] = include_bytes!("../../rune/assets/IBMPlexMono-Regular.ttf");
        let style = TextStyle::new(14.0, 400.0);
        let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
        let done = std::sync::Arc::new(std::sync::Barrier::new(2));

        let r1 = ready.clone();
        let d1 = done.clone();
        let plex_thread = std::thread::spawn(move || {
            let _mine = TextSystem::new(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
            let before = measure("Last 12 weeks", &[], style, None).0;
            let epoch_before = font_epoch();
            r1.wait();
            d1.wait();
            let after = measure("Last 12 weeks", &[], style, None).0;
            let epoch_after = font_epoch();
            (before, after, epoch_before, epoch_after)
        });

        let inter_thread = std::thread::spawn(move || {
            ready.wait();
            let mut other = TextSystem::default();
            let inter_w = measure("Last 12 weeks", &[], style, None).0;
            let other_epoch = font_epoch();
            other.set_families("Inter", "Inter");
            let after_switch = measure("Last 12 weeks", &[], style, None).0;
            let switched_epoch = font_epoch();
            done.wait();
            (inter_w, after_switch, other_epoch, switched_epoch)
        });

        let (before, after, epoch_before, epoch_after) = plex_thread.join().unwrap();
        let (inter_w, after_switch, other_epoch, switched_epoch) = inter_thread.join().unwrap();
        assert_ne!(before, inter_w, "custom font and default Inter must measure differently");
        assert_eq!(before, after, "another thread's TextSystem changed this thread's measure");
        assert_eq!(epoch_before, epoch_after, "another thread's TextSystem changed this thread's font_epoch");
        assert_eq!(inter_w, after_switch, "other thread kept measuring in Inter");
        assert_ne!(other_epoch, switched_epoch, "other thread's set_families moved its own font_epoch");
    }

    #[test]
    fn thread_fonts_preserve_scope_and_seed_with_families() {
        const PLEX: &[u8] = include_bytes!("../../rune/assets/IBMPlexMono-Regular.ttf");
        let base = crate::fonts::default_fonts::DEFAULT_FONTS;
        let with_plex = [base[0], base[1], base[2], PLEX];
        let mono_style = TextStyle::new(14.0, 400.0).mono();

        std::thread::spawn(move || {
            let _ts1 = TextSystem::new(&with_plex, ("Inter", "Inter"));
            let inter_w = measure("iiiiiiiiii", &[], mono_style, None).0;
            let (scope1, fg1) = THREAD_FONTS.with_borrow(|t| {
                let f = t.as_ref().expect("THREAD_FONTS set");
                (f.scope, f.fonts_generation)
            });
            assert_ne!(scope1, 0, "THREAD_FONTS must get a non-zero scope");

            // Another thread overwrites process-wide FONTS with defaults (no IBM Plex Mono).
            std::thread::spawn(|| drop(TextSystem::default())).join().unwrap();

            // Re-noting the same font slices with new families preserves scope and fonts_generation
            // so MEASURER updates families without rebuilding its TextSystem.
            let _ts2 = TextSystem::new(&with_plex, ("Inter", "IBM Plex Mono"));
            let (scope2, fg2) = THREAD_FONTS.with_borrow(|t| {
                let f = t.as_ref().expect("THREAD_FONTS set");
                (f.scope, f.fonts_generation)
            });
            assert_eq!(scope1, scope2, "THREAD_FONTS keeps its scope across rebuilds");
            assert_eq!(fg1, fg2, "matching font slices reuse fonts_generation");
            let plex_w = measure("iiiiiiiiii", &[], mono_style, None).0;
            assert_ne!(plex_w, inter_w, "same thread's new families took effect");

            // with_families seeds from this thread's THREAD_FONTS (still containing PLEX), not global FONTS.
            let _fam = with_families("Inter", "IBM Plex Mono");
            let has_plex = with_text(|t| t.font_cx.collection.family_id("IBM Plex Mono").is_some());
            assert!(has_plex, "with_families must preserve this thread's fonts");
            assert_eq!(measure("iiiiiiiiii", &[], mono_style, None).0, plex_w);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn with_fonts_isolates_nests_traps_and_restores() {
        const PLEX: &[u8] = include_bytes!("../../rune/assets/IBMPlexMono-Regular.ttf");
        let base = crate::fonts::default_fonts::DEFAULT_FONTS;
        let with_plex = [base[0], base[1], base[2], PLEX];
        let style = TextStyle::new(14.0, 400.0);
        let mono_style = style.mono();

        let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
        let done = std::sync::Arc::new(std::sync::Barrier::new(2));
        let r1 = ready.clone();
        let d1 = done.clone();

        // Thread that never built a TextSystem: with_fonts isolates it from another thread's
        // TextSystem::new + set_families, nests with with_families in both orders, traps notes,
        // and restores on drop and panic unwind.
        let scoped = std::thread::spawn(move || {
            let g_plex = with_fonts(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
            let plex_w = measure("Last 12 weeks", &[], style, None).0;
            let plex_epoch = font_epoch();
            let plex_fg = OVERRIDE.with_borrow(|o| o.as_ref().unwrap().fonts_generation);

            r1.wait();
            d1.wait();
            assert_eq!(measure("Last 12 weeks", &[], style, None).0, plex_w, "with_fonts isolated from concurrent TextSystem::new");
            assert_eq!(font_epoch(), plex_epoch, "with_fonts epoch isolated from concurrent TextSystem::new");
            assert!(THREAD_FONTS.with_borrow(Option::is_none), "with_fonts must not populate THREAD_FONTS");

            // Same font slices reuse fonts_generation.
            {
                let _same = with_fonts(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
                let same_fg = OVERRIDE.with_borrow(|o| o.as_ref().unwrap().fonts_generation);
                assert_eq!(same_fg, plex_fg, "identical font slices reuse fonts_generation");
                assert_ne!(font_epoch(), plex_epoch, "nested with_fonts gets its own scope epoch");
            }
            assert_eq!(font_epoch(), plex_epoch);

            // Order 1: with_fonts outer, with_families inner.
            {
                let _outer = with_fonts(&with_plex, ("Inter", "Inter"));
                let inter_mono = measure("iiiiiiiiii", &[], mono_style, None).0;
                let outer_epoch = font_epoch();
                {
                    let _inner = with_families("Inter", "IBM Plex Mono");
                    let plex_mono = measure("iiiiiiiiii", &[], mono_style, None).0;
                    assert_ne!(plex_mono, inter_mono, "inner with_families uses outer with_fonts' fonts");
                    assert_ne!(font_epoch(), outer_epoch);
                }
                assert_eq!(measure("iiiiiiiiii", &[], mono_style, None).0, inter_mono);
                assert_eq!(font_epoch(), outer_epoch);
            }

            // Order 2: with_families outer, with_fonts inner.
            {
                let _outer = with_families("IBM Plex Mono", "IBM Plex Mono");
                let outer_epoch = font_epoch();
                {
                    let _inner = with_fonts(&[], ("Inter", "Inter"));
                    let inter_w = measure("Last 12 weeks", &[], style, None).0;
                    assert_ne!(inter_w, plex_w, "inner with_fonts replaces outer's fonts");
                    assert_ne!(font_epoch(), outer_epoch);
                }
                assert_eq!(measure("Last 12 weeks", &[], style, None).0, plex_w);
                assert_eq!(font_epoch(), outer_epoch);
            }

            // Traps note_fonts and note_families while live, restoring on drop and panic unwind.
            {
                let _trap = with_fonts(&with_plex, ("Inter", "Inter"));
                let trapped_before = measure("iiiiiiiiii", &[], mono_style, None).0;
                let mut ts = TextSystem::new(&with_plex, ("Inter", "Inter"));
                ts.set_families("Inter", "IBM Plex Mono");
                assert_ne!(measure("iiiiiiiiii", &[], mono_style, None).0, trapped_before);
                assert!(THREAD_FONTS.with_borrow(Option::is_none), "trapped note_fonts must not touch THREAD_FONTS");
            }
            assert_eq!(measure("Last 12 weeks", &[], style, None).0, plex_w);
            assert_eq!(font_epoch(), plex_epoch);

            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _panic_guard = with_fonts(&[], ("Inter", "Inter"));
                assert_ne!(measure("Last 12 weeks", &[], style, None).0, plex_w);
                panic!("unwind restores with_fonts");
            }));
            assert!(panicked.is_err());
            assert_eq!(measure("Last 12 weeks", &[], style, None).0, plex_w);
            assert_eq!(font_epoch(), plex_epoch);

            drop(g_plex);
            assert!(OVERRIDE.with_borrow(Option::is_none));
        });

        let concurrent = std::thread::spawn(move || {
            ready.wait();
            let mut ts = TextSystem::default();
            ts.set_families("Inter", "Inter");
            done.wait();
        });

        scoped.join().unwrap();
        concurrent.join().unwrap();
    }

    #[test]
    fn default_measure_and_with_fonts_load_no_system_fonts() {
        let sans = TextStyle::new(13.0, 400.0);

        std::thread::spawn(move || {
            let _ts = TextSystem::default();
            let _ = measure("Hello world 0123", &[], sans, None);
            MEASURER.with_borrow(|m| {
                let m = m.as_ref().expect("measurer built");
                assert!(m.text.system_mono.is_none(), "sans-only measure must not load system monospace fonts");
                assert!(m.text.font_files.is_empty(), "sans-only measure must not open system font files");
                assert!(m.text.installed.is_empty(), "sans-only measure must not query installed fonts");
            });
        })
        .join()
        .unwrap();

        std::thread::spawn(move || {
            let _guard = with_fonts(&[], ("Inter", "ui-monospace"));
            assert!(MEASURER.with_borrow(Option::is_none), "with_fonts itself must not build a TextSystem");
            let _ = measure("Hello world 0123", &[], sans, None);
            MEASURER.with_borrow(|m| {
                let m = m.as_ref().expect("measurer built");
                assert!(m.text.system_mono.is_none(), "with_fonts default mono must stay lazy on sans-only measure");
                assert!(m.text.font_files.is_empty(), "with_fonts default mono must not open system font files");
                assert!(m.text.installed.is_empty(), "with_fonts default mono must not query installed fonts");
            });
        })
        .join()
        .unwrap();
    }

    #[test]
    fn dropping_override_guards_at_thread_exit_after_override_destroyed_does_not_abort() {
        thread_local! {
            static HELD_FAMILIES: RefCell<Option<FamiliesGuard>> = const { RefCell::new(None) };
            static HELD_FONTS: RefCell<Option<FontsGuard>> = const { RefCell::new(None) };
        }

        std::thread::spawn(|| {
            HELD_FAMILIES.with(|slot| assert!(slot.borrow().is_none()));
            let guard = with_families("Inter", "ui-monospace");
            HELD_FAMILIES.with(|slot| *slot.borrow_mut() = Some(guard));
        })
        .join()
        .expect("thread with TLS FamiliesGuard exited cleanly");

        std::thread::spawn(|| {
            HELD_FONTS.with(|slot| assert!(slot.borrow().is_none()));
            let guard = with_fonts(&[], ("Inter", "ui-monospace"));
            HELD_FONTS.with(|slot| *slot.borrow_mut() = Some(guard));
        })
        .join()
        .expect("thread with TLS FontsGuard exited cleanly");
    }
}
