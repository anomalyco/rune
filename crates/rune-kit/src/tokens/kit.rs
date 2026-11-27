//! The semantic token schema every rune-kit component reads. A theme fills one `Kit` per
//! appearance (see `rune-theme-opencode` for the OpenCode theme, or [`Kit::neutral`]).
//! Components never name raw colours, sizes or fonts outside these fields.

use rune::{Anim, Color, Curve, Easing, El, Icon, Shadow, Spring, TextStyle};

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

/// Elevation tokens as box-shadow layers (outer and inset), painted in order. Build one from
/// [`Elevation::default`], setting its fields, and [`Elevation::with_input`] for the fields'.
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
    /// Text fields and field selects (`--v2-elevation-input`); empty for `button_neutral`'s.
    pub(crate) input: Vec<Shadow>,
}

impl Elevation {
    /// Text fields' and field selects' own shadow, `--v2-elevation-input`: in OpenCode's dark
    /// theme lighter than a button's (no drop, a 16% light ring). Without it fields take
    /// [`Self::button_neutral`].
    pub fn with_input(mut self, layers: impl Into<Vec<Shadow>>) -> Self {
        self.input = layers.into();
        self
    }

    /// The shadow text fields and field selects draw ([`Self::with_input`]).
    pub fn input(&self) -> &[Shadow] {
        if self.input.is_empty() { &self.button_neutral } else { &self.input }
    }
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
    /// Monospace copy (code, ids, hashes): 12 / 18 in the mono slot (`ui-monospace` by default on native, `IBM Plex Mono` on web).
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

fn f32_hash_bits(x: f32) -> u32 {
    if x == 0.0 {
        0
    } else if x.is_nan() {
        f32::NAN.to_bits()
    } else {
        x.to_bits()
    }
}

pub(crate) fn hash_anim<H: std::hash::Hasher>(anim: &Anim, state: &mut H) {
    use std::hash::Hash;
    match anim.curve {
        Curve::Spring(s) => (0u8, f32_hash_bits(s.duration), f32_hash_bits(s.bounce)).hash(state),
        Curve::Tween { duration, easing } => {
            (1u8, f32_hash_bits(duration)).hash(state);
            match easing {
                Easing::Linear => 0u8.hash(state),
                Easing::CubicBezier(b) => {
                    (1u8, [b.x1, b.y1, b.x2, b.y2].map(f32_hash_bits)).hash(state);
                }
                Easing::Steps(n, pos) => {
                    let p = match pos {
                        rune_motion::StepPosition::Start => 0u8,
                        rune_motion::StepPosition::End => 1u8,
                        rune_motion::StepPosition::None => 2u8,
                        rune_motion::StepPosition::Both => 3u8,
                    };
                    (2u8, n, p).hash(state);
                }
                Easing::Piecewise(stops) => {
                    (3u8, stops.len()).hash(state);
                    for [x, y] in stops {
                        (f32_hash_bits(*x), f32_hash_bits(*y)).hash(state);
                    }
                }
            }
        }
        Curve::Decay(d) => (2u8, f32_hash_bits(d.rate)).hash(state),
        Curve::Instant => 3u8.hash(state),
        Curve::Hermite { duration, end_velocity } => {
            (4u8, f32_hash_bits(duration), f32_hash_bits(end_velocity)).hash(state);
        }
    }
    let blend = match anim.blend {
        rune_motion::Blend::Auto => 0u8,
        rune_motion::Blend::Merge => 1u8,
        rune_motion::Blend::Additive => 2u8,
    };
    f32_hash_bits(anim.delay).hash(state);
    blend.hash(state);
    match anim.velocity {
        rune_motion::Velocity::Inherit => 0u8.hash(state),
        rune_motion::Velocity::Zero => 1u8.hash(state),
        rune_motion::Velocity::Uniform(v) => (2u8, f32_hash_bits(v)).hash(state),
    }
    anim.token.hash(state);
}

pub(crate) fn hash_anim_pair<H: std::hash::Hasher>(pair: Option<(Anim, Anim)>, state: &mut H) {
    use std::hash::Hash;
    match pair {
        None => 0u8.hash(state),
        Some((a, b)) => {
            1u8.hash(state);
            hash_anim(&a, state);
            hash_anim(&b, state);
        }
    }
}

/// Motion tokens for controls, overlays, disclosures, and pointer hover/press feedback.
///
/// `hover` and `press` default to `None`, keeping the runtime's built-in tracks (instant hover
/// enter / 0.1 s ease-out exit; `tok::motion::INSTANT` press down / `SNAPPY` or `POP` up) and
/// letting an enclosing [`El::hover_motion_within`](rune::El::hover_motion_within) apply.
///
/// ```
/// # use rune::{Anim, Curve, Easing};
/// # use rune_kit::Kit;
/// let mut k = Kit::neutral(true);
/// k.motion = k.motion
///     .hover_motion(Anim::INSTANT, Curve::tween(0.12, Easing::EASE_OUT))
///     .press_motion(Curve::tween(0.08, Easing::EASE_OUT), Curve::tween(0.16, Easing::EASE_OUT));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// Controls changing state (switch thumb, checkbox, colour changes): ~90–170 ms ease-out on the web.
    pub control: Spring,
    /// Overlays opening and closing (popover, menu, tooltip): ~120–150 ms.
    pub overlay: Spring,
    /// Disclosure transitions (shell `SplitView` panes via `PaneMotion`, and `SidebarSection` collapse/expand).
    pub disclosure: Spring,
    /// Default hover `(enter, exit)` curves for kit controls; `None` keeps the runtime default
    /// (instant enter, 0.1 s ease-out exit). Overridden by explicit [`El::hover_motion`] or an
    /// enclosing [`El::hover_motion_within`], and reduced under Reduce Motion.
    pub hover: Option<(Anim, Anim)>,
    /// Default press `(down, up)` curves for kit controls; `None` keeps the runtime default
    /// (`tok::motion::INSTANT` down, `SNAPPY` / `POP` up). Overridden by explicit
    /// [`El::press_motion`], and reduced under Reduce Motion.
    pub press: Option<(Anim, Anim)>,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            control: rune::tok::motion::SNAPPY,
            overlay: rune::tok::motion::SNAPPY,
            disclosure: rune::tok::motion::DEFAULT,
            hover: None,
            press: None,
        }
    }
}

impl Motion {
    /// Sets the default hover `(enter, exit)` curves for kit controls.
    pub fn hover_motion(mut self, enter: impl Into<Anim>, exit: impl Into<Anim>) -> Self {
        self.hover = Some((enter.into(), exit.into()));
        self
    }

    /// Sets the default press `(down, up)` curves for kit controls.
    pub fn press_motion(mut self, down: impl Into<Anim>, up: impl Into<Anim>) -> Self {
        self.press = Some((down.into(), up.into()));
        self
    }
}

impl std::hash::Hash for Motion {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for s in [self.control, self.overlay, self.disclosure] {
            (f32_hash_bits(s.duration), f32_hash_bits(s.bounce)).hash(state);
        }
        hash_anim_pair(self.hover, state);
        hash_anim_pair(self.press, state);
    }
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
    /// The system contrast theme's highlight while one paints the window (CSS
    /// `forced-colors: active`). Fields then draw their edge as a border in `c.text_base` (2
    /// points and dashed when invalid) and focus as a 2 point ring in this colour, 2 out, so
    /// rest, focus and invalid stay apart in the theme's few colours (`text-input.css`,
    /// opencode#65100).
    pub forced: Option<Color>,
    /// Component metrics a theme sets in place of the built-in ones (button and switch sizes,
    /// field box, text area type, checkbox colours). `Metrics::default()` keeps them all.
    pub metrics: crate::Metrics,
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
        self.apply_interaction_motion(e.hover(hover).press(crate::press_over(hover, pressed)))
    }

    /// Applies [`Motion::hover`] as a token default unless `e` already carries an explicit
    /// [`El::hover_motion`].
    pub(crate) fn apply_hover_motion(&self, e: El) -> El {
        match self.motion.hover {
            Some((enter, exit)) => e.token_hover_motion(enter, exit),
            None => e,
        }
    }

    /// Applies [`Motion::press`] as a token default unless `e` already carries an explicit
    /// [`El::press_motion`].
    pub(crate) fn apply_press_motion(&self, e: El) -> El {
        match self.motion.press {
            Some((down, up)) => e.token_press_motion(down, up),
            None => e,
        }
    }

    /// Applies [`Motion::hover`] and [`Motion::press`] as token defaults where not already set on `e`.
    pub(crate) fn apply_interaction_motion(&self, e: El) -> El {
        self.apply_press_motion(self.apply_hover_motion(e))
    }

    /// Paints `base` or its forced `State::Hover`/`State::Pressed` composite (`crate::over`),
    /// and wires live hover/press overlays when `state == State::Rest && interactive`.
    pub(crate) fn surface_state(
        &self,
        e: El,
        state: crate::State,
        base: Color,
        hover: Color,
        pressed: Color,
        interactive: bool,
    ) -> El {
        let fill = match state {
            crate::State::Hover => crate::over(hover, base),
            crate::State::Pressed => crate::over(pressed, base),
            _ => base,
        };
        let e = e.fill(fill);
        if state == crate::State::Rest && interactive {
            self.interactive_with(e, hover, pressed)
        } else {
            e
        }
    }

    /// Paints `base` (or `hover` composited over `base` when `forced_hover`) and attaches a live
    /// `.hover(hover)` overlay when `live_hover` is true.
    pub(crate) fn hover_surface(
        &self,
        e: El,
        base: Color,
        hover: Color,
        forced_hover: bool,
        live_hover: bool,
    ) -> El {
        let e = e.fill(if forced_hover { crate::over(hover, base) } else { base });
        if live_hover { self.apply_hover_motion(e.hover(hover)) } else { e }
    }

    /// Transparent-base interactive state (inline triggers, segments): paints `c.hover` when
    /// `forced_hover`, `c.pressed` when `state == State::Pressed`, or wires live hover/press
    /// overlays when `live` is true.
    pub(crate) fn overlay_state(&self, e: El, state: crate::State, forced_hover: bool, live: bool) -> El {
        if forced_hover {
            e.fill(self.c.hover)
        } else if state == crate::State::Pressed {
            e.fill(self.c.pressed)
        } else if live {
            self.interactive(e)
        } else {
            e
        }
    }

    /// Attaches the kit's keyboard focus ring (`c.border_focus`) with exact `width` and `offset`.
    pub(crate) fn focus_ring(&self, e: El, width: f32, offset: f32) -> El {
        e.focus_ring(width, offset, self.c.border_focus)
    }

    /// Attaches the kit's keyboard focus ring and paints the matching forced outline when
    /// `forced` is true (e.g. `State::Focus`).
    pub(crate) fn focus_ring_state(&self, e: El, width: f32, offset: f32, forced: bool) -> El {
        let e = self.focus_ring(e, width, offset);
        if forced { e.outline(width, offset, self.c.border_focus) } else { e }
    }

    /// Dims a disabled control at the kit's standard 50% layer opacity when `disabled` is true.
    pub(crate) fn disabled_dim(&self, e: El, disabled: bool) -> El {
        self.disabled_dim_alpha(e, disabled, 0.5)
    }

    /// Dims a disabled control at `alpha` layer opacity when `disabled` is true.
    pub(crate) fn disabled_dim_alpha(&self, e: El, disabled: bool, alpha: f32) -> El {
        if disabled { e.layer_opacity(alpha) } else { e }
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

    /// `layers` as painted (back-to-front, reversing CSS `box-shadow` front-to-back order so the
    /// first CSS shadow paints on top): rings recoloured by [`Kit::ring`] when the theme sets it.
    pub fn shadow_layers<'a>(&'a self, layers: &'a [Shadow]) -> impl Iterator<Item = Shadow> + 'a {
        let ring = self.ring.map(|f| f(&self.c));
        layers.iter().rev().map(move |s| match ring {
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

    /// Wraps `viewport` (an [`El::scroll`](rune::El::scroll) element) with a vertical scroll bar
    /// over its right edge that follows the system "Show scroll bars" preference and rides scroll
    /// offsets through live values with zero view rebuilds per scroll frame.
    #[track_caller]
    pub fn scroll_bar(&self, viewport: El) -> El {
        crate::scroll_bar::wrap(crate::ScrollBarStyle::from_kit(self), viewport, std::panic::Location::caller())
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

    #[test]
    fn state_helpers_match_manual_elements_bit_for_bit() {
        use rune::{Cx, Env, Runtime, col, el};
        use rune::render::{KIND_LAYER, KIND_LAYER_END};

        fn build_helpers(_: &(), _: &mut Cx<()>) -> El {
            let k = Kit::neutral(true);
            let hovered = k.surface_state(el().key("s").size(40.0, 20.0), crate::State::Hover, k.c.bg_base, k.c.hover, k.c.pressed, true);
            let hovered_only = k.hover_surface(el().key("h").size(40.0, 20.0), k.c.bg_base, k.c.hover, true, false);
            let overlay_pressed = k.overlay_state(el().key("o").size(40.0, 20.0), crate::State::Pressed, false, true);
            let focused = k.focus_ring_state(el().key("f").size(40.0, 20.0).fill(k.c.bg_base), 2.0, 1.0, true);
            let dimmed = k.disabled_dim(el().key("d").size(40.0, 20.0).fill(k.c.bg_base).child(el().size(10.0, 10.0).fill(k.c.text_base)), true);
            col().child(hovered).child(hovered_only).child(overlay_pressed).child(focused).child(dimmed)
        }

        fn build_manual(_: &(), _: &mut Cx<()>) -> El {
            let k = Kit::neutral(true);
            let hovered = el().key("s").size(40.0, 20.0).fill(crate::over(k.c.hover, k.c.bg_base));
            let hovered_only = el().key("h").size(40.0, 20.0).fill(crate::over(k.c.hover, k.c.bg_base));
            let overlay_pressed = el().key("o").size(40.0, 20.0).fill(k.c.pressed);
            let focused = el().key("f").size(40.0, 20.0).fill(k.c.bg_base).focus_ring(2.0, 1.0, k.c.border_focus).outline(2.0, 1.0, k.c.border_focus);
            let dimmed = el().key("d").size(40.0, 20.0).fill(k.c.bg_base).child(el().size(10.0, 10.0).fill(k.c.text_base)).layer_opacity(0.5);
            col().child(hovered).child(hovered_only).child(overlay_pressed).child(focused).child(dimmed)
        }

        let mut a = Runtime::new((), build_helpers, (200.0, 200.0), Env::new(true, 1.0));
        let mut b = Runtime::new((), build_manual, (200.0, 200.0), Env::new(true, 1.0));
        a.frame(0.0);
        b.frame(0.0);
        assert_eq!(a.instances.len(), b.instances.len());
        for (ia, ib) in a.instances.iter().zip(&b.instances) {
            assert_eq!((ia.rect, ia.fill, ia.border, ia.params), (ib.rect, ib.fill, ib.border, ib.params));
        }
        let opens = a.instances.iter().filter(|i| i.params[1] == KIND_LAYER).count();
        let closes = a.instances.iter().filter(|i| i.params[1] == KIND_LAYER_END).count();
        assert!(opens >= 1 && opens == closes, "disabled_dim balances compositing layers");
    }
}
