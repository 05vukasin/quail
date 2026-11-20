//! A small deterministic PRNG (xorshift64*): the same seed gives the same inputs everywhere.

/// Pseudo-random generator; never zero inside.
pub struct Rng(u64);

impl Rng {
    /// A generator for `seed` and stream `stream` (one per job), mixed with `SplitMix64` so
    /// close seeds give unrelated sequences.
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut z = seed ^ stream.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        Self(z | 1)
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `0..n` (0 when `n` is 0).
    #[allow(clippy::cast_possible_truncation)]
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi.saturating_sub(lo) + 1)
    }

    /// True with probability `num / den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next_u64() % den < num
    }

    /// One element of a non-empty slice.
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }

    /// A size biased towards small values: 1, 2, 4 ... up to `max`, each power about equally likely.
    pub fn log_size(&mut self, max: usize) -> usize {
        let bits = usize::BITS - max.max(1).leading_zeros();
        let b = self.below(bits as usize + 1);
        let hi = (1usize << b).min(max.max(1));
        self.range(hi / 2 + usize::from(hi == 1), hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = Rng::new(7, 1);
        let mut b = Rng::new(7, 1);
        let mut c = Rng::new(7, 2);
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        let zs: Vec<u64> = (0..8).map(|_| c.next_u64()).collect();
        assert_eq!(xs, ys);
        assert_ne!(xs, zs);
        for _ in 0..1000 {
            assert!(a.below(10) < 10);
            let s = a.log_size(1000);
            assert!((1..=1000).contains(&s), "{s}");
            let r = a.range(3, 5);
            assert!((3..=5).contains(&r));
        }
    }
}
