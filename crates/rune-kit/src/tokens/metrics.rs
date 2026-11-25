//! Component metrics a theme can set on [`Kit::metrics`](crate::Kit::metrics): the sizes,
//! padding, radii, text style and colours some controls otherwise fix in code. Each entry is
//! `None` by default, and a `None` entry keeps the control's built-in look, which follows the
//! kit's other tokens (`radius`, `ty`, `c`) as they are when the control is built.

use rune::{Color, TextStyle};

use crate::{ControlSize, Kit};

fn non_neg(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// One value per [`ControlSize`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BySize<T> {
    pub mini: T,
    pub small: T,
    pub regular: T,
    pub large: T,
    pub extra_large: T,
}

impl<T: Copy> BySize<T> {
    /// The value for `size`.
    pub fn get(&self, size: ControlSize) -> T {
        match size {
            ControlSize::Mini => self.mini,
            ControlSize::Small => self.small,
            ControlSize::Regular => self.regular,
            ControlSize::Large => self.large,
            ControlSize::ExtraLarge => self.extra_large,
        }
    }
}

/// A push [`Button`](crate::Button)'s box at one size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonMetrics {
    pub height: f32,
    /// Padding left and right of the label.
    pub padding_x: f32,
    pub radius: f32,
    pub icon: f32,
}

impl ButtonMetrics {
    /// The built-in table, as macOS 27's `NSButton` push buttons measure (16, 20, 24, 28 and 36
    /// points tall), with corners from `k.radius`. Large and ExtraLarge are capsules there; here
    /// they take the theme's larger radii.
    pub fn kit(k: &Kit) -> BySize<ButtonMetrics> {
        let m = |height, padding_x, radius, icon| ButtonMetrics { height, padding_x, radius, icon };
        BySize {
            mini: m(16.0, 8.0, k.radius.sm, 12.0),
            small: m(20.0, 10.0, k.radius.sm, 14.0),
            regular: m(24.0, 12.0, k.radius.md, 16.0),
            large: m(28.0, 14.0, k.radius.lg, 16.0),
            extra_large: m(36.0, 18.0, k.radius.xl, 16.0),
        }
    }
}

/// A compact [`Switch`](crate::Switch)'s track and knob at one size. The track is a capsule and
/// the knob a pill, `inset` points in from the track's edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchMetrics {
    pub track_width: f32,
    pub track_height: f32,
    pub knob_width: f32,
    pub knob_height: f32,
    pub inset: f32,
}

impl SwitchMetrics {
    /// The built-in table: `NSSwitch`'s track and knob, measured on macOS 27.
    pub const NS_SWITCH: BySize<SwitchMetrics> = {
        const fn m(track_width: f32, track_height: f32, knob_width: f32, knob_height: f32, inset: f32) -> SwitchMetrics {
            SwitchMetrics { track_width, track_height, knob_width, knob_height, inset }
        }
        BySize {
            mini: m(36.0, 16.0, 21.0, 14.0, 1.0),
            small: m(44.0, 20.0, 26.0, 16.0, 2.0),
            regular: m(54.0, 24.0, 32.0, 20.0, 2.0),
            large: m(64.0, 28.0, 38.0, 24.0, 2.0),
            extra_large: m(80.0, 36.0, 48.0, 30.0, 3.0),
        }
    };
}

/// How a compact or web [`Switch`](crate::Switch) paints its track and knob over their fills.
/// Shadows stay in `Kit::elevation` (`switch_on`, `switch_off`, `elements`).
///
/// ```
/// # use rune_kit::{Kit, Switch, SwitchLook};
/// let mut k = Kit::neutral(true);
/// k.metrics.switch_look = Some(SwitchLook { track_border: 1.0, ..SwitchLook::flat(&k) });
/// let _ = Switch::new(true).build(&k);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchLook {
    /// The track's sheen: a white gradient that brightens towards its bottom.
    pub sheen: bool,
    /// The knob's depth gradient and its 0.5 point rim, from `c.depth_top` and `c.depth_bottom`.
    pub knob_depth: bool,
    /// The track's border width; 0 draws none.
    pub track_border: f32,
    /// The track border's colour while off.
    pub track_border_off: Color,
    /// The track border's colour while on.
    pub track_border_on: Color,
}

impl SwitchLook {
    /// The built-in look: sheen and knob depth, no track border (`c.border_base` off and
    /// `c.bg_accent` on once a width is set).
    pub fn kit(k: &Kit) -> SwitchLook {
        SwitchLook { sheen: true, knob_depth: true, track_border: 0.0, track_border_off: k.c.border_base, track_border_on: k.c.bg_accent }
    }

    /// Plain fills: no sheen, no knob gradient or rim, no track border.
    pub fn flat(k: &Kit) -> SwitchLook {
        SwitchLook { sheen: false, knob_depth: false, ..SwitchLook::kit(k) }
    }
}

/// A boxed [`TextInput`](crate::TextInput)'s and [`TextArea`](crate::TextArea)'s box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldMetrics {
    /// A `TextInput`'s height unless `.height` is set (at least its line plus 12).
    pub height: f32,
    /// The same with `.large()`.
    pub large_height: f32,
    /// A `TextInput`'s padding left and right of its text, or left of its leading icon. The
    /// text keeps the 6 point gap that follows the field's leading spacer, so with no leading
    /// icon it starts at least 6 points in: a `padding_x` below 6 still starts it at 6. The
    /// right padding is `padding_x` as given.
    pub padding_x: f32,
    pub radius: f32,
    /// The focus outline's width. It is drawn in `c.border_focus`; an invalid field's outline
    /// takes the same shape in `c.danger.fg`.
    pub focus_width: f32,
    /// The focus outline's offset outward from the box's edge, as `El::outline` takes it:
    /// `-focus_width` draws the ring just inside the edge, 0 or more a halo outside it.
    pub focus_offset: f32,
}

impl FieldMetrics {
    /// The built-in box: 28 points tall (32 large), 8 points in, `k.radius.md` corners and a
    /// 1 point focus outline just inside the edge.
    pub fn kit(k: &Kit) -> FieldMetrics {
        FieldMetrics {
            height: 28.0,
            large_height: 32.0,
            padding_x: 8.0,
            radius: k.radius.md,
            focus_width: 1.0,
            focus_offset: -1.0,
        }
    }
}

/// A [`TextArea`](crate::TextArea)'s text style and padding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextAreaMetrics {
    pub style: TextStyle,
    /// Padding on every side.
    pub padding: f32,
}

impl TextAreaMetrics {
    /// The built-in look: `k.ty.ui` on a 1.35 line, 8 points in.
    pub fn kit(k: &Kit) -> TextAreaMetrics {
        TextAreaMetrics { style: TextStyle { line_height: (13.0f32 * 1.35 * 2.0).round() / 2.0, ..k.ty.ui }, padding: 8.0 }
    }
}

/// A checked or mixed [`Checkbox`](crate::Checkbox)'s colours at rest and focused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CheckboxColors {
    pub fill: Color,
    pub border: Color,
    pub mark: Color,
}

impl CheckboxColors {
    /// The built-in colours: `c.surface_weak` with a `c.std_border` edge and a `c.std_icon` mark.
    pub fn kit(k: &Kit) -> CheckboxColors {
        CheckboxColors { fill: k.c.surface_weak, border: k.c.std_border, mark: k.c.std_icon }
    }
}

/// A [`SegmentedControl`](crate::SegmentedControl)'s height, horizontal segment padding and
/// corner radius.
///
/// ```
/// # use rune_kit::{Kit, SegmentedControl, SegmentedMetrics};
/// let mut k = Kit::neutral(true);
/// k.metrics.segmented = Some(SegmentedMetrics { height: 24.0, padding_x: 8.0, ..SegmentedMetrics::kit(&k) });
/// let _ = SegmentedControl::new(&["List", "Grid"], Some(0)).build(&k);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentedMetrics {
    pub height: f32,
    /// Padding left and right of each segment's content.
    pub padding_x: f32,
    pub radius: f32,
}

impl SegmentedMetrics {
    /// The built-in box: 28 points tall, 12 points of side padding and `k.radius.md` corners.
    pub fn kit(k: &Kit) -> SegmentedMetrics {
        SegmentedMetrics { height: 28.0, padding_x: 12.0, radius: k.radius.md }
    }
}

/// A linear [`Progress`](crate::Progress) bar's track height and rim width.
///
/// ```
/// # use rune_kit::{Kit, Progress, ProgressMetrics};
/// let mut k = Kit::neutral(true);
/// k.metrics.progress = Some(ProgressMetrics { border_width: 0.0, ..ProgressMetrics::kit(&k) });
/// let _ = Progress::new(Some(0.6)).build(&k);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressMetrics {
    pub height: f32,
    /// Track rim width; `0.0` omits the border.
    pub border_width: f32,
}

impl ProgressMetrics {
    /// The built-in track: 8 points tall with a 1 point rim.
    pub fn kit(_k: &Kit) -> ProgressMetrics {
        ProgressMetrics { height: 8.0, border_width: 1.0 }
    }
}

/// A [`Keybind`](crate::Keybind) keycap's height (and minimum width), horizontal padding and
/// corner radius.
///
/// ```
/// # use rune_kit::{Keybind, KeybindMetrics, Kit};
/// let mut k = Kit::neutral(true);
/// k.metrics.keybind = Some(KeybindMetrics { height: 18.0, padding_x: 6.0, ..KeybindMetrics::kit(&k) });
/// let _ = Keybind::new(&["⌘", "K"]).build(&k);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeybindMetrics {
    /// Keycap height and minimum width.
    pub height: f32,
    /// Padding left and right of each keycap's label.
    pub padding_x: f32,
    pub radius: f32,
}

impl KeybindMetrics {
    /// The built-in keycap: 14 points tall, 4 points of side padding and `k.radius.xs` corners.
    pub fn kit(k: &Kit) -> KeybindMetrics {
        KeybindMetrics { height: 14.0, padding_x: 4.0, radius: k.radius.xs }
    }
}

/// A standard [`Switch`](crate::Switch)'s (`Switch::standard`) track and thumb geometry.
///
/// ```
/// # use rune_kit::{Kit, StandardSwitchMetrics, Switch};
/// let mut k = Kit::neutral(true);
/// k.metrics.standard_switch = Some(StandardSwitchMetrics {
///     track_width: 32.0,
///     track_height: 20.0,
///     thumb: 16.0,
///     ..StandardSwitchMetrics::kit(&k)
/// });
/// let _ = Switch::new(true).standard().build(&k);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StandardSwitchMetrics {
    pub track_width: f32,
    pub track_height: f32,
    /// Diameter of the checked thumb; unchecked adds the 1 point border on each side.
    pub thumb: f32,
    /// Space between the track edge and the checked thumb.
    pub inset: f32,
}

impl StandardSwitchMetrics {
    /// The built-in standard switch: a 28 × 16 track with a 14 point thumb, 1 point inside.
    pub fn kit(_k: &Kit) -> StandardSwitchMetrics {
        StandardSwitchMetrics { track_width: 28.0, track_height: 16.0, thumb: 14.0, inset: 1.0 }
    }
}

/// Overrides for metrics some controls otherwise fix in code. `Metrics::default()` overrides
/// nothing.
///
/// ```
/// # use rune_kit::{
/// #     BySize, ButtonMetrics, FieldMetrics, KeybindMetrics, Kit, ProgressMetrics,
/// #     SegmentedMetrics, SwitchLook,
/// # };
/// let mut k = Kit::neutral(true);
/// // Tailwind's 24/28/32/36 buttons.
/// let m = |height, padding_x, radius| ButtonMetrics { height, padding_x, radius, icon: 16.0 };
/// k.metrics.button = Some(BySize {
///     mini: m(24.0, 8.0, 6.0),
///     small: m(28.0, 10.0, 8.0),
///     regular: m(32.0, 10.0, 8.0),
///     large: m(36.0, 12.0, 10.0),
///     extra_large: m(40.0, 16.0, 10.0),
/// });
/// // A 32 point field, 11 points in, with a 3 point focus halo outside its edge.
/// k.metrics.field = Some(FieldMetrics {
///     height: 32.0,
///     padding_x: 11.0,
///     radius: 10.0,
///     focus_width: 3.0,
///     focus_offset: 0.0,
///     ..FieldMetrics::kit(&k)
/// });
/// // Dense flat controls.
/// k.metrics.segmented = Some(SegmentedMetrics { height: 24.0, padding_x: 8.0, ..SegmentedMetrics::kit(&k) });
/// k.metrics.disclosure_header_height = Some(26.0);
/// k.metrics.progress = Some(ProgressMetrics { height: 6.0, ..ProgressMetrics::kit(&k) });
/// k.metrics.divider_thickness = Some(0.5);
/// k.metrics.keybind = Some(KeybindMetrics { height: 16.0, ..KeybindMetrics::kit(&k) });
/// k.metrics.text_area_min_height = Some(60.0);
/// k.metrics.switch_look = Some(SwitchLook { track_border: 1.0, ..SwitchLook::flat(&k) });
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    /// Push [`Button`](crate::Button) boxes per [`ControlSize`]; else [`ButtonMetrics::kit`].
    /// The deprecated `ButtonSize` sizes keep their own metrics.
    pub button: Option<BySize<ButtonMetrics>>,
    /// Compact [`Switch`](crate::Switch) sizes per [`ControlSize`]; else
    /// [`SwitchMetrics::NS_SWITCH`]. The web and standard appearances keep their own.
    pub switch: Option<BySize<SwitchMetrics>>,
    /// Compact and web [`Switch`](crate::Switch) sheen, knob depth and track border; else
    /// [`SwitchLook::kit`]. The standard appearance keeps its own.
    pub switch_look: Option<SwitchLook>,
    /// Boxed text fields; else [`FieldMetrics::kit`].
    pub field: Option<FieldMetrics>,
    /// [`TextArea`](crate::TextArea); else [`TextAreaMetrics::kit`].
    pub text_area: Option<TextAreaMetrics>,
    /// Checked [`Checkbox`](crate::Checkbox) colours; else [`CheckboxColors::kit`].
    pub checkbox: Option<CheckboxColors>,

    // Controls and containers (#2362).
    /// [`SegmentedControl`](crate::SegmentedControl) height, side padding and corner radius; else
    /// [`SegmentedMetrics::kit`].
    pub segmented: Option<SegmentedMetrics>,
    /// [`Collapsible`](crate::Collapsible) and [`Accordion`](crate::Accordion) header row height;
    /// else `32.0`.
    pub disclosure_header_height: Option<f32>,
    /// Linear [`Progress`](crate::Progress) track height and rim width; else
    /// [`ProgressMetrics::kit`].
    pub progress: Option<ProgressMetrics>,
    /// [`Divider`](crate::Divider) rule thickness in points; else `1.0`.
    pub divider_thickness: Option<f32>,
    /// [`Keybind`](crate::Keybind) keycap height, side padding and corner radius; else
    /// [`KeybindMetrics::kit`].
    pub keybind: Option<KeybindMetrics>,
    /// Minimum height for non-auto-growing [`TextArea`](crate::TextArea) boxes; else `80.0`.
    pub text_area_min_height: Option<f32>,
    /// Standard [`Switch`](crate::Switch) (`Switch::standard`) track and thumb geometry; else
    /// [`StandardSwitchMetrics::kit`].
    pub standard_switch: Option<StandardSwitchMetrics>,
}

impl Kit {
    pub(crate) fn button_metrics(&self, size: ControlSize) -> ButtonMetrics {
        self.metrics.button.unwrap_or_else(|| ButtonMetrics::kit(self)).get(size)
    }
    pub(crate) fn switch_metrics(&self, size: ControlSize) -> SwitchMetrics {
        self.metrics.switch.unwrap_or(SwitchMetrics::NS_SWITCH).get(size)
    }
    pub(crate) fn switch_look(&self) -> SwitchLook {
        let look = self.metrics.switch_look.unwrap_or_else(|| SwitchLook::kit(self));
        SwitchLook { track_border: non_neg(look.track_border), ..look }
    }
    pub(crate) fn field_metrics(&self) -> FieldMetrics {
        self.metrics.field.unwrap_or_else(|| FieldMetrics::kit(self))
    }
    pub(crate) fn text_area_metrics(&self) -> TextAreaMetrics {
        self.metrics.text_area.unwrap_or_else(|| TextAreaMetrics::kit(self))
    }
    pub(crate) fn checkbox_colors(&self) -> CheckboxColors {
        self.metrics.checkbox.unwrap_or_else(|| CheckboxColors::kit(self))
    }

    pub(crate) fn segmented_metrics(&self) -> SegmentedMetrics {
        let m = self.metrics.segmented.unwrap_or_else(|| SegmentedMetrics::kit(self));
        SegmentedMetrics { height: non_neg(m.height), padding_x: non_neg(m.padding_x), radius: non_neg(m.radius) }
    }
    pub(crate) fn disclosure_header_height(&self) -> f32 {
        non_neg(self.metrics.disclosure_header_height.unwrap_or(32.0))
    }
    pub(crate) fn progress_metrics(&self) -> ProgressMetrics {
        let m = self.metrics.progress.unwrap_or_else(|| ProgressMetrics::kit(self));
        ProgressMetrics { height: non_neg(m.height), border_width: non_neg(m.border_width) }
    }
    pub(crate) fn divider_thickness(&self) -> f32 {
        non_neg(self.metrics.divider_thickness.unwrap_or(1.0))
    }
    pub(crate) fn keybind_metrics(&self) -> KeybindMetrics {
        let m = self.metrics.keybind.unwrap_or_else(|| KeybindMetrics::kit(self));
        KeybindMetrics { height: non_neg(m.height), padding_x: non_neg(m.padding_x), radius: non_neg(m.radius) }
    }
    pub(crate) fn text_area_min_height(&self) -> f32 {
        non_neg(self.metrics.text_area_min_height.unwrap_or(80.0))
    }
    pub(crate) fn standard_switch_metrics(&self) -> StandardSwitchMetrics {
        let m = self.metrics.standard_switch.unwrap_or_else(|| StandardSwitchMetrics::kit(self));
        StandardSwitchMetrics {
            track_width: non_neg(m.track_width),
            track_height: non_neg(m.track_height),
            thumb: non_neg(m.thumb),
            inset: non_neg(m.inset),
        }
    }
    pub(crate) fn radius_sm(&self) -> f32 {
        non_neg(self.radius.sm)
    }
    pub(crate) fn radius_md(&self) -> f32 {
        non_neg(self.radius.md)
    }
}
