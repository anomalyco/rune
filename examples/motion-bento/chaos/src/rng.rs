//! SplitMix64: tiny, fast and identical on every platform, so a seed names one show everywhere.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }

    pub fn rangef(&mut self, lo: f32, hi: f32) -> f32 {
        self.range(lo as f64, hi as f64) as f32
    }

    /// Uniform in 0..n.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.f64() * n as f64) as usize).min(n.saturating_sub(1))
    }

    /// An index into `weights`, chosen in proportion to them.
    pub fn weighted(&mut self, weights: &[u32]) -> usize {
        let total: u32 = weights.iter().sum();
        let mut pick = (self.f64() * total as f64) as u32;
        for (i, w) in weights.iter().enumerate() {
            if pick < *w {
                return i;
            }
            pick -= w;
        }
        weights.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_names_one_sequence() {
        let a: Vec<u64> = (0..4).map({
            let mut r = Rng::new(7);
            move |_| r.next_u64()
        }).collect();
        let b: Vec<u64> = (0..4).map({
            let mut r = Rng::new(7);
            move |_| r.next_u64()
        }).collect();
        assert_eq!(a, b);
        assert_ne!(Rng::new(8).next_u64(), a[0]);
    }
}
