//! A label whose changed characters roll (iOS numeric/text content transition), for proportional
//! and monospace fonts alike.
//!
//! Each grapheme cluster is its own keyed text node in a soft-clipped row, as wide as its advance
//! in the whole label shaped once (kerning included), so at rest the row sets exactly like the
//! same string as one text. When the text changes,
//! the new clusters are matched to the old ones (common prefix and suffix, then an LCS of the
//! middle for short strings): matched clusters keep their key and slide to their new x on the
//! layout spring; the rest leave through one edge and enter from the other with a small per-glyph
//! stagger. A label given its [`RollingText::value`] picks the edge from the value's direction, as
//! SwiftUI's `.numericText(value:)` does: up as it rises, down as it falls. The row's width springs to the new width (`width_leads`). Everything runs on the
//! render clock; reduced motion drops the travel and leaves a crossfade.
//!
//! Keys must survive between builds, so each label remembers its previous clusters by its
//! [`RollingText::new`] key (per thread, per key). Rebuilding with the same text is a no-op.

use std::cell::RefCell;
use std::collections::HashMap;

use rune::text::Run;
use rune::{Color, Edge, Effect, El, Spring, SpringPair, TextStyle, Transition, Vec2, row, text};
use unicode_segmentation::UnicodeSegmentation;

use crate::Kit;

/// Extra spring duration per changed glyph, left to right (the default [`RollingText::stagger`]).
const STAGGER: f32 = 0.03;
/// The stagger stops growing after this many glyphs, so long replacements still land together.
const STAGGER_CAP: usize = 6;
/// LCS only runs while `old × new` middle clusters fit this budget; longer middles just roll.
const LCS_BUDGET: usize = 48 * 48;

/// Labels remembered per thread; the least recently built go first. A label that was evicted
/// and comes back starts its roll over.
const MAX_LABELS: usize = 512;
/// Cluster text (and key) bytes remembered per thread.
const MAX_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct Memory {
    clusters: Vec<(String, u64)>,
    next: u64,
    value: Option<f64>,
    used: u64,
}

impl Memory {
    fn bytes(&self, key: &str) -> usize {
        key.len() + self.clusters.iter().map(|(c, _)| c.len() + 16).sum::<usize>()
    }
}

#[derive(Default)]
struct Store {
    labels: HashMap<String, Memory>,
    tick: u64,
    bytes: usize,
}

impl Store {
    /// Drops the least recently built labels until both bounds hold, never `keep` (the label being
    /// built, so one label bigger than the byte bound is still remembered).
    fn evict(&mut self, keep: &str) {
        if self.labels.len() <= MAX_LABELS && self.bytes <= MAX_BYTES {
            return;
        }
        let mut order: Vec<(u64, String)> = self.labels.iter().filter(|(k, _)| k.as_str() != keep).map(|(k, m)| (m.used, k.clone())).collect();
        order.sort_unstable();
        // Down to three quarters, so the sort isn't paid on every new label.
        let (labels, bytes) = (MAX_LABELS * 3 / 4, MAX_BYTES * 3 / 4);
        for (_, k) in order {
            if self.labels.len() <= labels && self.bytes <= bytes {
                break;
            }
            if let Some(m) = self.labels.remove(&k) {
                self.bytes -= m.bytes(&k);
            }
        }
    }
}

thread_local! {
    static MEMORY: RefCell<Store> = RefCell::new(Store::default());
}

/// A label that rolls only the characters that changed.
///
/// ```ignore
/// RollingText::new("copy-label", if copied { format!("Copied, clears in 0:{left:02}") } else { "Copy password".into() })
///     .font(k.ty.ui_medium)
///     .build(&k)
/// ```
pub struct RollingText {
    key: String,
    text: String,
    font: Option<TextStyle>,
    color: Option<Color>,
    spring: Spring,
    from: Edge,
    value: Option<f64>,
    trailing: bool,
    stagger: f32,
    travel: f32,
    exit: f32,
    scale: f32,
}

impl RollingText {
    /// The default roll and slide spring: quick, with a hint of overshoot.
    pub const SPRING: Spring = Spring::new(0.36, 0.12);

    /// `key` names this label (it keys the row and remembers the previous text); `text` is what
    /// it shows now.
    pub fn new(key: impl Into<String>, text: impl Into<String>) -> RollingText {
        RollingText {
            key: key.into(),
            text: text.into(),
            font: None,
            color: None,
            spring: Self::SPRING,
            from: Edge::Bottom,
            value: None,
            trailing: false,
            stagger: STAGGER,
            travel: 1.0,
            exit: 1.0,
            scale: 1.0,
        }
    }
    pub fn font(mut self, font: TextStyle) -> Self {
        self.font = Some(font);
        self
    }
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
    /// The roll and slide spring (default [`RollingText::SPRING`]).
    pub fn spring(mut self, spring: Spring) -> Self {
        self.spring = spring;
        self
    }
    /// The edge new glyphs enter from (old ones leave through the opposite edge). Default
    /// `Edge::Bottom`; `Edge::Top` reads as counting down.
    pub fn from(mut self, edge: Edge) -> Self {
        self.from = edge;
        self
    }
    /// The number the label shows (SwiftUI's `.numericText(value:)`): when it rises, new glyphs
    /// enter from [`RollingText::from`] (digits move up); when it falls, from the opposite edge.
    /// Text that changes while the value holds (a new unit or format) rolls from `from`, as a
    /// label without a value does; so does a value that isn't finite.
    pub fn value(mut self, value: f64) -> Self {
        self.value = Some(value);
        self
    }
    /// Extra spring duration per changed glyph, left to right, so a replacement ripples across
    /// (default 0.03 s). 0 lands every changed glyph together, as a number should.
    pub fn stagger(mut self, secs: f32) -> Self {
        self.stagger = secs.max(0.0);
        self
    }
    /// How far a rolling glyph travels, as a fraction of its line (default 1, a full line).
    /// Shorter travel reads as a softer roll: the glyphs cross-fade over less distance, so less
    /// of each is cut by the edge fade.
    pub fn travel(mut self, fraction: f32) -> Self {
        self.travel = fraction.clamp(0.0, 1.0);
        self
    }
    /// How long a leaving glyph takes, as a fraction of the roll (default 1). Below 1 the old
    /// glyph is gone before the new one lands, so a short [`RollingText::travel`] doesn't show
    /// both at once.
    pub fn exit(mut self, fraction: f32) -> Self {
        self.exit = fraction.clamp(0.2, 1.0);
        self
    }
    /// The scale a rolling glyph shrinks to while it travels (default 1, 0.9 at the least).
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale.clamp(0.9, 1.0);
        self
    }
    /// The label sits against its trailing edge (right-aligned), so it widens to the left. The
    /// label and its glyphs ride their parent's trailing edge ([`El::ride_trailing`]), so new
    /// glyphs roll in where they come to rest and the glyphs that stay or leave hold their
    /// columns, whatever spring the row around it slides on: "9s" → "10s" rolls "10" in beside
    /// the "s", not over it.
    pub fn trailing(mut self) -> Self {
        self.trailing = true;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let font = self.font.unwrap_or(k.ty.ui);
        let color = self.color.unwrap_or(k.c.text_base);
        let (glyphs, falling) = keys(&self.key, &self.text, self.value, self.trailing);
        let from = if falling { self.from.opposite() } else { self.from };
        let edge = (font.line_px() * 0.12).max(1.0);
        // Glyphs leave the way this build rolls, whichever way they last did.
        let push = roll(from, self.travel, self.scale);
        let leave = Spring::new(self.spring.duration * self.exit, self.spring.bounce);
        let exit = push.springs(SpringPair::new(self.spring, leave));
        // The fades sit in padding that negative margins take back out of layout, so the resting
        // glyphs (descenders included) are never faded but rolling glyphs and a growing or
        // shrinking edge stay soft. Glyphs slide on the row's own width-leading X spring. A
        // trailing label and its glyphs instead ride their parent's trailing edge: as the label
        // widens to the left, the glyphs that stay, the ones leaving and the ones entering all
        // hold their columns, whatever springs the rows around them slide on.
        let side = (font.size * 0.3).max(2.0);
        let lead = |e: El| if self.trailing { e.ride_trailing() } else { e.width_leads() };
        let mut e = lead(row()
            .key(self.key.clone())
            .role("text")
            .label(self.text.clone())
            .h(font.line_px() + 2.0 * edge)
            .px(side)
            .py(edge)
            .ml(-side)
            .mr(-side)
            .mt(-edge)
            .mb(-edge)
            .motion(self.spring))
            .clip_fade(side, edge, side, edge)
            .child_exit(exit);
        // Every cell measures as its slice of the whole label shaped once, so the row keeps the
        // run's kerned, fractional advances and the width of the same string set as one text.
        let run = Run::new(self.text.as_str());
        let mut rank = 0;
        for (g, (at, _)) in glyphs.into_iter().zip(self.text.grapheme_indices(true)) {
            let spring = if g.entered {
                let s = Spring::new(self.spring.duration + self.stagger * rank.min(STAGGER_CAP) as f32, self.spring.bounce);
                rank += 1;
                s
            } else {
                self.spring
            };
            let tr = if g.entered {
                push.springs(SpringPair::new(spring, leave))
            } else {
                Transition::asymmetric(Transition::identity(), push).springs(SpringPair::new(self.spring, leave))
            };
            e = e.child(lead(
                text(g.cluster)
                    .key(format!("{}.{}", self.key, g.id))
                    .font(font)
                    .advance_in(&run, at)
                    .color(color)
                    .motion(spring),
            )
            .transition(tr));
        }
        e
    }
}

/// `Transition::push(edge)` over `travel` of the glyph's size, at `scale`: in from `edge`, out
/// through the opposite one, fading.
fn roll(edge: Edge, travel: f32, scale: f32) -> Transition {
    let e = |d: Edge| {
        let v = d.direction();
        Effect { offset_fraction: Vec2::new(v.x * travel, v.y * travel), opacity: 0.0, scale, ..Effect::IDENTITY }
    };
    Transition { insertion: e(edge), removal: e(edge.opposite()), ..Transition::push(edge) }
}

struct Glyph {
    cluster: String,
    id: u64,
    /// New in this build (it rolls in); otherwise it kept its identity.
    entered: bool,
}

/// Assigns each cluster of `text` a stable id: clusters matched to the previous text under `key`
/// keep theirs, the rest get fresh ones. Also whether `value` fell below the last finite value
/// one.
fn keys(key: &str, text: &str, value: Option<f64>, trailing: bool) -> (Vec<Glyph>, bool) {
    MEMORY.with(|m| {
        let mut store = m.borrow_mut();
        store.tick += 1;
        let tick = store.tick;
        let Store { labels, bytes, .. } = &mut *store;
        let known = labels.contains_key(key);
        let mem = labels.entry(key.to_owned()).or_default();
        mem.used = tick;
        if known {
            *bytes -= mem.bytes(key);
        }
        let new: Vec<&str> = text.graphemes(true).collect();
        let old: Vec<&str> = mem.clusters.iter().map(|(c, _)| c.as_str()).collect();
        let matched = diff(&old, &new, trailing);
        let fresh = mem.clusters.is_empty() && mem.next == 0;
        let mut glyphs = Vec::with_capacity(new.len());
        for (c, from) in new.iter().zip(matched) {
            let (id, entered) = match from {
                Some(i) => (mem.clusters[i].1, false),
                None => {
                    mem.next += 1;
                    (mem.next, !fresh)
                }
            };
            glyphs.push(Glyph { cluster: (*c).to_owned(), id, entered });
        }
        mem.clusters = glyphs.iter().map(|g| (g.cluster.clone(), g.id)).collect();
        let falling = matches!((mem.value, value), (Some(old), Some(new)) if new.is_finite() && new < old);
        if let Some(v) = value.filter(|v| v.is_finite()) {
            mem.value = Some(v);
        }
        *bytes += mem.bytes(key);
        store.evict(key);
        (glyphs, falling)
    })
}

/// For each cluster of `new`, the index of the `old` cluster it continues, if any. Matches keep
/// order: the common prefix and suffix, then an LCS of the middle when it is short. Middle matches
/// survive in runs of two or more, or alone when they hold their place (the ":" of "1:00" →
/// "0:59"); a lone letter that would slide among rolling neighbours rolls too, so "Copy password"
/// → "Copied, clears in" doesn't leave a stray "r" or "s" gliding across the label.
///
/// A `trailing` label holds still at its right edge, so it matches like an odometer, from the
/// right: the suffix first, a prefix only of non-digits (a "$" slides left with the number, a
/// digit that changes column rolls), and a place counted from the end. "10s" → "100s" keeps
/// "0s" and rolls a new "1" and "0" in; "19s" → "100s" keeps only the "s".
pub(crate) fn diff(old: &[&str], new: &[&str], trailing: bool) -> Vec<Option<usize>> {
    let mut out = vec![None; new.len()];
    let n = old.len().min(new.len());
    let digit = |c: &str| c.bytes().all(|b| b.is_ascii_digit());
    let mut suf = 0;
    let suffix = |out: &mut Vec<Option<usize>>, pre: usize, suf: &mut usize| {
        while *suf < n - pre && old[old.len() - 1 - *suf] == new[new.len() - 1 - *suf] {
            out[new.len() - 1 - *suf] = Some(old.len() - 1 - *suf);
            *suf += 1;
        }
    };
    if trailing {
        suffix(&mut out, 0, &mut suf);
    }
    let mut pre = 0;
    while pre < n - suf && old[pre] == new[pre] && !(trailing && digit(new[pre])) {
        out[pre] = Some(pre);
        pre += 1;
    }
    if !trailing {
        suffix(&mut out, pre, &mut suf);
    }
    let (a, b) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
    if a.is_empty() || b.is_empty() || a.len() * b.len() > LCS_BUDGET {
        return out;
    }
    // How far a match slides, in glyphs, measured from whichever end it sits closer to.
    let shift = |i: usize, j: usize| {
        let end = (a.len() - i).abs_diff(b.len() - j);
        if trailing { end } else { i.abs_diff(j).min(end) }
    };
    // A weighted LCS (suffix table): each match scores more the less it slides, so among
    // equally long subsequences the one that stays in place wins ("1:00" → "0:59" keeps ":").
    let weight = |i: usize, j: usize| 8 - shift(i, j).min(7) as u32;
    let w = b.len() + 1;
    let mut t = vec![0u32; (a.len() + 1) * w];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            let skip = t[(i + 1) * w + j].max(t[i * w + j + 1]);
            t[i * w + j] = if a[i] == b[j] { skip.max(t[(i + 1) * w + j + 1] + weight(i, j)) } else { skip };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] && t[i * w + j] == t[(i + 1) * w + j + 1] + weight(i, j) {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    for (k, &(i, j)) in pairs.iter().enumerate() {
        let run = (k > 0 && pairs[k - 1] == (i - 1, j - 1)) || pairs.get(k + 1) == Some(&(i + 1, j + 1));
        if run || shift(i, j) == 0 {
            out[pre + j] = Some(pre + i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored() -> (usize, usize) {
        MEMORY.with(|m| {
            let m = m.borrow();
            (m.labels.len(), m.bytes)
        })
    }

    #[test]
    fn memory_is_bounded_and_a_label_built_every_frame_keeps_its_own() {
        for i in 0..10_000 {
            keys("busy", &format!("{}", i % 7), None, false);
            keys(&format!("row-{i}"), "0:30", None, false);
            assert!(stored().0 <= MAX_LABELS && stored().1 <= MAX_BYTES);
        }
        let (first, _) = keys("busy", "3", None, false);
        let (again, _) = keys("busy", "3", None, false);
        assert!(!again[0].entered && again[0].id == first[0].id, "the busy label kept its memory");
        assert!(first[0].id > 1, "ids continued rather than restarting");
        let bytes: usize = MEMORY.with(|m| m.borrow().labels.iter().map(|(k, v)| v.bytes(k)).sum());
        assert_eq!(bytes, stored().1);
    }

    #[test]
    fn long_labels_are_bounded_by_bytes() {
        let long = "x".repeat(5_000);
        for i in 0..200 {
            keys(&format!("long-{i}"), &long, None, false);
            assert!(stored().1 <= MAX_BYTES);
        }
    }

    fn kept(old: &str, new: &str) -> String {
        let (o, n): (Vec<&str>, Vec<&str>) = (old.graphemes(true).collect(), new.graphemes(true).collect());
        diff(&o, &n, false).iter().zip(&n).map(|(m, c)| if m.is_some() { *c } else { "_" }).collect()
    }

    fn kept_end(old: &str, new: &str) -> String {
        let (o, n): (Vec<&str>, Vec<&str>) = (old.graphemes(true).collect(), new.graphemes(true).collect());
        diff(&o, &n, true).iter().zip(&n).map(|(m, c)| if m.is_some() { *c } else { "_" }).collect()
    }

    #[test]
    fn a_trailing_label_matches_from_the_right_like_an_odometer() {
        assert_eq!(kept_end("10s", "100s"), "__0s");
        assert_eq!(kept_end("19s", "100s"), "___s");
        assert_eq!(kept_end("9s", "10s"), "__s");
        assert_eq!(kept_end("$1.98", "$1.99"), "$1.9_");
        assert_eq!(kept_end("$9.99", "$10.00"), "$__.__");
        assert_eq!(kept_end("59s", "1m 00s"), "_____s");
    }

    #[test]
    fn a_countdown_rolls_only_the_changed_digits() {
        assert_eq!(kept("0:30", "0:29"), "0:__");
        assert_eq!(kept("0:21", "0:20"), "0:2_");
        assert_eq!(kept("1:00", "0:59"), "_:__");
        assert_eq!(kept("9", "10"), "__");
    }

    #[test]
    fn prefixes_suffixes_and_runs_keep_identity_but_lone_far_matches_roll() {
        assert_eq!(kept("Copy password", "Copied, clears in 0:30"), "Cop___________________");
        assert_eq!(kept("Copied, clears in 0:30", "Copy password"), "Cop__________");
        assert_eq!(kept("12 files", "3 files"), "_ files");
        assert_eq!(kept("Save draft", "Saved draft"), "Save_ draft");
        // The middle "ab" run moves as a pair.
        assert_eq!(kept("xxabyy", "xqqqabzyy"), "x___ab_yy");
    }

    #[test]
    fn graphemes_are_one_glyph() {
        assert_eq!(kept("e\u{301}1", "e\u{301}2"), "e\u{301}_");
        assert_eq!(kept("👍🏽 1", "👍🏽 2"), "👍🏽 _");
    }

    #[test]
    fn rebuilding_the_same_text_keeps_every_key() {
        let a: Vec<u64> = keys("same", "0:30", None, false).0.iter().map(|g| g.id).collect();
        let b: Vec<u64> = keys("same", "0:30", None, false).0.iter().map(|g| g.id).collect();
        assert_eq!(a, b);
        let c = keys("same", "0:29", None, false).0;
        assert_eq!((c[0].id, c[1].id), (a[0], a[1]));
        assert!(c[2].entered && c[3].entered && !c[0].entered);
        assert!(c[2].id > a[3] && c[3].id > a[3], "new glyphs never reuse an exiting key");
    }

    #[test]
    fn a_value_rolls_the_way_it_moves() {
        let falls = |text: &str, v: Option<f64>| keys("dir", text, v, false).1;
        assert!(!falls("9s", Some(9.0)), "the first build rolls nowhere, as counting up");
        assert!(!falls("10s", Some(10.0)), "rising");
        assert!(falls("8s", Some(8.0)), "falling");
        assert!(!falls("8 s", Some(8.0)), "new text with the same value rolls by the text rule");
        assert!(!falls("8s", None), "so does a build without a value");
        assert!(!falls("-", Some(f64::NAN)) && !falls("∞", Some(f64::NEG_INFINITY)), "no direction for NaN or infinity");
        assert!(falls("7s", Some(7.0)), "and they don't replace the last value");
        assert!(!falls("12s", Some(12.0)), "rising again");
    }
}
