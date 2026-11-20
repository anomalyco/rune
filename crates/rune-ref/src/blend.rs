//! Scalar W3C separable blending oracle. Inputs and outputs are premultiplied in the
//! caller's working colour space; light with RGB greater than alpha must be normalized first.

#[derive(Clone, Copy, Debug)]
pub enum Separable {
    Multiply,
    Screen,
    Overlay,
    SoftLight,
}

pub fn composite(mode: Separable, src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let (sa, da) = (src[3], dst[3]);
    let mut out = [0.0; 4];
    for c in 0..3 {
        let s = if sa > 0.0 { src[c] / sa } else { 0.0 };
        let b = if da > 0.0 { dst[c] / da } else { 0.0 };
        let blended = match mode {
            Separable::Multiply => b * s,
            Separable::Screen => b + s - b * s,
            Separable::Overlay if b <= 0.5 => 2.0 * b * s,
            Separable::Overlay => 1.0 - 2.0 * (1.0 - b) * (1.0 - s),
            Separable::SoftLight if s <= 0.5 => b - (1.0 - 2.0 * s) * b * (1.0 - b),
            Separable::SoftLight => {
                let d = if b <= 0.25 {
                    ((16.0 * b - 12.0) * b + 4.0) * b
                } else {
                    libm::sqrtf(b.max(0.0))
                };
                b + (2.0 * s - 1.0) * (d - b)
            }
        };
        out[c] = src[c] * (1.0 - da) + sa * da * blended + dst[c] * (1.0 - sa);
    }
    out[3] = sa + da * (1.0 - sa);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_backdrops_preserve_the_source_in_every_mode() {
        let src = [0.12, 0.24, 0.06, 0.3];
        for mode in [
            Separable::Multiply,
            Separable::Screen,
            Separable::Overlay,
            Separable::SoftLight,
        ] {
            assert_eq!(composite(mode, src, [0.0; 4]), src);
        }
    }
}
