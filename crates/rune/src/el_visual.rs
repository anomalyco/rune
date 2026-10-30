//! `Visual`: hot inline paint fields on `El` and `Node`, with rarely set extras behind `Cold`.
use std::ops::Deref;
use std::rc::Rc;

use rune_motion::{Anim, Transition};

use crate::cold::{Cold, Empty};
use crate::color::Color;
use crate::el::{Outline, RadialGradient, Shadow};
use crate::shape::Shape;
use crate::text::Icon;

/// Rarely set visual extras shared behind `Cold<VisualCold>` so common boxes and text pay 8 bytes.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct VisualCold {
    pub surface: Option<(crate::NativeSurface, crate::SurfaceFit)>,
    /// Bottom colour of a vertical border gradient whose top is `border` (`El::border_gradient`).
    pub border_bottom: Option<Color>,
    /// Dash and gap lengths of a dashed border, in points (`El::border_dash`).
    pub border_dash: Option<[f32; 2]>,
    /// Points into the dash pattern where it starts (`El::border_dash_phase`).
    pub border_dash_phase: f32,
    pub shadows: Vec<Shadow>,
    pub blur_motion: Option<Box<Anim>>,
    /// Soft clip of children to this rect: fade widths left, top, right, bottom.
    pub clip: Option<[f32; 4]>,
    /// Rounded clip of this element and its subtree, applied after subtree filters.
    pub clip_radius: Option<f32>,
    /// Per-corner clip radii override (`[tl, tr, br, bl]`, in points).
    pub(crate) clip_corners: Option<[f32; 4]>,
    pub icon: Option<(&'static Icon, Color)>,
    /// Vertical linear gradient (top, bottom) painted over the fill, under hover/press overlays.
    pub gradient: Option<Box<[Color; 2]>>,
    /// Radial or angled linear gradient painted over the fill, under hover/press overlays.
    pub(crate) radial: Option<Box<RadialGradient>>,
    pub outline: Option<Outline>,
    /// Vector strokes and fills in the element's own points, over the fill.
    pub shape: Option<Rc<Shape>>,
    /// An RGBA image scaled into the element's rect, over the fill and shape.
    pub pixels: Option<crate::pixels::Pixels>,
    /// How `pixels` keeps its aspect ratio; `None` stretches it to the rect.
    pub pixels_fit: Option<crate::SurfaceFit>,
    /// An effect slot (`El::effect`): `(bleed_pt, needs_backdrop)`.
    pub(crate) effect: Option<(f32, bool)>,
    /// Backdrop blur standard deviation in points (`El::backdrop_blur`); 0 is off.
    pub(crate) backdrop_blur: f32,
    /// The hover track's enter and exit curves (default: snappy spring both ways).
    pub hover_motion: Option<Rc<(Anim, Anim)>>,
    /// Hover curves for this element and the hover tracks below it without their own.
    pub(crate) hover_within: Option<Rc<(Anim, Anim)>>,
    /// Opacity factor (rest, hovered) following the nearest ancestor group's hover.
    pub group_opacity: Option<[f32; 2]>,
    /// Text or icon colour while the nearest ancestor group is hovered.
    pub group_color: Option<Color>,
    /// `El::icon_rotation`: the icon's turn in radians, clockwise.
    pub(crate) icon_turn: f32,
    /// Optional app-supplied key mixed into the cached subtree's signature (`El::cache_key`).
    pub(crate) cache_key: Option<u64>,
    pub(crate) offset_motion: Option<Box<Anim>>,
    /// The curve `El::scale` changes on.
    pub(crate) scale_motion: Option<Box<Anim>>,
    /// `El::origin`: the unit point scale and rotation pivot about; `None` is the centre.
    pub(crate) origin: Option<[f32; 2]>,
    /// The curve `El::rotate` changes on.
    pub(crate) rotate_motion: Option<Box<Anim>>,
    /// `El::child_exit`: how children removed in a commit leave, in place of their own.
    pub(crate) child_exit: Option<Box<Transition>>,
    /// `El::enter_delay`: seconds a new element holds its insertion effect before entering.
    pub(crate) enter_delay: f32,
}

impl Empty for VisualCold {
    const EMPTY: &'static Self = &VisualCold {
        surface: None,
        border_bottom: None,
        border_dash: None,
        border_dash_phase: 0.0,
        shadows: Vec::new(),
        blur_motion: None,
        clip: None,
        clip_radius: None,
        clip_corners: None,
        icon: None,
        gradient: None,
        radial: None,
        outline: None,
        shape: None,
        pixels: None,
        pixels_fit: None,
        effect: None,
        backdrop_blur: 0.0,
        hover_motion: None,
        hover_within: None,
        group_opacity: None,
        group_color: None,
        icon_turn: 0.0,
        cache_key: None,
        offset_motion: None,
        scale_motion: None,
        origin: None,
        rotate_motion: None,
        child_exit: None,
        enter_delay: 0.0,
    };
}

#[derive(Clone, Debug)]
pub struct Visual {
    pub fill: Color,
    pub border: Color,
    pub hover: Color,
    pub press: Color,
    pub radius: [f32; 4],
    /// `El::offset`: a visual shift in points, and the curve it moves on.
    pub(crate) offset: [f32; 2],
    pub border_width: f32,
    pub opacity: f32,
    /// Static alpha of this element's own paint; never inherited or isolated.
    pub paint_alpha: f32,
    pub blur: f32,
    /// `El::rotate`, in radians clockwise.
    pub(crate) rotate: f32,
    /// App-defined tag a compositor reads per instance (`Runtime::marks`); 0 inherits the parent's.
    pub mark: u32,
    /// Rectangular native input region for overlays using `OverlayInput::Regions`.
    pub hit: bool,
    /// Corners follow Apple's continuous curve instead of circular arcs (`El::continuous_corners`).
    pub continuous_corners: bool,
    pub blend: crate::render::Blend,
    /// Tracks hover for its descendants' `group_*` styles even without a hover colour. Any
    /// element with a hover colour is a group too.
    pub hover_group: bool,
    /// Rasterize this subtree once into an offscreen texture when settled (`El::cache`).
    pub(crate) cache: bool,
    /// `El::raise`: paints and takes input above the rest of the window.
    pub(crate) raise: bool,
    /// `El::pixel_snap(false)`: this subtree paints at its exact fractional rects.
    pub(crate) no_pixel_snap: bool,
    /// `El::ride_trailing`: x holds against the parent's trailing edge while its width moves.
    pub(crate) ride_trailing: bool,
    pub(crate) cold: Cold<VisualCold>,
}

impl Visual {
    #[inline]
    pub(crate) fn cold_mut(&mut self) -> &mut VisualCold {
        &mut self.cold
    }
}

impl Deref for Visual {
    type Target = VisualCold;
    #[inline(always)]
    fn deref(&self) -> &VisualCold {
        &self.cold
    }
}

impl Default for Visual {
    fn default() -> Self {
        Visual {
            fill: Color::TRANSPARENT,
            border: Color::TRANSPARENT,
            hover: Color::TRANSPARENT,
            press: Color::TRANSPARENT,
            radius: [0.0; 4],
            offset: [0.0; 2],
            border_width: 0.0,
            opacity: 1.0,
            paint_alpha: 1.0,
            blur: 0.0,
            rotate: 0.0,
            mark: 0,
            hit: false,
            continuous_corners: false,
            blend: crate::render::Blend::SourceOver,
            hover_group: false,
            cache: false,
            raise: false,
            no_pixel_snap: false,
            ride_trailing: false,
            cold: Cold::default(),
        }
    }
}
