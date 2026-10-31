use rune::Color;

/// The panel's colours. `dark()` is the knobs library's main look.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub bg: Color,
    /// Wells: the preset tray.
    pub inset: Color,
    /// The selected preset chip.
    pub raised: Color,
    pub fg: Color,
    /// Labels and readouts.
    pub fg2: Color,
    /// Group tabs at rest, icons, hints.
    pub fg3: Color,
    /// A card of rows.
    pub fill: Color,
    pub fill_hover: Color,
    pub fill_selected: Color,
    /// A slider's fill at rest, hovered and dragged.
    pub slider: Color,
    pub slider_hover: Color,
    pub slider_active: Color,
    pub handle: Color,
    pub border: Color,
    /// The hairline between rows, over their fills.
    pub rule: Color,
    /// The panel's top edge highlight.
    pub rim: Color,
    pub shadow: Color,
    /// The outline around a region.
    pub outline: Color,
}

impl Style {
    /// Opaque colours worked out as the browser composites the knobs theme (sRGB), so they read
    /// the same in Rune's linear blending.
    pub fn dark() -> Self {
        let h = |s: &str| rune::color::hex(s).unwrap_or(Color::BLACK);
        Self {
            bg: h("#1d1d1f"),
            inset: h("#161618"),
            raised: h("#3c3c3e"),
            fg: h("#fafafa"),
            fg2: h("#c4c4c6"),
            fg3: h("#8a8a8d"),
            fill: h("#29292b"),
            fill_hover: h("#2f2f31"),
            fill_selected: h("#363638"),
            slider: h("#5a5a5c"),
            slider_hover: h("#626264"),
            slider_active: h("#6e6e70"),
            handle: h("#ffffff"),
            border: h("#2c2c2e"),
            rule: Color::WHITE.alpha(0.025),
            rim: h("#3a3a3c"),
            shadow: Color::BLACK.alpha(0.45),
            outline: Color::WHITE.alpha(0.55),
        }
    }

    /// The light look: white card, dark text, grey fills.
    pub fn light() -> Self {
        let h = |s: &str| rune::color::hex(s).unwrap_or(Color::WHITE);
        Self {
            bg: h("#ffffff"),
            inset: h("#ececee"),
            raised: h("#ffffff"),
            fg: h("#1d1d1f"),
            fg2: h("#4a4a4d"),
            fg3: h("#8a8a8d"),
            fill: h("#f3f3f5"),
            fill_hover: h("#ececee"),
            fill_selected: h("#e4e4e7"),
            slider: h("#dcdce0"),
            slider_hover: h("#d3d3d8"),
            slider_active: h("#c8c8ce"),
            handle: h("#1d1d1f"),
            border: h("#e2e2e5"),
            rule: Color::BLACK.alpha(0.05),
            rim: h("#ffffff"),
            shadow: Color::BLACK.alpha(0.18),
            outline: Color::BLACK.alpha(0.45),
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::dark()
    }
}
