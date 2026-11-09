//! The eased zoom: scale and offsets on springs of one shape (`@opencode/ui` `motion/spring.ts`,
//! an exact analytic step), so the point under the pointer stays put while they glide and a new
//! target mid-glide keeps the velocity it has.

use super::math::ZoomView;

/// `FLIGHT_SPRING` (`image-flight.ts`): 0.3 s, no bounce.
const DURATION: f64 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Spring {
    pub value: f64,
    pub target: f64,
    pub velocity: f64,
}

impl Spring {
    fn at(value: f64) -> Spring { Spring { value, target: value, velocity: 0.0 } }

    fn resting(&self) -> bool { (self.value - self.target).abs() < 0.005 && self.velocity.abs() < 0.02 }

    fn step(&mut self, dt: f64) {
        let (x0, v0) = (self.value - self.target, self.velocity);
        if x0 == 0.0 && v0 == 0.0 { return; }
        let t = dt.clamp(0.0, 0.1);
        if t == 0.0 { return; }
        // Critically damped: omega = 2π / duration, zeta = 1.
        let w = std::f64::consts::TAU / DURATION;
        let e = (-w * t).exp();
        let d = e * (1.0 + w * t) * x0 + e * t * v0;
        let velocity = -e * w * w * t * x0 + e * (1.0 - w * t) * v0;
        if d.abs() < 0.005 && velocity.abs() < 0.02 {
            *self = Spring::at(self.target);
            return;
        }
        self.value = self.target + d;
        self.velocity = velocity;
    }
}

/// The view as three springs: what shows is their values, where an eased zoom heads their
/// targets, so repeated steps build on the destination rather than the frame in flight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Glide {
    pub scale: Spring,
    pub x: Spring,
    pub y: Spring,
}

impl Default for Glide {
    fn default() -> Glide { Glide::snap(super::math::FIT_VIEW) }
}

impl Glide {
    /// Pinch, wheel and drag follow the hand directly.
    pub fn snap(view: ZoomView) -> Glide {
        Glide { scale: Spring::at(view.scale.into()), x: Spring::at(view.x.into()), y: Spring::at(view.y.into()) }
    }

    pub fn shown(&self) -> ZoomView {
        ZoomView { scale: self.scale.value as f32, x: self.x.value as f32, y: self.y.value as f32 }
    }

    pub fn goal(&self) -> ZoomView {
        ZoomView { scale: self.scale.target as f32, x: self.x.target as f32, y: self.y.target as f32 }
    }

    pub fn set(&mut self, to: ZoomView) {
        self.scale.target = to.scale.into();
        self.x.target = to.x.into();
        self.y.target = to.y.into();
    }

    /// Steps `dt` seconds; true while any spring still moves.
    pub fn step(&mut self, dt: f64) -> bool {
        for s in [&mut self.scale, &mut self.x, &mut self.y] { s.step(dt); }
        !(self.scale.resting() && self.x.resting() && self.y.resting())
    }
}
