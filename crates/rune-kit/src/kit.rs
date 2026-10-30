//! The semantic token schema every rune-kit component reads. A theme fills one `Kit` per
//! appearance (see `rune-theme-opencode` for the OpenCode theme, or [`Kit::neutral`]).
//! Components never name raw colours, sizes or fonts outside these fields.

use rune::{Color, El, Icon, Shadow, Spring, TextStyle};

/// Colours for one status role (success, warning, danger, info).
#[derive(Clone, Copy, Debug, Hash)]
pub struct StateColors {
    pub bg: Color,
    pub fg: Color,
    pub border: Color,
    /// The role's icon/accent colour (card rules, status icons).
    pub icon: Color,
}

/// Semantic colours. Translucent tokens are resolved for the blend mode of the window the kit
/// was built for (`rune::theme::blends_srgb`): as authored when it blends in sRGB, the default,
/// and solved over [`Kit::blend_over`] when it blends in linear light (see
/// `rune::theme::for_blend`). Hashable, as [`Type`] and [`Radius`] are, so a row can use
/// the tokens it reads as a `memo` version.
#[derive(Clone, Copy, Debug, Hash)]
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
#[derive(Clone, Copy, Debug, Hash)]
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

impl Type {
    /// Secondary text under a control or row (a setting's explanation, a file's size): `small`
    /// on a 16 line, so 11 / 16 in the built-in themes. A method rather than a field, so a
    /// theme's own `Type` gets it from its `small`.
    pub fn caption(&self) -> TextStyle {
        self.small.lh(16.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Radius {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

/// Hashes the radii's bits, `-0.0` as `0.0` and every NaN alike, as `Color` does.
impl std::hash::Hash for Radius {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let bits = |x: f32| if x == 0.0 { 0 } else if x.is_nan() { f32::NAN.to_bits() } else { x.to_bits() };
        [self.xs, self.sm, self.md, self.lg, self.xl].map(bits).hash(state);
    }
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
    /// The surface translucent colours are solved over when the window blends in linear light
    /// (`App::srgb_blend(false)`), so they look as they would blended in sRGB. None leaves
    /// colours as they are in both modes.
    pub blend_over: Option<Color>,
    /// When set, zero-blur elevation layers (rings) take this palette colour as they are
    /// painted, so they follow token overrides made after the kit was built
    /// ([`Kit::from_palette`] sets `c.border_base`).
    pub ring: Option<fn(&Palette) -> Color>,
    /// Invalid inputs also paint their text, placeholder and caret in `c.danger.fg`
    /// (`@opencode/ui`'s look). Otherwise only the outline turns `danger`.
    pub invalid_text: bool,
}

impl Kit {
    /// Gives `e` the kit's hover and press look: `c.hover` under the pointer, and `c.pressed`
    /// in its place while held, as the kit's own rows and buttons show them. `e` still needs a
    /// handler (or `.hit()`) to be hit.
    pub fn interactive(&self, e: El) -> El {
        self.interactive_with(e, self.c.hover, self.c.pressed)
    }

    /// [`Self::interactive`] with other overlays, e.g. `c.contrast_hover` and
    /// `c.contrast_pressed` on a contrast fill. `pressed` replaces `hover` while held rather
    /// than stacking on it.
    pub fn interactive_with(&self, e: El, hover: Color, pressed: Color) -> El {
        e.hover(hover).press(crate::press_over(hover, pressed))
    }

    /// Paints elevation layers on `e` as box shadows, e.g. `k.elevate(e, &k.elevation.floating)`.
    pub fn elevate(&self, e: El, layers: &[Shadow]) -> El {
        crate::shadows(self, e, layers)
    }

    /// `c` at opacity `a`, as the browser blends it: unchanged in a window that blends in sRGB,
    /// re-solved over `blend_over` in one that blends in linear light (`rune::theme::for_blend`).
    pub fn alpha(&self, c: Color, a: f32) -> Color {
        let ca = c.alpha(a);
        self.blend_over.map_or(ca, |bg| rune::theme::for_blend(ca, bg))
    }

    /// The text selection highlight of the kit's fields. Pass it to `El::selection_color` on
    /// `El::selectable` text so its selection looks the same.
    pub fn selection(&self) -> Color {
        self.alpha(self.c.text_accent, 0.3)
    }

    /// A straight-alpha colour as painted on `backdrop`: re-solved for that surface when the
    /// window blends in linear light, unchanged when it blends in sRGB.
    pub fn on(&self, c: Color, backdrop: Color) -> Color {
        if self.blend_over.is_none() {
            return c;
        }
        rune::theme::for_blend(c, backdrop)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, RandomState};

    #[test]
    fn equal_tokens_hash_equal() {
        let s = RandomState::new();
        let (a, b, light) = (Kit::neutral(true), Kit::neutral(true), Kit::neutral(false));
        assert_eq!(s.hash_one(a.c), s.hash_one(b.c));
        assert_eq!(s.hash_one(a.ty), s.hash_one(b.ty));
        assert_eq!(s.hash_one(a.radius), s.hash_one(b.radius));
        assert_ne!(s.hash_one(a.c), s.hash_one(light.c));
        let negative = Radius { xs: -0.0, ..a.radius };
        assert_eq!(s.hash_one(Radius { xs: 0.0, ..a.radius }), s.hash_one(negative));
        assert_ne!(s.hash_one(a.radius), s.hash_one(Radius { md: a.radius.md + 1.0, ..a.radius }));
        let ty = Type { ui: a.ty.ui.lh(a.ty.ui.line_height + 1.0), ..a.ty };
        assert_ne!(s.hash_one(a.ty), s.hash_one(ty));
        assert_eq!(s.hash_one(a.ty.ui.tracking(0.0)), s.hash_one(a.ty.ui.tracking(-0.0)));
    }
}
