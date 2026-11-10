//! How Rune's glyphs approach CoreText's: the [`FontSmoothing`] modes and what each changes.

use super::ApplePipeline;

/// How glyphs are rasterised and placed: CoreGraphics' glyph pipeline, with or without font
/// smoothing's dilation. Set app-wide with `App::font_smoothing`, or at run time with
/// `Tx::set_font_smoothing`, which redraws with glyphs rasterised for the new mode.
///
/// The default is [`FontSmoothing::On`] on macOS and in a Mac's browser, and
/// [`FontSmoothing::Windfoil`] everywhere else (Windows, Linux, the web); every mode can be
/// chosen on every OS. `On` and `Off` work on any OS (they are arithmetic on outlines, not calls
/// into CoreText). Either way the pipeline
/// floors pens to CoreGraphics' subpixel steps (`quantisation_steps`), flattens curves and
/// hints small sizes as CoreGraphics does, and draws coverage as is. Windows blend in
/// sRGB-encoded space, as CoreGraphics does, unless the app sets `App::srgb_blend`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontSmoothing {
    /// Text that matches CoreText's with font smoothing on (macOS's default): the pipeline plus
    /// font smoothing's outline dilation, which grows outlines by a radius set by the text
    /// colour's luminance and the device ppem ([`ApplePipeline::SMOOTHED`]). The default on
    /// macOS, and on the web in a Mac's browser, which draws the page's own text this way.
    On,
    /// Text that matches CoreText with font smoothing off, which is what macOS browsers draw
    /// for `-webkit-font-smoothing: antialiased`: the pipeline without the dilation
    /// ([`ApplePipeline::UNSMOOTHED`]). Lighter than `On`. Linux and the web other than a Mac's
    /// browser drew with it before `Windfoil`.
    Off,
    /// Text that matches Chromium's on Windows: DirectWrite rasterises each glyph as Skia asks it
    /// to (the rendering mode the font's `gasp` table picks, its hints, grayscale antialiasing),
    /// pens sit on quarter pixels, and Skia's text gamma and contrast weigh the coverage by the
    /// text's luminance. Elsewhere, without DirectWrite, it draws as `Off`.
    DirectWrite,
    /// Rune's own text, the same on every platform, and the default everywhere but macOS: each
    /// glyph's exact pixel coverage from its outline (Windfoil's box filter), grown by CoreText's
    /// font smoothing dilation (times `TEXT_WEIGHT`, so its ink matches CoreText's), with
    /// coverage drawn as is and pens on quarter pixels. Masks at rest are filled on the GPU by the
    /// shader that draws text whose scale moves (zooms, presence scales, text flights) straight
    /// from its outlines, so it stays sharp and doesn't change when it lands. Where the renderer
    /// can't draw outlines, the CPU fills the same masks and moving text draws from them.
    Windfoil,
    /// Windfoil with Chromium's weight: each glyph's exact box coverage of its design outline,
    /// with no dilation or hint, drawn through Skia's text gamma (the luminance-keyed exponent
    /// `Off` takes off macOS), as Chromium's grayscale text weighs. Pens on quarter pixels.
    WindfoilChromium,
    /// Slug's coverage (Lengyel): the winding along a horizontal and a vertical ray through each
    /// pixel, each spread over the pixel's width, averaged; of the design outline, through Skia's
    /// text gamma. Drawn by the same shader and store as Windfoil, so it needs the renderer's
    /// Windfoil shader: without it the CPU fills Windfoil's box coverage instead.
    Slug,
    /// FreeType's light autohint (skrifa's) moves the outline's heights onto the pixel grid at
    /// each size, then Windfoil's box coverage, through Skia's text gamma. Pens on quarter pixels.
    HintedGpu,
    /// What Chromium draws on Linux: the light-autohinted outline (FreeType's `hintslight`) on
    /// whole-pixel pens, with grayscale box coverage through Skia's text gamma. On Windows
    /// Chromium's text is `DirectWrite`'s.
    Hinted,
    /// `WindfoilChromium` per subpixel: Windfoil's box over each third of a pixel, red to blue,
    /// blended channel by channel (dual-source blending), as ClearType draws. Inside layers, and
    /// where the GPU can't blend that way, it draws as `WindfoilChromium`.
    WindfoilRgb,
    /// Windows only: DirectWrite's ClearType (`CLEARTYPE_3x1`, Skia's gamma per channel),
    /// blended channel by channel, as Chromium draws text on an opaque background. Inside layers,
    /// and where the GPU can't blend that way, it draws as `DirectWrite`.
    ClearType,
}

impl Default for FontSmoothing {
    fn default() -> Self {
        match FontSmoothing::classic() {
            FontSmoothing::On => FontSmoothing::On,
            _ => FontSmoothing::Windfoil,
        }
    }
}

impl FontSmoothing {
    /// The mode this OS drew with before `Windfoil`: `On` on macOS and in a Mac's browser,
    /// `DirectWrite` on Windows and `Off` elsewhere. Where it's `On` it stays the default; an
    /// app's setting offers it as the way back elsewhere.
    pub fn classic() -> FontSmoothing {
        if cfg!(target_os = "macos") {
            FontSmoothing::On
        } else if cfg!(target_os = "windows") {
            FontSmoothing::DirectWrite
        } else {
            #[cfg(target_arch = "wasm32")]
            if let Some(platform) = crate::platform::web::platform::browser_platform() {
                return browser_default(&platform.0, platform.1);
            }
            FontSmoothing::Off
        }
    }
}

/// What a browser's page text draws like, from its `navigator` platform and touch points:
/// Safari, Chrome and Firefox on a Mac draw with font smoothing unless a page turns it off.
/// iPhones and iPads never smooth, and an iPad reports itself as a Mac with a touch screen.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn browser_default(platform: &str, touch_points: u32) -> FontSmoothing {
    let mac = platform.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("mac"));
    if mac && touch_points <= 1 { FontSmoothing::On } else { FontSmoothing::Off }
}

impl FontSmoothing {
    /// The CoreGraphics stages glyphs rasterise with: all of them for `On`, all but the
    /// dilation for `Off` (and for `DirectWrite` where DirectWrite can't draw a glyph).
    pub fn pipeline(self) -> ApplePipeline {
        match self {
            FontSmoothing::On | FontSmoothing::Windfoil => ApplePipeline::SMOOTHED,
            _ => ApplePipeline::UNSMOOTHED,
        }
    }

    /// How a mode drawn from outlines (`text/windfoil.rs`) draws them; None for the others.
    pub(crate) fn look(self) -> Option<Look> {
        let chromium = |coverage, hint, whole_pens| Look { macos_weight: false, coverage, hint, whole_pens };
        Some(match self {
            FontSmoothing::Windfoil => Look { macos_weight: true, coverage: Coverage::Box, hint: OutlineHint::CoreGraphics, whole_pens: false },
            FontSmoothing::WindfoilChromium => chromium(Coverage::Box, OutlineHint::None, false),
            FontSmoothing::Slug => chromium(Coverage::Slug, OutlineHint::None, false),
            FontSmoothing::HintedGpu => chromium(Coverage::Box, OutlineHint::Auto, false),
            FontSmoothing::Hinted => chromium(Coverage::Box, OutlineHint::Auto, true),
            FontSmoothing::WindfoilRgb => chromium(Coverage::Rgb, OutlineHint::None, false),
            FontSmoothing::On | FontSmoothing::Off | FontSmoothing::DirectWrite | FontSmoothing::ClearType => return None,
        })
    }

    /// The glyph key tag of an outline-drawn mode's masks: one per mode, so none share masks.
    pub(crate) fn outline_key(self) -> Option<u8> {
        let n = match self {
            FontSmoothing::Windfoil => 0,
            FontSmoothing::WindfoilChromium => 1,
            FontSmoothing::Slug => 2,
            FontSmoothing::HintedGpu => 3,
            FontSmoothing::Hinted => 4,
            FontSmoothing::WindfoilRgb => 5,
            FontSmoothing::On | FontSmoothing::Off | FontSmoothing::DirectWrite | FontSmoothing::ClearType => return None,
        };
        Some(super::glyph_emit::WINDFOIL_KEY | n)
    }

    /// Whether it draws subpixel masks, three coverages a pixel.
    pub(crate) fn lcd(self) -> bool {
        matches!(self, FontSmoothing::WindfoilRgb | FontSmoothing::ClearType)
    }

    /// What a subpixel mode draws as while subpixel text can't draw here (`lcd_ok` false): its
    /// grayscale twin. Every other mode draws as itself.
    pub(crate) fn drawn(self, lcd_ok: bool) -> FontSmoothing {
        match self {
            FontSmoothing::WindfoilRgb if !lcd_ok => FontSmoothing::WindfoilChromium,
            FontSmoothing::ClearType if !lcd_ok => FontSmoothing::DirectWrite,
            mode => mode,
        }
    }
}

/// How an outline-drawn mode draws a glyph ([`FontSmoothing::look`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Look {
    /// CoreText's dilation (times `TEXT_WEIGHT`) and coverage as is; otherwise no dilation and
    /// Skia's text gamma.
    pub(crate) macos_weight: bool,
    pub(crate) coverage: Coverage,
    pub(crate) hint: OutlineHint,
    /// Pens on whole pixels rather than quarters.
    pub(crate) whole_pens: bool,
}

/// The coverage function the outline shader integrates (`outline.wgsl`, the instance's `border.w`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coverage {
    Box = 0,
    Slug = 1,
    /// The box over each subpixel: three coverages a pixel. A mask's fill takes 3, one subpixel
    /// a texel.
    Rgb = 2,
}

/// What moves an outline onto the pixel grid before its coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutlineHint {
    None,
    /// CoreGraphics' y autohint, applied in the shader.
    CoreGraphics,
    /// FreeType's light autohint (skrifa), baked into the glyph's record at its size.
    Auto,
}

/// Glyph x phases are in sixtieths of a device pixel ([`PHASES`]), which hold every step
/// CoreGraphics quantises to (fifths to wholes).
pub(crate) const PHASES: f32 = 60.0;

/// How many x positions per device pixel CoreGraphics' subpixel quantisation draws a glyph of
/// `ppem` device px at: one more than the whole number of times `ppem` goes into 33⅓, at most
/// five. Fifths below 8⅓ ppem, quarters to 11.1, thirds to 16.7, halves to 33.3 and whole
/// pixels above, at every scale (measured on macOS 15.3: 1× and 2×, ppem 3–80, in ¼ steps
/// around each boundary). A ¼-point floor matches it only at 2× from 17 to 33 ppem.
pub(crate) fn quantisation_steps(ppem: f32) -> f32 {
    if ppem > 0.0 { (1.0 + (100.0 / 3.0 / ppem).floor()).min(5.0) } else { 5.0 }
}

/// The device pixel a glyph's quad starts at and its x phase in sixtieths of a pixel (its atlas
/// bin, [`PHASES`]), for a pen at `x` device px and a glyph of `ppem` device px. The pen floors
/// to [`quantisation_steps`] steps a pixel, as CoreGraphics' default subpixel quantisation does.
pub(crate) fn glyph_x(x: f32, ppem: f32) -> (f32, u8) {
    let steps = quantisation_steps(ppem);
    // Parley's pens are whole 1/256 px, so summing a line's advances in f32 only adds noise
    // (a run split by a colour span sums in a different order). Snapping to that grid drops
    // the noise without moving a pen that sits just short of a step (353 - 1/256 still floors).
    let x = ((x * 256.0).round() / 256.0 * steps).floor() / steps;
    let fx = x.floor();
    let phase = ((x - fx) * steps).round();
    if phase >= steps { (fx + 1.0, 0) } else { (fx, (phase * (PHASES / steps)) as u8) }
}

/// Coverage exponents for black and for white text with font smoothing off: the curve Skia's
/// text gamma (sRGB, contrast 1) draws grayscale text with, for outlines rasterised elsewhere.
const TEXT_GAMMA: [f32; 2] = [1.12, 0.45];

/// The exponent glyph coverage in `mode` is raised to when drawn in premultiplied `fill`, on
/// Apple platforms (`apple`) or Windows (`windows`) or neither. With smoothing off on Linux,
/// Windows and the web it stands in for the text gamma a browser's grayscale antialiasing
/// applies there: light text on a dark page draws heavier and dark text on a light one a little
/// lighter than coverage blended as is, keyed by the text's luminance. On macOS and iOS coverage
/// is drawn as is in every mode: CoreText with smoothing off, and Safari and Chrome with
/// `-webkit-font-smoothing: antialiased`, apply no text gamma (measured on macOS 15.3 with
/// Chrome 156: the same ink as CoreText, where the gamma drew light-on-dark text up to 24%
/// heavier). With smoothing on, CoreText's dilation does that job, and with DirectWrite on
/// Windows Skia's gamma tables do; coverage is drawn as is.
pub(crate) fn coverage_exponent(mode: FontSmoothing, fill: [f32; 4], apple: bool, windows: bool) -> f32 {
    // DirectWrite masks already carry Skia's gamma for their text's luminance.
    let direct_write = matches!(mode, FontSmoothing::DirectWrite | FontSmoothing::ClearType) && windows;
    // Outline modes with Chromium's weight take its gamma on every OS, as the lab drew them.
    let chromium = mode.look().is_some_and(|l| !l.macos_weight);
    if (apple && !chromium) || direct_write || matches!(mode, FontSmoothing::On | FontSmoothing::Windfoil) || fill[3] <= 0.0 {
        return 1.0;
    }
    let luma = (0.2126 * fill[0] + 0.7152 * fill[1] + 0.0722 * fill[2]) / fill[3];
    TEXT_GAMMA[0] + (TEXT_GAMMA[1] - TEXT_GAMMA[0]) * luma.clamp(0.0, 1.0)
}

#[cfg(test)]
#[path = "coretext_match_tests.rs"]
mod tests;
