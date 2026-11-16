//! Outlines for the glyphs an app's text will need, built a little at a time in the idle turns
//! after its first frame, so the first frame that shows text in a new place draws them instead of
//! building them. An app names the styles ([`warm_outlines`]); a window's text system takes the
//! request up on its next idle turn and builds printable ASCII's outlines in those styles, a slice
//! a turn and none while input waits, then stops: an idle window wakes for it no more.
//!
//! Only outlines: a face's outline is one record whatever the size (unless TrueType or vertical
//! hints fit it to a ppem), where its masks would multiply by sizes and pen phases.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use super::*;

thread_local! {
    /// The styles last asked for on this thread, numbered by request.
    static ASKED: RefCell<(u64, Vec<TextStyle>)> = const { RefCell::new((0, Vec::new())) };
}

/// Printable ASCII, what most of an app's text is drawn from.
const CHARS: &str = "!\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

/// Builds the outlines of printable ASCII in `styles` in the idle turns after a window's first
/// frame, in every window on this thread. A later call replaces the request. Does nothing where
/// glyphs aren't drawn from outlines.
pub fn warm_outlines(styles: &[TextStyle]) {
    ASKED.with_borrow_mut(|(n, asked)| {
        *n += 1;
        *asked = styles.to_vec();
    });
}

/// A text system's warm-up: the request it took, the styles left, and the glyphs of the style
/// it's on (each run's face, coordinates and glyph ids).
#[derive(Default)]
pub(crate) struct Warm {
    taken: u64,
    styles: Vec<TextStyle>,
    glyphs: Vec<(Face, Vec<i16>, Vec<u32>)>,
}

impl TextSystem {
    /// Whether a warm-up is owed: asked for since it last looked, or not yet built.
    pub(crate) fn warm_owed(&self) -> bool {
        let w = &self.atlas.outlines.warm;
        ASKED.with_borrow(|(n, _)| *n != w.taken) || !w.styles.is_empty() || !w.glyphs.is_empty()
    }

    /// Builds owed outlines at `scale` for about `budget`; true while more are owed.
    pub(crate) fn warm_step(&mut self, scale: f32, budget: Duration) -> bool {
        let end = Instant::now() + budget;
        ASKED.with_borrow(|(n, asked)| {
            let w = &mut self.atlas.outlines.warm;
            if *n != w.taken {
                w.taken = *n;
                w.styles = asked.iter().rev().copied().collect();
                w.glyphs.clear();
            }
        });
        if self.smoothing.look().is_none() {
            let w = &mut self.atlas.outlines.warm;
            (w.styles.clear(), w.glyphs.clear());
            return false;
        }
        while Instant::now() < end {
            if let Some((face, coords, mut ids)) = self.atlas.outlines.warm.glyphs.pop() {
                let ppem = face.size;
                while let Some(id) = ids.pop() {
                    let _ = self.find_outline(&face, &coords, id, ppem);
                    if Instant::now() >= end { break; }
                }
                if !ids.is_empty() { self.atlas.outlines.warm.glyphs.push((face, coords, ids)); }
            } else if let Some(style) = self.atlas.outlines.warm.styles.pop() {
                let layout = self.layout_keyed(None, CHARS, &[], &[], &[], style, scale, None, Alignment::Start, false, false, Keep::Owned, parley::BaseDirection::Ltr);
                for line in layout.lines() {
                    for item in line.items() {
                        let PositionedLayoutItem::GlyphRun(run) = item else { continue };
                        let mut coords = Vec::new();
                        let face = Face::of(&run, &mut coords);
                        let ids = run.glyphs().map(|g| g.id).collect();
                        self.atlas.outlines.warm.glyphs.push((face, coords, ids));
                    }
                }
            } else {
                break;
            }
        }
        self.warm_owed()
    }
}
