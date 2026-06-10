//! Tiny deterministic PRNG, private to the crate.
//!
//! Probability trigs must be deterministic and replayable: the same seed,
//! pattern, and call sequence always produce identical output. No global
//! state, no system entropy, no dependencies.

/// Minimal PCG32 (XSH-RR variant).
#[derive(Clone, Debug)]
pub(crate) struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    pub(crate) fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: 0xda3e_39cb_94b9_5bdb | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform draw in `[0.0, 1.0)`.
    pub(crate) fn next_f32(&mut self) -> f32 {
        // 24 mantissa-exact bits.
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Pcg32;

    #[test]
    fn deterministic_per_seed() {
        let a: Vec<f32> = std::iter::repeat_with({
            let mut r = Pcg32::new(7);
            move || r.next_f32()
        })
        .take(64)
        .collect();
        let b: Vec<f32> = std::iter::repeat_with({
            let mut r = Pcg32::new(7);
            move || r.next_f32()
        })
        .take(64)
        .collect();
        let c: Vec<f32> = std::iter::repeat_with({
            let mut r = Pcg32::new(8);
            move || r.next_f32()
        })
        .take(64)
        .collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.iter().all(|v| (0.0..1.0).contains(v)));
    }
}
