//! The arithmetic of a coverage integral over any number of lanes: one `f32`, or four at once
//! ([`F4`]). Every operation is IEEE single precision, rounded to nearest (add, subtract,
//! multiply, divide, square root; compares and selects), lane by lane and in the same order, and
//! nothing is fused or reordered, so each lane of a wide integral gives the very bits the one-lane
//! form does. [`F4`] is SSE2 on x86_64 and simd128 on the web when the build enables it
//! (`-C target-feature=+simd128`); elsewhere it is four `f32`s, which the compiler may vectorize
//! and which give the same bits either way.

use std::ops::{Add, Div, Mul, Neg, Sub};

pub(crate) trait Lanes: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> + Neg<Output = Self> {
    /// One boolean a lane.
    type Mask: Copy;
    const N: usize;
    fn splat(v: f32) -> Self;
    /// Lanes from the first `N` values.
    fn load(v: &[f32; 4]) -> Self;
    /// The first `N` values are the lanes.
    fn store(self) -> [f32; 4];
    fn sqrt(self) -> Self;
    fn lt(self, o: Self) -> Self::Mask;
    fn le(self, o: Self) -> Self::Mask;
    fn ge(self, o: Self) -> Self::Mask;
    fn gt(self, o: Self) -> Self::Mask;
    fn ne(self, o: Self) -> Self::Mask;
    /// `a` where `m`, else `b`.
    fn select(m: Self::Mask, a: Self, b: Self) -> Self;
    /// Whether `a` and `b` agree.
    fn same(a: Self::Mask, b: Self::Mask) -> Self::Mask;
    fn and(a: Self::Mask, b: Self::Mask) -> Self::Mask;
    /// `a` and not `b`.
    fn and_not(a: Self::Mask, b: Self::Mask) -> Self::Mask;
    /// The set lanes, a bit each, lane 0 the lowest.
    fn bits(m: Self::Mask) -> u32;

    /// `f32::clamp`: below `lo` is `lo`, then above `hi` is `hi`.
    #[inline(always)]
    fn clamp(self, lo: Self, hi: Self) -> Self {
        let x = Self::select(self.lt(lo), lo, self);
        Self::select(x.gt(hi), hi, x)
    }

    /// The larger of the two (`b` when they're equal).
    #[inline(always)]
    fn max(self, b: Self) -> Self {
        Self::select(self.gt(b), self, b)
    }
}

impl Lanes for f32 {
    type Mask = bool;
    const N: usize = 1;
    #[inline(always)]
    fn splat(v: f32) -> f32 { v }
    #[inline(always)]
    fn load(v: &[f32; 4]) -> f32 { v[0] }
    #[inline(always)]
    fn store(self) -> [f32; 4] { [self, 0.0, 0.0, 0.0] }
    #[inline(always)]
    fn sqrt(self) -> f32 { f32::sqrt(self) }
    #[inline(always)]
    fn lt(self, o: f32) -> bool { self < o }
    #[inline(always)]
    fn le(self, o: f32) -> bool { self <= o }
    #[inline(always)]
    fn ge(self, o: f32) -> bool { self >= o }
    #[inline(always)]
    fn gt(self, o: f32) -> bool { self > o }
    #[inline(always)]
    fn ne(self, o: f32) -> bool { self != o }
    #[inline(always)]
    fn select(m: bool, a: f32, b: f32) -> f32 { if m { a } else { b } }
    #[inline(always)]
    fn same(a: bool, b: bool) -> bool { a == b }
    #[inline(always)]
    fn and(a: bool, b: bool) -> bool { a && b }
    #[inline(always)]
    fn and_not(a: bool, b: bool) -> bool { a && !b }
    #[inline(always)]
    fn bits(m: bool) -> u32 { u32::from(m) }
}

/// Four lanes as four `f32`s: the form for targets without a vector unit this code uses, and the
/// check that the vector forms give the same bits.
#[derive(Clone, Copy)]
pub(crate) struct Portable([f32; 4]);

macro_rules! portable_op {
    ($tr:ident, $f:ident, $op:tt) => {
        impl $tr for Portable {
            type Output = Portable;
            #[inline(always)]
            fn $f(self, o: Portable) -> Portable { Portable(std::array::from_fn(|i| self.0[i] $op o.0[i])) }
        }
    };
}
portable_op!(Add, add, +);
portable_op!(Sub, sub, -);
portable_op!(Mul, mul, *);
portable_op!(Div, div, /);

impl Neg for Portable {
    type Output = Portable;
    #[inline(always)]
    fn neg(self) -> Portable { Portable(self.0.map(|v| -v)) }
}

impl Lanes for Portable {
    type Mask = [bool; 4];
    const N: usize = 4;
    #[inline(always)]
    fn splat(v: f32) -> Portable { Portable([v; 4]) }
    #[inline(always)]
    fn load(v: &[f32; 4]) -> Portable { Portable(*v) }
    #[inline(always)]
    fn store(self) -> [f32; 4] { self.0 }
    #[inline(always)]
    fn sqrt(self) -> Portable { Portable(self.0.map(f32::sqrt)) }
    #[inline(always)]
    fn lt(self, o: Portable) -> [bool; 4] { std::array::from_fn(|i| self.0[i] < o.0[i]) }
    #[inline(always)]
    fn le(self, o: Portable) -> [bool; 4] { std::array::from_fn(|i| self.0[i] <= o.0[i]) }
    #[inline(always)]
    fn ge(self, o: Portable) -> [bool; 4] { std::array::from_fn(|i| self.0[i] >= o.0[i]) }
    #[inline(always)]
    fn gt(self, o: Portable) -> [bool; 4] { std::array::from_fn(|i| self.0[i] > o.0[i]) }
    #[inline(always)]
    fn ne(self, o: Portable) -> [bool; 4] { std::array::from_fn(|i| self.0[i] != o.0[i]) }
    #[inline(always)]
    fn select(m: [bool; 4], a: Portable, b: Portable) -> Portable { Portable(std::array::from_fn(|i| if m[i] { a.0[i] } else { b.0[i] })) }
    #[inline(always)]
    fn same(a: [bool; 4], b: [bool; 4]) -> [bool; 4] { std::array::from_fn(|i| a[i] == b[i]) }
    #[inline(always)]
    fn and(a: [bool; 4], b: [bool; 4]) -> [bool; 4] { std::array::from_fn(|i| a[i] && b[i]) }
    #[inline(always)]
    fn and_not(a: [bool; 4], b: [bool; 4]) -> [bool; 4] { std::array::from_fn(|i| a[i] && !b[i]) }
    #[inline(always)]
    fn bits(m: [bool; 4]) -> u32 { m.iter().enumerate().map(|(i, &v)| u32::from(v) << i).sum() }
}

#[cfg(target_arch = "x86_64")]
pub(crate) use sse::F4;
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub(crate) use simd128::F4;
#[cfg(not(any(target_arch = "x86_64", all(target_arch = "wasm32", target_feature = "simd128"))))]
pub(crate) type F4 = Portable;

#[cfg(target_arch = "x86_64")]
mod sse {
    use std::arch::x86_64::*;
    use std::ops::{Add, Div, Mul, Neg, Sub};

    use super::Lanes;

    macro_rules! sse {
        ($e:expr) => {
            // SAFETY: SSE2 is part of every x86_64 target, so its intrinsics run on any x86_64
            // CPU; the one store writes four f32s into a four-f32 array.
            unsafe { $e }
        };
    }

    /// Four lanes in an SSE register: `addps`, `subps`, `mulps`, `divps` and `sqrtps` round
    /// exactly as their scalar forms (SSE2 is every x86_64's baseline).
    #[derive(Clone, Copy)]
    pub(crate) struct F4(__m128);

    macro_rules! sse_op {
        ($tr:ident, $f:ident, $op:ident) => {
            impl $tr for F4 {
                type Output = F4;
                #[inline(always)]
                fn $f(self, o: F4) -> F4 { F4(sse!($op(self.0, o.0))) }
            }
        };
    }
    sse_op!(Add, add, _mm_add_ps);
    sse_op!(Sub, sub, _mm_sub_ps);
    sse_op!(Mul, mul, _mm_mul_ps);
    sse_op!(Div, div, _mm_div_ps);

    impl Neg for F4 {
        type Output = F4;
        #[inline(always)]
        fn neg(self) -> F4 { F4(sse!(_mm_xor_ps(self.0, _mm_set1_ps(-0.0)))) }
    }

    impl Lanes for F4 {
        type Mask = __m128;
        const N: usize = 4;
        #[inline(always)]
        fn splat(v: f32) -> F4 { F4(sse!(_mm_set1_ps(v))) }
        #[inline(always)]
        fn load(v: &[f32; 4]) -> F4 { F4(sse!(_mm_loadu_ps(v.as_ptr()))) }
        #[inline(always)]
        fn store(self) -> [f32; 4] {
            let mut out = [0.0f32; 4];
            sse!(_mm_storeu_ps(out.as_mut_ptr(), self.0));
            out
        }
        #[inline(always)]
        fn sqrt(self) -> F4 { F4(sse!(_mm_sqrt_ps(self.0))) }
        #[inline(always)]
        fn lt(self, o: F4) -> __m128 { sse!(_mm_cmplt_ps(self.0, o.0)) }
        #[inline(always)]
        fn le(self, o: F4) -> __m128 { sse!(_mm_cmple_ps(self.0, o.0)) }
        #[inline(always)]
        fn ge(self, o: F4) -> __m128 { sse!(_mm_cmpge_ps(self.0, o.0)) }
        #[inline(always)]
        fn gt(self, o: F4) -> __m128 { sse!(_mm_cmpgt_ps(self.0, o.0)) }
        #[inline(always)]
        fn ne(self, o: F4) -> __m128 { sse!(_mm_cmpneq_ps(self.0, o.0)) }
        #[inline(always)]
        fn select(m: __m128, a: F4, b: F4) -> F4 { F4(sse!(_mm_or_ps(_mm_and_ps(m, a.0), _mm_andnot_ps(m, b.0)))) }
        #[inline(always)]
        fn same(a: __m128, b: __m128) -> __m128 { sse!(_mm_castsi128_ps(_mm_cmpeq_epi32(_mm_castps_si128(a), _mm_castps_si128(b)))) }
        #[inline(always)]
        fn and(a: __m128, b: __m128) -> __m128 { sse!(_mm_and_ps(a, b)) }
        #[inline(always)]
        fn and_not(a: __m128, b: __m128) -> __m128 { sse!(_mm_andnot_ps(b, a)) }
        #[inline(always)]
        fn bits(m: __m128) -> u32 { sse!(_mm_movemask_ps(m)) as u32 }
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod simd128 {
    use std::arch::wasm32::*;
    use std::ops::{Add, Div, Mul, Neg, Sub};

    use super::Lanes;

    /// Four lanes in a wasm `v128`: `f32x4` arithmetic and `sqrt` are IEEE, rounded to nearest.
    #[derive(Clone, Copy)]
    pub(crate) struct F4(v128);

    macro_rules! wasm_op {
        ($tr:ident, $f:ident, $op:ident) => {
            impl $tr for F4 {
                type Output = F4;
                #[inline(always)]
                fn $f(self, o: F4) -> F4 { F4($op(self.0, o.0)) }
            }
        };
    }
    wasm_op!(Add, add, f32x4_add);
    wasm_op!(Sub, sub, f32x4_sub);
    wasm_op!(Mul, mul, f32x4_mul);
    wasm_op!(Div, div, f32x4_div);

    impl Neg for F4 {
        type Output = F4;
        #[inline(always)]
        fn neg(self) -> F4 { F4(f32x4_neg(self.0)) }
    }

    impl Lanes for F4 {
        type Mask = v128;
        const N: usize = 4;
        #[inline(always)]
        fn splat(v: f32) -> F4 { F4(f32x4_splat(v)) }
        #[inline(always)]
        fn load(v: &[f32; 4]) -> F4 { F4(f32x4(v[0], v[1], v[2], v[3])) }
        #[inline(always)]
        fn store(self) -> [f32; 4] {
            [f32x4_extract_lane::<0>(self.0), f32x4_extract_lane::<1>(self.0), f32x4_extract_lane::<2>(self.0), f32x4_extract_lane::<3>(self.0)]
        }
        #[inline(always)]
        fn sqrt(self) -> F4 { F4(f32x4_sqrt(self.0)) }
        #[inline(always)]
        fn lt(self, o: F4) -> v128 { f32x4_lt(self.0, o.0) }
        #[inline(always)]
        fn le(self, o: F4) -> v128 { f32x4_le(self.0, o.0) }
        #[inline(always)]
        fn ge(self, o: F4) -> v128 { f32x4_ge(self.0, o.0) }
        #[inline(always)]
        fn gt(self, o: F4) -> v128 { f32x4_gt(self.0, o.0) }
        #[inline(always)]
        fn ne(self, o: F4) -> v128 { f32x4_ne(self.0, o.0) }
        #[inline(always)]
        fn select(m: v128, a: F4, b: F4) -> F4 { F4(v128_bitselect(a.0, b.0, m)) }
        #[inline(always)]
        fn same(a: v128, b: v128) -> v128 { i32x4_eq(a, b) }
        #[inline(always)]
        fn and(a: v128, b: v128) -> v128 { v128_and(a, b) }
        #[inline(always)]
        fn and_not(a: v128, b: v128) -> v128 { v128_andnot(a, b) }
        #[inline(always)]
        fn bits(m: v128) -> u32 { u32::from(i32x4_bitmask(m)) }
    }
}
