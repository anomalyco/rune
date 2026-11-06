//! The page's tokens: black, one neutral at four strengths, hairlines as translucent white and
//! Rune's blue ink as the only accent. One sans (Geist) and one mono (Geist Mono), line heights
//! on the 4 pt grid.

use rune::{Color, TextStyle, oklch};

pub const BG: Color = Color::BLACK;

pub fn fg() -> Color {
    rune::color::hex("#ededed").unwrap_or(Color::WHITE)
}
pub fn fg2() -> Color {
    Color::WHITE.alpha(0.62)
}
pub fn fg3() -> Color {
    Color::WHITE.alpha(0.5)
}
pub fn surface1() -> Color {
    Color::WHITE.alpha(0.03)
}
pub fn surface2() -> Color {
    Color::WHITE.alpha(0.06)
}
pub fn surface3() -> Color {
    Color::WHITE.alpha(0.1)
}
pub fn line() -> Color {
    Color::WHITE.alpha(0.09)
}
pub fn line2() -> Color {
    Color::WHITE.alpha(0.16)
}
pub fn ink() -> Color {
    oklch(0.74, 0.14, 245.0)
}
pub fn glint() -> Color {
    oklch(0.97, 0.035, 245.0)
}
pub fn code_keyword() -> Color {
    oklch(0.74, 0.12, 25.0)
}
pub fn code_string() -> Color {
    oklch(0.84, 0.06, 80.0)
}
pub fn code_call() -> Color {
    oklch(0.84, 0.045, 235.0)
}

pub const R2: f32 = 8.0;
/// The content column's width, before gutters.
pub const CONTENT: f32 = 1120.0;
/// Paragraphs stop at this width (36rem).
pub const MEASURE: f32 = 576.0;
pub const TOPBAR: f32 = 52.0;

pub const TEXT_12_MEDIUM: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0);
pub const TEXT_13: TextStyle = TextStyle::new(13.0, 400.0).lh(20.0);
pub const TEXT_14: TextStyle = TextStyle::new(14.0, 400.0).lh(22.0);
pub const TEXT_14_MEDIUM: TextStyle = TextStyle::new(14.0, 500.0).lh(22.0);
pub const TEXT_16: TextStyle = TextStyle::new(16.0, 400.0).lh(26.0);
pub const TEXT_16_MEDIUM: TextStyle = TextStyle::new(16.0, 500.0).lh(26.0);
pub const MONO_12: TextStyle = TextStyle::new(12.0, 400.0).lh(16.0).mono();
pub const MONO_13: TextStyle = TextStyle::new(13.0, 400.0).lh(20.0).mono();
/// Code blocks: mono 12 on 20 pt lines.
pub const CODE: TextStyle = TextStyle::new(12.0, 400.0).lh(20.0).mono();

/// The page's breakpoints, read from its width once per resize that crosses one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Bp {
    /// The widest breakpoint at or under the page's width: 0, 400, 560, 760, 860 or 1000.
    pub at: u16,
}

impl Bp {
    pub const STEPS: [u16; 6] = [0, 400, 560, 760, 860, 1000];

    pub fn of(width: f32) -> Bp {
        let at = Self::STEPS.iter().rev().copied().find(|&s| width >= f32::from(s)).unwrap_or(0);
        Bp { at }
    }
    /// Under 400: 16 pt gutters.
    pub fn narrow(self) -> bool {
        self.at < 400
    }
    /// Under 560: smaller headings and section padding.
    pub fn phone(self) -> bool {
        self.at < 560
    }
    /// Under 760: the gallery and OpenCode screens stack.
    pub fn stack_gallery(self) -> bool {
        self.at < 760
    }
    /// Under 860: code beside its output, the frame diagram, facts and platforms stack.
    pub fn stack_split(self) -> bool {
        self.at < 860
    }
    /// Under 1000: the hero's live card goes under its text.
    pub fn stack_hero(self) -> bool {
        self.at < 1000
    }
    pub fn gutter(self) -> f32 {
        if self.narrow() { 16.0 } else { 24.0 }
    }
    pub fn h1(self) -> TextStyle {
        if self.phone() { TextStyle::new(36.0, 500.0).lh(44.0).tracking(-0.9) } else { TextStyle::new(40.0, 500.0).lh(48.0).tracking(-1.0) }
    }
    pub fn h2(self) -> TextStyle {
        if self.phone() { TextStyle::new(24.0, 500.0).lh(32.0).tracking(-0.48) } else { TextStyle::new(28.0, 500.0).lh(36.0).tracking(-0.56) }
    }
    pub fn lede(self) -> TextStyle {
        if self.phone() { TEXT_16 } else { TextStyle::new(18.0, 400.0).lh(28.0) }
    }
}
