//! The random numbers of the schedules (WRK-02, D-071): published algorithms, written out here, so that the same seed
//! gives the same packets on every machine and in every version of the tool.
//!
//! - [`SplitMix64`] (Steele, Lea and Flood, 2014) turns one 64 bit seed into the state of the generator.
//! - [`Xoshiro256PlusPlus`] (Blackman and Vigna, 2019) is the generator: 256 bits of state, a period of 2^256 - 1.
//! - A uniform number in `(0, 1]` is the top 53 bits of an output plus one, times 2^-53: an exact double.
//! - An exponential number with mean `m` is `-ln(u) * m`. The logarithm is [`ln`], made of additions, multiplications and
//!   divisions only, which IEEE 754 fixes to the last bit, so it is the same on every processor. The `ln` of the standard
//!   library calls the maths library of the machine, which is allowed to differ in the last bit between systems.
//!
//! The first outputs for fixed seeds are pinned in the tests below and in `docs/AGENT.md`.

// The numbers here are below 2^53, where a double is exact, or are made into a double on purpose
#![allow(clippy::cast_precision_loss)]

use std::f64::consts::LN_2;

/// The generator that seeds the other one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A generator that starts from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The seed of the generator of workload `index`, from the seed of the run: the first output of a `SplitMix64` that starts
/// at the first output of another one started at `base`, mixed with the index. Two workloads of one run, and the same
/// workload in two runs with different seeds, get unrelated sequences.
#[must_use]
pub fn derive_seed(base: u64, index: u64) -> u64 {
    let first = SplitMix64::new(base).next_u64();
    SplitMix64::new(first ^ index.wrapping_mul(0xD1B5_4A32_D192_ED03)).next_u64()
}

/// xoshiro256++ 1.0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xoshiro256PlusPlus {
    state: [u64; 4],
}

impl Xoshiro256PlusPlus {
    /// A generator whose state is the first four outputs of a [`SplitMix64`] started at `seed`.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        let mut mixer = SplitMix64::new(seed);
        Self {
            state: [
                mixer.next_u64(),
                mixer.next_u64(),
                mixer.next_u64(),
                mixer.next_u64(),
            ],
        }
    }

    /// A generator with this state, which must not be all zeros.
    #[must_use]
    pub const fn from_state(state: [u64; 4]) -> Self {
        Self { state }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = self.state;
        let result = s0.wrapping_add(s3).rotate_left(23).wrapping_add(s0);
        let t = s1 << 17;
        let s2 = s2 ^ s0;
        let s3 = s3 ^ s1;
        let s1 = s1 ^ s2;
        let s0 = s0 ^ s3;
        let s2 = s2 ^ t;
        let s3 = s3.rotate_left(45);
        self.state = [s0, s1, s2, s3];
        result
    }

    /// A number in `(0, 1]` with 53 bits: never 0, so its logarithm exists.
    pub fn unit(&mut self) -> f64 {
        // (x >> 11) + 1 is at most 2^53, which a double holds exactly; 2^-53 is a power of two
        (((self.next_u64() >> 11) + 1) as f64) * 1.110_223_024_625_156_5e-16
    }

    /// An exponentially distributed number with the mean `mean`.
    pub fn exponential(&mut self, mean: f64) -> f64 {
        -ln(self.unit()) * mean
    }
}

/// The natural logarithm of a positive, normal double, to within about one unit in the last place, using only the
/// operations whose result IEEE 754 fixes: the exponent is taken apart from the bits, the mantissa `m` is brought to
/// `(1/sqrt 2, sqrt 2]`, and `ln m = 2 atanh((m - 1) / (m + 1))` is summed as a series (the argument is at most 0.1716, so
/// 13 terms are below 10^-17).
#[must_use]
pub fn ln(x: f64) -> f64 {
    debug_assert!(
        x.is_normal() && x > 0.0,
        "ln is for positive normal numbers"
    );
    let bits = x.to_bits();
    let mut exponent = i64::try_from((bits >> 52) & 0x7ff).unwrap_or(0) - 1023;
    let mut mantissa = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if mantissa > std::f64::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }
    let s = (mantissa - 1.0) / (mantissa + 1.0);
    let s2 = s * s;
    let mut sum = 0.0_f64;
    for k in (0..=13_u32).rev() {
        sum = sum * s2 + 1.0 / f64::from(2 * k + 1);
    }
    2.0 * s * sum + (exponent as f64) * LN_2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_gives_the_published_outputs_for_seed_0() {
        let mut mixer = SplitMix64::new(0);
        assert_eq!(
            [
                mixer.next_u64(),
                mixer.next_u64(),
                mixer.next_u64(),
                mixer.next_u64()
            ],
            [
                0xE220_A839_7B1D_CDAF,
                0x6E78_9E6A_A1B9_65F4,
                0x06C4_5D18_8009_454F,
                0xF88B_B8A8_724C_81EC
            ]
        );
    }

    #[test]
    fn xoshiro256_plus_plus_gives_the_reference_outputs() {
        // The state 1, 2, 3, 4 and the first ten outputs of the reference implementation (prng.di.unimi.it)
        let mut generator = Xoshiro256PlusPlus::from_state([1, 2, 3, 4]);
        let outputs: Vec<u64> = (0..10).map(|_| generator.next_u64()).collect();
        assert_eq!(
            outputs,
            [
                41_943_041,
                58_720_359,
                3_588_806_011_781_223,
                3_591_011_842_654_386,
                9_228_616_714_210_784_205,
                9_973_669_472_204_895_162,
                14_011_001_112_246_962_877,
                12_406_186_145_184_390_807,
                15_849_039_046_786_891_736,
                10_450_023_813_501_588_000
            ]
        );
    }

    #[test]
    fn the_seed_gives_pinned_outputs_and_a_different_seed_gives_others() {
        let mut generator = Xoshiro256PlusPlus::from_seed(42);
        let outputs: Vec<u64> = (0..3).map(|_| generator.next_u64()).collect();
        assert_eq!(outputs, PINNED_SEED_42);
        let mut other = Xoshiro256PlusPlus::from_seed(43);
        assert_ne!(other.next_u64(), outputs[0]);
    }

    /// The first three outputs for the seed 42, from an independent implementation in Python (see `docs/AGENT.md`).
    const PINNED_SEED_42: [u64; 3] = [
        15_021_278_609_987_233_951,
        5_881_210_131_331_364_753,
        18_149_643_915_985_481_100,
    ];

    #[test]
    fn the_unit_number_is_never_zero_and_at_most_one() {
        let mut generator = Xoshiro256PlusPlus::from_state([0, 0, 0, 1]);
        for _ in 0..10_000 {
            let u = generator.unit();
            assert!(u > 0.0 && u <= 1.0, "{u}");
        }
        // The extremes: the smallest output gives 2^-53, the largest gives 1
        let low = 1_f64 * 1.110_223_024_625_156_5e-16;
        let high = ((u64::MAX >> 11) + 1) as f64 * 1.110_223_024_625_156_5e-16;
        assert_eq!((low, high), (2.0_f64.powi(-53), 1.0));
    }

    #[test]
    fn the_logarithm_agrees_with_the_library_to_the_last_digits() {
        let mut generator = Xoshiro256PlusPlus::from_seed(7);
        let mut worst = 0.0_f64;
        for _ in 0..100_000 {
            let x = generator.unit();
            worst = worst.max(((ln(x) - x.ln()) / x.ln().abs().max(1e-300)).abs().min(1.0));
        }
        // Near x = 1 the logarithm is near 0 and the relative error is the largest; the absolute one is far smaller
        for x in [1.0, 0.5, 2.0_f64.powi(-53), 0.999_999, 1e-10, 3.0, 1e300] {
            let (mine, theirs) = (ln(x), x.ln());
            assert!(
                (mine - theirs).abs() <= 4e-16 * theirs.abs().max(1.0),
                "{x}: {mine} against {theirs}"
            );
        }
        assert!(worst < 1e-12, "{worst}");
    }

    #[test]
    fn the_exponential_has_the_mean_and_the_median_of_the_distribution() {
        let mut generator = Xoshiro256PlusPlus::from_seed(11);
        let n = 200_000_u32;
        let mut samples: Vec<f64> = (0..n).map(|_| generator.exponential(1000.0)).collect();
        let mean = samples.iter().sum::<f64>() / f64::from(n);
        // The standard error of the mean is 1000 / sqrt(n) = 2.24: four of them
        assert!((mean - 1000.0).abs() < 9.0, "mean {mean}");
        samples.sort_by(f64::total_cmp);
        let median = samples[samples.len() / 2];
        // The median is mean * ln 2 = 693.1; its standard error is 1 / (2 f(m) sqrt(n)) with the density f(m) = 0.0005 at
        // the median, which is 2.24 again: four of them (the seed is fixed, so this either holds or never does)
        assert!((median - 693.1).abs() < 9.0, "median {median}");
    }

    #[test]
    fn derived_seeds_differ_by_index_and_by_base() {
        let seeds = [
            derive_seed(1, 0),
            derive_seed(1, 1),
            derive_seed(2, 0),
            derive_seed(2, 1),
        ];
        for (i, a) in seeds.iter().enumerate() {
            for b in &seeds[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(derive_seed(1, 0), derive_seed(1, 0));
        // Pinned, from the same independent implementation as the outputs above
        assert_eq!(
            [
                derive_seed(1, 0),
                derive_seed(1, 1),
                derive_seed(42, 0),
                derive_seed(42, 3)
            ],
            [
                6_791_897_765_849_424_158,
                2_569_293_373_224_866_520,
                6_332_618_229_526_065_668,
                15_184_021_707_433_295_639
            ]
        );
    }
}
