//! `ui.*` props that ride on `El`'s motion and paint builders: `gradient` (with `angle`, `radial`,
//! `cx`, `cy`), `loop`, `bounce` and a `hoverTransition` in seconds.

#![allow(unsafe_code)]

use std::os::raw::c_int;

use mlua::ffi;
use rune::{Anim, Color, Curve, Easing, El, LoopSpec, Spring};

use super::Env;
use super::values::{Value, name};

/// The most stops `El::linear_gradient` and `El::radial_gradient` take.
const MAX_STOPS: usize = 16;

/// `gradient` and the props that shape it, applied once the whole table is read.
#[derive(Clone, Default)]
pub(crate) struct Gradient {
    colors: Vec<Color>,
    /// `angle`, in degrees (CSS: `0` bottom-to-top, `90` left-to-right, `180` top-to-bottom).
    angle: Option<f32>,
    /// `radial`, the radius in points: a radial gradient instead of a linear one.
    radial: Option<f32>,
    cx: Option<f32>,
    cy: Option<f32>,
}

impl Gradient {
    pub(crate) fn set_angle(&mut self, deg: f32) {
        self.angle = Some(deg);
    }
    pub(crate) fn set_radial(&mut self, radius: f32) {
        self.radial = Some(radius);
    }
    pub(crate) fn set_cx(&mut self, x: f32) {
        self.cx = Some(x);
    }
    pub(crate) fn set_cy(&mut self, y: f32) {
        self.cy = Some(y);
    }

    /// Reads `gradient = { "#a", "#b", ... }` at stack slot `idx`: 2 to 16 colours, spread evenly.
    pub(crate) unsafe fn read_colors(&mut self, state: *mut ffi::lua_State, idx: c_int, env: &Env) -> Result<(), String> {
        // SAFETY: `idx` is a table; each lookup pushes one value, popped right after.
        let colors = unsafe {
            let idx = ffi::lua_absindex(state, idx);
            let len = ffi::lua_objlen(state, idx);
            if !(2..=MAX_STOPS).contains(&len) {
                return Err(format!("gradient takes 2 to {MAX_STOPS} colours, got {len}"));
            }
            let mut colors = Vec::with_capacity(len);
            for i in 1..=len {
                ffi::lua_rawgeti(state, idx, i as ffi::lua_Integer);
                let res = match super::values::read(state, -1) {
                    Value::Str(s) => env.resolve(s),
                    other => Err(format!("gradient[{i}] must be a color token or ui.hex(...), got {}", name(other))),
                };
                ffi::lua_settop(state, -2);
                colors.push(res?);
            }
            colors
        };
        self.colors = colors;
        Ok(())
    }

    /// What [`Self::apply`] paints: radial when `radial` is set (centred on `cx`, `cy`, `0.5`
    /// each by default), else linear at `angle` degrees (`180`, top to bottom, by default). `None`
    /// without colours, so `angle`, `radial`, `cx` and `cy` alone paint nothing.
    pub(crate) fn geometry(&self) -> Option<Geometry> {
        if self.colors.is_empty() {
            return None;
        }
        Some(match self.radial {
            Some(radius) => Geometry::Radial { center: [self.cx.unwrap_or(0.5), self.cy.unwrap_or(0.5)], radius },
            None => Geometry::Linear { angle: self.angle.unwrap_or(180.0) },
        })
    }

    /// Paints the gradient over `e`'s fill, its colours spread evenly from the first stop to the
    /// last.
    pub(crate) fn apply(&self, e: El) -> El {
        let Some(geometry) = self.geometry() else { return e };
        #[cfg(test)]
        APPLIED.with(|a| a.borrow_mut().push((geometry, self.colors.clone())));
        let last = (self.colors.len() - 1) as f32;
        let stops: Vec<(f32, Color)> = self.colors.iter().enumerate().map(|(i, c)| (i as f32 / last, *c)).collect();
        match geometry {
            Geometry::Radial { center, radius } => e.radial_gradient(center, radius, stops),
            Geometry::Linear { angle } => e.linear_gradient(angle.to_radians(), stops),
        }
    }
}

/// A gradient's shape, as its props give it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Geometry {
    /// `angle` in degrees, CSS's convention.
    Linear { angle: f32 },
    /// Centre in normalized node coordinates, radius in points.
    Radial { center: [f32; 2], radius: f32 },
}

#[cfg(test)]
thread_local! {
    /// Every gradient [`Gradient::apply`] painted on this thread, for tests: Rune has no public
    /// read of an element's gradient.
    pub(crate) static APPLIED: std::cell::RefCell<Vec<(Geometry, Vec<Color>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `loop = "spin:<s>" | "slide:<s>:<dx>:<dy>" | "blink:<s>:<low>"`: a turn every `s` seconds; a
/// sweep to `(dx, dy)` fractions of the node's own size; or opacity dipping to `low` and back.
pub(crate) fn parse_loop(s: &str) -> Result<LoopSpec, String> {
    let mut parts = s.split(':');
    let kind = parts.next().unwrap_or_default();
    let nums: Vec<f64> = parts.map(|p| p.trim().parse::<f64>().map_err(|_| format!("loop {s:?}: {p:?} is not a number"))).collect::<Result<_, _>>()?;
    let period = |nums: &[f64]| match nums.first() {
        Some(&p) if p.is_finite() && p > 0.0 => Ok(p),
        _ => Err(format!("loop {s:?}: the period must be positive seconds")),
    };
    let finite = |n: f64| if n.is_finite() { Ok(n as f32) } else { Err(format!("loop {s:?}: {n} is not finite")) };
    match (kind, nums.len()) {
        ("spin", 1) => Ok(LoopSpec::rotation(period(&nums)?)),
        ("slide", 3) => Ok(LoopSpec::slide(period(&nums)?, [0.0, 0.0], [finite(nums[1])?, finite(nums[2])?])),
        ("blink", 2) => {
            let low = nums[1];
            if !(0.0..=1.0).contains(&low) {
                return Err(format!("loop {s:?}: low must be between 0 and 1"));
            }
            Ok(LoopSpec::opacity(period(&nums)?, 0.0, low as f32, 1.0))
        }
        _ => Err(format!("loop must be \"spin:<s>\", \"slide:<s>:<dx>:<dy>\" or \"blink:<s>:<low>\", got {s:?}")),
    }
}

/// `bounce`: `true` springs `scale` changes on SwiftUI's `.bouncy`; a number in `[0, 1)` is the
/// bounce of a 0.5 s spring; `false` leaves the default curve.
pub(crate) fn bounce(v: Value<'_>) -> Result<Option<Spring>, String> {
    match v {
        Value::Bool(true) => Ok(Some(Spring::BOUNCY)),
        Value::Bool(false) | Value::Nil => Ok(None),
        Value::Num(n) if (0.0..1.0).contains(&n) => Ok(Some(Spring::new(0.5, n as f32))),
        Value::Num(n) => Err(format!("bounce must be at least 0 and below 1, got {n}")),
        other => Err(format!("bounce must be a boolean or number, got {}", name(other))),
    }
}

/// The longest `hoverTransition` in seconds: past it a hover would never visibly land.
const MAX_HOVER_SECONDS: f64 = 60.0;

/// `hoverTransition = <seconds>`: hover enters and leaves on a CSS `ease` tween that long, up
/// to a minute; `0` is instant.
pub(crate) fn hover_seconds(n: f64) -> Result<Anim, String> {
    match n {
        0.0 => Ok(Anim::INSTANT),
        n if n.is_finite() && n > 0.0 && n <= MAX_HOVER_SECONDS => Ok(Curve::tween(n as f32, Easing::EASE).into()),
        n => Err(format!("hoverTransition must be between 0 and {MAX_HOVER_SECONDS} seconds, got {n}")),
    }
}
