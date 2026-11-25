//! Component metrics a theme can set on [`Kit::metrics`](crate::Kit::metrics): the sizes,
//! padding, radii, text style and colours some controls otherwise fix in code. Each entry is
//! `None` by default, and a `None` entry keeps the control's built-in look, which follows the
//! kit's other tokens (`radius`, `ty`, `c`) as they are when the control is built.

use rune::{Color, TextStyle};

use crate::{ControlSize, Kit};

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

/// Overrides for metrics some controls otherwise fix in code. `Metrics::default()` overrides
/// nothing.
///
/// ```
/// # use rune_kit::{BySize, ButtonMetrics, FieldMetrics, Kit};
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
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    /// Push [`Button`](crate::Button) boxes per [`ControlSize`]; else [`ButtonMetrics::kit`].
    /// The deprecated `ButtonSize` sizes keep their own metrics.
    pub button: Option<BySize<ButtonMetrics>>,
    /// Compact [`Switch`](crate::Switch) sizes per [`ControlSize`]; else
    /// [`SwitchMetrics::NS_SWITCH`]. The web and standard appearances keep their own.
    pub switch: Option<BySize<SwitchMetrics>>,
    /// Boxed text fields; else [`FieldMetrics::kit`].
    pub field: Option<FieldMetrics>,
    /// [`TextArea`](crate::TextArea); else [`TextAreaMetrics::kit`].
    pub text_area: Option<TextAreaMetrics>,
    /// Checked [`Checkbox`](crate::Checkbox) colours; else [`CheckboxColors::kit`].
    pub checkbox: Option<CheckboxColors>,
}

impl Kit {
    pub(crate) fn button_metrics(&self, size: ControlSize) -> ButtonMetrics {
        self.metrics.button.unwrap_or_else(|| ButtonMetrics::kit(self)).get(size)
    }
    pub(crate) fn switch_metrics(&self, size: ControlSize) -> SwitchMetrics {
        self.metrics.switch.unwrap_or(SwitchMetrics::NS_SWITCH).get(size)
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
}
