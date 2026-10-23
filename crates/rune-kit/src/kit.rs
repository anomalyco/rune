//! The semantic token schema every rune-kit component reads. A theme fills one `Kit` per
//! appearance (see `rune-theme-opencode` for the OpenCode theme, or [`Kit::neutral`]).
//! Components never name raw colours, sizes or fonts outside these fields.

use rune::{Color, Icon, Shadow, Spring, TextStyle};

/// Colours for one status role (success, warning, danger, info).
#[derive(Clone, Copy, Debug)]
pub struct StateColors {
    pub bg: Color,
    pub fg: Color,
    pub border: Color,
    /// The role's icon/accent colour (card rules, status icons).
    pub icon: Color,
}

/// Semantic colours. Translucent tokens are already solved for the theme's blending (see
/// `rune::theme::srgb_equivalent`).
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// The page behind components (Storybook's frame, an app's window).
    pub page: Color,
    pub bg_base: Color,
    pub bg_deep: Color,
    pub layer_01: Color,
    pub layer_02: Color,
    pub layer_03: Color,
    pub layer_04: Color,
    pub bg_inverse: Color,
    pub bg_contrast: Color,
    pub bg_button_neutral: Color,
    pub bg_icon_button_contrast: Color,
    pub bg_accent: Color,
    /// Submit buttons and contrast icon buttons: the top sheen, the hover overlay and the ink.
    /// `submit_highlight`, `submit_hover`, `contrast_*` and `depth_*` are straight alpha (not
    /// solved over `bg_base`): they sit on dark or accent surfaces, so paint them with
    /// [`Kit::on`] and the surface they cover.
    pub submit_highlight: Color,
    pub submit_hover: Color,
    pub submit_fg: Color,
    pub submit_icon: Color,

    pub text_base: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub text_inverse: Color,
    pub text_contrast: Color,
    pub text_accent: Color,

    pub icon_base: Color,
    pub icon_muted: Color,
    pub icon_faint: Color,
    pub icon_inverse: Color,
    pub icon_contrast: Color,
    pub icon_accent: Color,

    pub border_muted: Color,
    pub border_base: Color,
    pub border_strong: Color,
    pub border_focus: Color,

    pub hover: Color,
    pub pressed: Color,
    pub contrast_hover: Color,
    pub contrast_pressed: Color,
    pub scrim: Color,
    /// The top and bottom of the "depth" sheen on raised knobs.
    pub depth_top: Color,
    pub depth_bottom: Color,
    /// Opaque white and black that sheens and highlights are mixed from (`Kit::alpha`).
    pub white: Color,
    pub black: Color,
    /// Switch and radio knobs (off, on).
    pub knob: Color,
    pub knob_on: Color,

    pub success: StateColors,
    pub warning: StateColors,
    pub danger: StateColors,
    pub info: StateColors,

    /// Roles the kit's older ("standard" appearance) components still use.
    pub surface_base: Color,
    pub surface_weak: Color,
    pub surface_hover: Color,
    pub surface_raised: Color,
    pub surface_raised_base: Color,
    pub surface_raised_hover: Color,
    pub surface_float: Color,
    pub surface_disabled: Color,
    pub border_weak: Color,
    pub border_hover: Color,
    pub border_active: Color,
    pub border_disabled: Color,
    pub text_strong: Color,
    pub text_weak: Color,
    pub text_invert_strong: Color,
    pub icon_weaker: Color,
    /// `--border-base`, `--icon-base`, `--icon-invert-base`, `--icon-strong-base`, `--text-base`
    /// of the older token set (standard-appearance switch and checkbox).
    pub std_border: Color,
    pub std_icon: Color,
    pub std_icon_invert: Color,
    pub std_icon_strong: Color,
    pub std_text: Color,
}

/// Elevation tokens as box-shadow layers (outer and inset), painted in order.
#[derive(Clone, Debug, Default)]
pub struct Elevation {
    pub raised: Vec<Shadow>,
    pub floating: Vec<Shadow>,
    pub overlay: Vec<Shadow>,
    pub button_neutral: Vec<Shadow>,
    pub button_contrast: Vec<Shadow>,
    pub elements: Vec<Shadow>,
    pub switch_off: Vec<Shadow>,
    pub switch_on: Vec<Shadow>,
    /// The older `--shadow-md` used by popovers and hover cards.
    pub md: Vec<Shadow>,
    /// The standard-appearance switch thumb.
    pub knob: Vec<Shadow>,
}

/// Small glyphs drawn inside components (not the app's icon catalog).
#[derive(Clone, Copy, Debug)]
pub struct Glyphs {
    pub check: &'static Icon,
    pub check_small: &'static Icon,
    pub chevron_down: &'static Icon,
    pub chevron_up: &'static Icon,
    pub caret_right: &'static Icon,
    pub triangle_down: &'static Icon,
    pub triangle_right: &'static Icon,
    pub close: &'static Icon,
    pub clear: &'static Icon,
    pub copy: &'static Icon,
    pub search: &'static Icon,
    pub lock: &'static Icon,
    pub eye: &'static Icon,
    pub eye_off: &'static Icon,
    pub circle_x: &'static Icon,
    /// A short horizontal rule (card title placeholder).
    pub dash: &'static Icon,
}

/// The type scale. Sizes, weights, line heights and tracking in points.
#[derive(Clone, Copy, Debug)]
pub struct Type {
    /// Controls and body copy: 13 / 20.
    pub ui: TextStyle,
    /// `ui` at the medium weight (buttons, titles).
    pub ui_medium: TextStyle,
    /// 13 on a 16 line (inputs, menu items).
    pub ui_compact: TextStyle,
    /// Captions and small labels: 11.
    pub small: TextStyle,
    pub small_medium: TextStyle,
    /// The older text-14 body (cards).
    pub body: TextStyle,
    pub body_medium: TextStyle,
    /// The older text-13 small (accordion, checkbox, progress).
    pub text_small: TextStyle,
    pub text_small_medium: TextStyle,
    /// The older text-12 (descriptions).
    pub text_xsmall: TextStyle,
    pub text_xsmall_medium: TextStyle,
    pub mono: TextStyle,
}

#[derive(Clone, Copy, Debug)]
pub struct Radius {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

/// Motion tokens. Hover/press overlays use the runtime's own snappy track.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    /// Controls changing state (switch thumb, checkbox, colour changes): ~90–170 ms ease-out on the web.
    pub control: Spring,
    /// Overlays opening and closing (popover, menu, tooltip): ~120–150 ms.
    pub overlay: Spring,
    /// Disclosure height changes (accordion, collapsible).
    pub disclosure: Spring,
}

#[derive(Clone, Debug)]
pub struct Kit {
    pub dark: bool,
    pub c: Palette,
    pub elevation: Elevation,
    pub glyph: Glyphs,
    pub ty: Type,
    pub radius: Radius,
    pub motion: Motion,
    /// The surface translucent colours are solved over (for sRGB-equivalent blending).
    pub blend_over: Option<Color>,
    /// When set, zero-blur elevation layers (rings) take this palette colour as they are
    /// painted, so they follow token overrides made after the kit was built
    /// ([`Kit::from_palette`] sets `c.border_base`).
    pub ring: Option<fn(&Palette) -> Color>,
}

impl Kit {
    /// `c` at opacity `a`, re-solved so linear-light blending gives the browser's sRGB result.
    pub fn alpha(&self, c: Color, a: f32) -> Color {
        let ca = c.alpha(a);
        self.blend_over.map_or(ca, |bg| rune::theme::srgb_equivalent(ca, bg))
    }

    /// A straight-alpha colour re-solved for the surface it is painted on.
    pub fn on(&self, c: Color, backdrop: Color) -> Color {
        if self.blend_over.is_none() {
            return c;
        }
        rune::theme::srgb_equivalent(c, backdrop)
    }

    /// `layers` as painted: rings recoloured by [`Kit::ring`] when the theme sets it.
    pub fn shadow_layers<'a>(&'a self, layers: &'a [Shadow]) -> impl Iterator<Item = Shadow> + 'a {
        let ring = self.ring.map(|f| f(&self.c));
        layers.iter().map(move |s| match ring {
            Some(color) if s.blur == 0.0 => Shadow { color, ..*s },
            _ => *s,
        })
    }

    /// White at opacity `a` painted on `backdrop`.
    pub fn light_on(&self, a: f32, backdrop: Color) -> Color {
        self.on(self.c.white.alpha(a), backdrop)
    }

    /// White at opacity `a` (the `alpha-light-*` ramp).
    pub fn light(&self, a: f32) -> Color {
        self.alpha(self.c.white, a)
    }

    /// Black at opacity `a` (the `alpha-dark-*` ramp).
    pub fn dark_a(&self, a: f32) -> Color {
        self.alpha(self.c.black, a)
    }
}
