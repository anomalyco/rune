//! A label whose changed characters roll (iOS numeric/text content transition), for proportional
//! and monospace fonts alike.
//!
//! Each grapheme cluster is its own keyed text node in a soft-clipped row. When the text changes,
//! the new clusters are matched to the old ones (common prefix and suffix, then an LCS of the
//! middle for short strings): matched clusters keep their key and slide to their new x on the
//! layout spring; the rest leave through one edge and enter from the other with a small per-glyph
//! stagger. The row's width springs to the new width (`width_leads`). Everything runs on the
//! render clock; reduced motion drops the travel and leaves a crossfade.
//!
//! Keys must survive between builds, so each label remembers its previous clusters by its
//! [`RollingText::new`] key (per thread, per key). Rebuilding with the same text is a no-op.

use std::cell::RefCell;
use std::collections::HashMap;

use rune::{Color, Edge, El, Spring, TextStyle, Transition, row, text};
use unicode_segmentation::UnicodeSegmentation;

use crate::Kit;

/// Extra spring duration per changed glyph, left to right.
const STAGGER: f32 = 0.03;
/// The stagger stops growing after this many glyphs, so long replacements still land together.
const STAGGER_CAP: usize = 6;
/// LCS only runs while `old × new` middle clusters fit this budget; longer middles just roll.
const LCS_BUDGET: usize = 48 * 48;

#[derive(Default)]
struct Memory {
    clusters: Vec<(String, u64)>,
    next: u64,
}

thread_local! {
    static MEMORY: RefCell<HashMap<String, Memory>> = RefCell::new(HashMap::new());
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

    pub fn build(self, k: &Kit) -> El {
        let font = self.font.unwrap_or(k.ty.ui);
        let color = self.color.unwrap_or(k.c.text_base);
        let glyphs = keys(&self.key, &self.text);
        let edge = (font.line_px() * 0.12).max(1.0);
        // The horizontal fade sits in padding that negative margins take back out of layout, so
        // the resting glyphs are never faded but a growing or shrinking edge stays soft.
        let side = (font.size * 0.3).max(2.0);
        let mut e = row()
            .key(self.key.clone())
            .role("text")
            .label(self.text.clone())
            .h(font.line_px())
            .px(side)
            .ml(-side)
            .mr(-side)
            .motion(self.spring)
            .width_leads()
            .clip_fade(side, edge, side, edge);
        let mut rank = 0;
        for g in glyphs {
            let spring = if g.entered {
                let s = Spring::new(self.spring.duration + STAGGER * rank.min(STAGGER_CAP) as f32, self.spring.bounce);
                rank += 1;
                s
            } else {
                self.spring
            };
            e = e.child(
                text(g.cluster)
                    .key(format!("{}.{}", self.key, g.id))
                    .font(font)
                    .color(color)
                    .motion(spring)
                    .transition(Transition::push(self.from)),
            );
        }
        e
    }
}

struct Glyph {
    cluster: String,
    id: u64,
    /// New in this build (it rolls in); otherwise it kept its identity.
    entered: bool,
}

/// Assigns each cluster of `text` a stable id: clusters matched to the previous text under `key`
/// keep theirs, the rest get fresh ones.
fn keys(key: &str, text: &str) -> Vec<Glyph> {
    MEMORY.with(|m| {
        let mut m = m.borrow_mut();
        let mem = m.entry(key.to_owned()).or_default();
        let new: Vec<&str> = text.graphemes(true).collect();
        let old: Vec<&str> = mem.clusters.iter().map(|(c, _)| c.as_str()).collect();
        let matched = diff(&old, &new);
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
        glyphs
    })
}

/// For each cluster of `new`, the index of the `old` cluster it continues, if any. Matches keep
/// order: the common prefix and suffix, then an LCS of the middle when it is short. Middle matches
/// survive in runs of two or more, or alone when they hold their place (the ":" of "1:00" →
/// "0:59"); a lone letter that would slide among rolling neighbours rolls too, so "Copy password"
/// → "Copied, clears in" doesn't leave a stray "r" or "s" gliding across the label.
pub(crate) fn diff(old: &[&str], new: &[&str]) -> Vec<Option<usize>> {
    let mut out = vec![None; new.len()];
    let n = old.len().min(new.len());
    let mut pre = 0;
    while pre < n && old[pre] == new[pre] {
        out[pre] = Some(pre);
        pre += 1;
    }
    let mut suf = 0;
    while suf < n - pre && old[old.len() - 1 - suf] == new[new.len() - 1 - suf] {
        out[new.len() - 1 - suf] = Some(old.len() - 1 - suf);
        suf += 1;
    }
    let (a, b) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
    if a.is_empty() || b.is_empty() || a.len() * b.len() > LCS_BUDGET {
        return out;
    }
    // How far a match slides, in glyphs, measured from whichever end it sits closer to.
    let shift = |i: usize, j: usize| i.abs_diff(j).min((a.len() - i).abs_diff(b.len() - j));
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

    fn kept(old: &str, new: &str) -> String {
        let (o, n): (Vec<&str>, Vec<&str>) = (old.graphemes(true).collect(), new.graphemes(true).collect());
        diff(&o, &n).iter().zip(&n).map(|(m, c)| if m.is_some() { *c } else { "_" }).collect()
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
        let a: Vec<u64> = keys("same", "0:30").iter().map(|g| g.id).collect();
        let b: Vec<u64> = keys("same", "0:30").iter().map(|g| g.id).collect();
        assert_eq!(a, b);
        let c = keys("same", "0:29");
        assert_eq!((c[0].id, c[1].id), (a[0], a[1]));
        assert!(c[2].entered && c[3].entered && !c[0].entered);
        assert!(c[2].id > a[3] && c[3].id > a[3], "new glyphs never reuse an exiting key");
    }
}
