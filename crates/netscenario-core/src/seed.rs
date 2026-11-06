//! The seeds of the parts of a run, derived from the seed of the scenario (the start of ENV-01).
//!
//! The derivation is part of what "the same seed gives the same result" means, so it never changes once a
//! release has shipped: a new derivation would give old seeds new loss patterns. A test pins the values.

use crate::backend::Direction;

/// The finalizer of `SplitMix64` (Steele, Lea and Flood): a bijection that spreads small differences over all
/// bits.
const fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The seed for the random loss of one direction of one link: a function of the seed of the scenario, the
/// position of the link in the scenario and the direction. Different links and directions get different
/// seeds, so their patterns do not repeat each other.
#[must_use]
pub fn derive_seed(scenario_seed: u64, link_index: usize, direction: Direction) -> u64 {
    let part = (link_index as u64) << 1
        | match direction {
            Direction::Forward => 0,
            Direction::Reverse => 1,
        };
    mix(mix(scenario_seed) ^ mix(part.wrapping_add(0x9e37_79b9_7f4a_7c15)))
}

/// The seed of the second `netem` queue of a direction (the one behind the rate, which holds the delay, the jitter
/// and the reordering), from the seed of the direction. A direction with a rate and a jitter or a reordering is
/// two queues, and each has a random generator of its own; giving both the same seed would make them draw the
/// same numbers.
#[must_use]
pub fn derive_stage_seed(direction_seed: u64, stage: u32) -> u64 {
    mix(mix(direction_seed) ^ mix(u64::from(stage).wrapping_add(0xd1b5_4a32_d192_ed03)))
}

/// The seed of repetition `index` (from 0) of a set of runs, from the seed of the set (ASR-02, D-074). Every repetition gets
/// the seed of the scenario replaced by this one, so the repetitions draw unrelated loss patterns and the set can be repeated
/// from its seed alone. Pinned like the others.
#[must_use]
pub fn derive_repetition_seed(set_seed: u64, index: u32) -> u64 {
    mix(mix(set_seed) ^ mix(u64::from(index).wrapping_add(0xa24b_aed4_963e_e407)))
}

/// The seed of the bootstrap of one metric of a set of runs, from the seed of the set and the name of the metric
/// (ASR-03, D-074). It depends on the name and not on the place of the metric in the list, so the interval of a metric does not
/// change when another metric appears or goes. The name is hashed with FNV-1a (64 bit), which is fixed and published.
#[must_use]
pub fn derive_bootstrap_seed(set_seed: u64, metric: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in metric.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    mix(mix(set_seed) ^ mix(hash ^ 0x5851_f42d_4c95_7f2d))
}

/// The seed of the numbers that the model of one link draws (a Markov chain, D-090), from the seed of the scenario and the **name**
/// of the link, not its place: a link added before it, or a model on another link, does not change its states. The name is hashed
/// with FNV-1a (64 bit), which is fixed and published. Pinned like the others.
#[must_use]
pub fn derive_model_seed(scenario_seed: u64, link: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in link.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    mix(mix(scenario_seed) ^ mix(hash ^ 0x2545_f491_4f6c_dd1d))
}

/// A seed for a run whose scenario has none. It comes from the random keys that the standard library draws
/// from the system for its hash tables, so two runs get different seeds, and the seed is recorded in the
/// result so that the run can be repeated.
#[must_use]
pub fn draw_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_seeds_differ() {
        let seeds: Vec<u64> = (0..8).map(|_| draw_seed()).collect();
        assert!(seeds.iter().any(|seed| *seed != seeds[0]), "{seeds:?}");
    }

    #[test]
    fn the_same_inputs_give_the_same_seed() {
        assert_eq!(
            derive_seed(1234, 0, Direction::Forward),
            derive_seed(1234, 0, Direction::Forward)
        );
    }

    #[test]
    fn links_directions_and_scenario_seeds_each_change_the_result() {
        let base = derive_seed(1234, 0, Direction::Forward);
        assert_ne!(base, derive_seed(1234, 0, Direction::Reverse));
        assert_ne!(base, derive_seed(1234, 1, Direction::Forward));
        assert_ne!(base, derive_seed(1235, 0, Direction::Forward));
    }

    /// The values are pinned: a change would give old seeds new loss patterns (see the module).
    #[test]
    fn the_derivation_is_pinned() {
        assert_eq!(derive_seed(0, 0, Direction::Forward), PINNED[0]);
        assert_eq!(derive_seed(1234, 0, Direction::Forward), PINNED[1]);
        assert_eq!(derive_seed(1234, 0, Direction::Reverse), PINNED[2]);
        assert_eq!(derive_seed(1234, 3, Direction::Reverse), PINNED[3]);
    }

    /// The seed of a second queue is pinned like the others, for the same reason.
    #[test]
    fn the_seed_of_a_second_queue_differs_and_is_pinned() {
        let direction = derive_seed(1234, 0, Direction::Forward);
        assert_ne!(derive_stage_seed(direction, 1), direction);
        assert_ne!(
            derive_stage_seed(direction, 1),
            derive_stage_seed(direction, 2)
        );
        assert_eq!(derive_stage_seed(direction, 1), PINNED_STAGE);
    }

    #[test]
    fn repetitions_and_bootstraps_get_unrelated_pinned_seeds() {
        let seeds: Vec<u64> = (0..5).map(|i| derive_repetition_seed(7, i)).collect();
        for (i, a) in seeds.iter().enumerate() {
            for b in &seeds[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_ne!(derive_repetition_seed(7, 0), derive_repetition_seed(8, 0));
        // The seed of a repetition is not the seed of the set, and the seed of a link is not either
        assert_ne!(derive_repetition_seed(7, 0), 7);
        assert_ne!(
            derive_repetition_seed(7, 0),
            derive_seed(7, 0, Direction::Forward)
        );
        assert_ne!(
            derive_bootstrap_seed(7, "media.loss_ratio"),
            derive_bootstrap_seed(7, "media.delay.p99")
        );
        assert_eq!(derive_bootstrap_seed(7, "a"), derive_bootstrap_seed(7, "a"));
        assert_eq!(
            [
                derive_repetition_seed(0, 0),
                derive_repetition_seed(1234, 9),
                derive_bootstrap_seed(0, ""),
                derive_bootstrap_seed(1234, "media.delay.p99"),
            ],
            PINNED_SET
        );
    }

    const PINNED_SET: [u64; 4] = [
        15_400_537_398_795_842_965,
        750_609_491_068_823_138,
        871_037_530_182_556_995,
        15_287_341_591_652_727_783,
    ];

    const PINNED_STAGE: u64 = 5_600_420_206_902_464_567;

    const PINNED: [u64; 4] = [
        5_197_578_548_964_807_871,
        18_174_057_026_977_956_097,
        11_207_990_962_961_007_184,
        15_833_557_950_962_919_465,
    ];

    /// Values from an implementation of the same derivation in Python that shares nothing with this code.
    #[test]
    fn the_seed_of_a_model_depends_on_the_name_of_the_link_and_is_pinned() {
        assert_eq!(derive_model_seed(0, "l"), 0x8c3f_ff19_0c9c_e449);
        assert_eq!(derive_model_seed(1234, "sat"), 0xbe51_1a4d_2a86_db5c);
        assert_eq!(derive_model_seed(1234, "l"), 0xceaa_13a0_d788_027c);
        assert_eq!(derive_model_seed(1235, "sat"), 0x248a_bca1_d953_2677);
    }
}
