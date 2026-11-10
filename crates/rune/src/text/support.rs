//! Which [`FontSmoothing`] modes this machine draws as named: DirectWrite needs Windows, and the
//! modes drawn from outlines need the renderer's Windfoil shader, which a renderer reports when
//! it builds or fails to build it (`note_outlines`). An app's setting lists every mode with this.

use std::sync::atomic::{AtomicU8, Ordering};

use super::FontSmoothing;
use super::outline::Support as Outlines;

/// Whether `mode` draws as named here.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmoothingSupport {
    Yes,
    /// It draws, as the reason says, not quite as named.
    Fallback(&'static str),
    /// It can't draw here: the reason, in a line.
    No(&'static str),
}

/// What the last renderer said about outline glyphs: 0 nothing yet, 1 drawn, 2 unavailable.
static OUTLINES: AtomicU8 = AtomicU8::new(0);

/// Records what a renderer can do with outlines, for [`FontSmoothing::support`].
pub(crate) fn note_outlines(support: Outlines) {
    OUTLINES.store(match support { Outlines::Untried => 0, Outlines::Drawn => 1, Outlines::Unavailable => 2 }, Ordering::Relaxed);
}

impl FontSmoothing {
    /// Every mode, in the order a setting lists them.
    pub const ALL: [FontSmoothing; 8] = [
        FontSmoothing::Windfoil,
        FontSmoothing::WindfoilChromium,
        FontSmoothing::Slug,
        FontSmoothing::HintedGpu,
        FontSmoothing::Hinted,
        FontSmoothing::DirectWrite,
        FontSmoothing::On,
        FontSmoothing::Off,
    ];

    /// Whether this machine draws `self` as named, and if not, why in a line.
    pub fn support(self) -> SmoothingSupport {
        let outlines_out = OUTLINES.load(Ordering::Relaxed) == 2;
        match self {
            FontSmoothing::DirectWrite if !cfg!(target_os = "windows") => SmoothingSupport::No("DirectWrite is Windows only"),
            FontSmoothing::Slug if outlines_out => SmoothingSupport::No("Slug draws on the GPU, and this renderer can't build its shader"),
            _ if outlines_out && self.look().is_some() => {
                SmoothingSupport::Fallback("This renderer can't build the outline shader: the CPU fills the same coverage, and moving text scales its masks")
            }
            _ => SmoothingSupport::Yes,
        }
    }
}
