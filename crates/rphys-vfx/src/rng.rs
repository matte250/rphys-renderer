//! A simple 64-bit multiplicative LCG pseudo-random number generator.
//!
//! Uses the same constants as `FollowCamera`'s shake implementation so the
//! VFX layer is reproducible across runs when given the same seed.
//!
//! ```
//! use rphys_vfx::rng::LcgRng;
//! let mut rng = LcgRng::new(42);
//! let v = rng.next_f32();
//! assert!(v >= 0.0 && v < 1.0);
//! ```

/// 64-bit multiplicative LCG.
///
/// Parameters:  
/// - `mul = 6_364_136_223_846_793_005` (same as in `FollowCamera`)  
/// - `add = 1_442_695_040_888_963_407`  
///
/// The sequence is deterministic for a given seed and suitable for particle
/// simulations where statistical quality is not critical.
pub struct LcgRng {
    state: u64,
}

impl LcgRng {
    /// Create a new RNG with the given seed.
    ///
    /// Using a non-zero seed produces a well-distributed sequence.
    pub fn new(seed: u64) -> Self {
        // Ensure we don't start with 0 (produces a degenerate sequence).
        let state = if seed == 0 {
            0xDEAD_BEEF_CAFE_BABEu64
        } else {
            seed
        };
        Self { state }
    }

    /// Advance the state and return the raw 64-bit value.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state
    }

    /// Return a pseudo-random `f32` in the half-open interval `[0.0, 1.0)`.
    ///
    /// Uses the top 32 bits of the LCG output for uniform distribution.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        // Take the top 32 bits and map to [0, 1).
        let bits = (self.next_u64() >> 32) as u32;
        bits as f32 / (u32::MAX as f64 + 1.0) as f32
    }

    /// Return a pseudo-random `f32` in the interval `[lo, hi)`.
    #[inline]
    pub fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lcg_rng_in_unit_range() {
        let mut rng = LcgRng::new(1);
        for _ in 0..10_000 {
            let v = rng.next_f32();
            assert!(v >= 0.0, "next_f32 returned negative: {v}");
            assert!(v < 1.0, "next_f32 returned >= 1.0: {v}");
        }
    }

    #[test]
    fn test_lcg_rng_not_constant() {
        let mut rng = LcgRng::new(99);
        let a = rng.next_f32();
        let b = rng.next_f32();
        // Extremely unlikely to be equal for a functional LCG.
        assert_ne!(a, b, "LCG should not produce identical consecutive values");
    }

    #[test]
    fn test_lcg_rng_range() {
        let mut rng = LcgRng::new(7);
        for _ in 0..1_000 {
            let v = rng.next_range(5.0, 10.0);
            assert!(v >= 5.0 && v < 10.0, "out of range: {v}");
        }
    }

    #[test]
    fn test_lcg_rng_deterministic() {
        let mut a = LcgRng::new(42);
        let mut b = LcgRng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
