// SPDX-License-Identifier: AGPL-3.0-or-later

//! A tiny deterministic random number generator for fixtures.

/// Deterministic, seedable pseudo-random generator (SplitMix64).
///
/// Fixtures must produce **identical data on every platform and in every
/// release**. Otherwise benchmark results can't be compared over time. So
/// this algorithm is implemented here, pinned by test vectors, and never
/// changed. It is **not** cryptographically secure and must never be used
/// outside tests and benchmarks.
///
/// ```
/// use imaginarium_test_utils::rng::Rng;
///
/// let mut a = Rng::new(42);
/// let mut b = Rng::new(42);
/// assert_eq!(a.next_u64(), b.next_u64());
/// ```
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Creates a generator from a seed. Equal seeds yield equal sequences.
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Returns the next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a float uniformly distributed in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        // The top 53 bits fill an f64 mantissa exactly.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Returns a float uniformly distributed in `[low, high)`.
    pub fn range_f64(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next_f64()
    }

    /// Returns an integer in `[0, bound)`. Returns 0 if `bound` is 0.
    ///
    /// Uses multiply-shift reduction. The bias is below 2⁻³² for any
    /// realistic bound, which is irrelevant for fixtures.
    pub fn below(&mut self, bound: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(bound)) >> 64) as u64
    }

    /// Returns `true` with the given probability (`0.0..=1.0`).
    pub fn chance(&mut self, probability: f64) -> bool {
        self.next_f64() < probability
    }

    /// Picks a random element of a non-empty slice.
    ///
    /// # Panics
    /// If `items` is empty. Fixtures always pass constant, non-empty tables.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        assert!(!items.is_empty(), "Rng::pick on an empty slice");
        &items[self.below(items.len() as u64) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    /// Published SplitMix64 reference outputs (seed 0), and a second seed
    /// cross-checked against an independent implementation. These pin the
    /// algorithm: if this test fails, fixture data changed.
    #[test]
    fn matches_reference_vectors() {
        let mut rng = Rng::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);

        let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
        assert_eq!(rng.next_u64(), 0x1619_22C6_45CE_50E8);
        assert_eq!(rng.next_u64(), 0xAD76_0CAF_A169_7B60);
    }

    #[test]
    fn floats_and_ranges_stay_in_bounds() {
        let mut rng = Rng::new(7);
        for _ in 0..10_000 {
            let f = rng.next_f64();
            assert!((0.0..1.0).contains(&f));
            let r = rng.range_f64(-5.0, 5.0);
            assert!((-5.0..5.0).contains(&r));
            assert!(rng.below(10) < 10);
        }
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn below_covers_the_whole_range() {
        let mut rng = Rng::new(1);
        let mut seen = [false; 6];
        for _ in 0..1_000 {
            seen[rng.below(6) as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }
}
