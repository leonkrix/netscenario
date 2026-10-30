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

#[cfg(test)]
mod tests {
    use super::*;

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

    const PINNED: [u64; 4] = [
        5_197_578_548_964_807_871,
        18_174_057_026_977_956_097,
        11_207_990_962_961_007_184,
        15_833_557_950_962_919_465,
    ];
}
