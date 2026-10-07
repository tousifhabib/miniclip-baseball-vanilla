//! Random numbers for the rules.
//!
//! The generator is our own so that a seed always gives the same game: a
//! test can replay one, and so can anybody chasing a fault. It is
//! xoshiro256**, started from the seed with SplitMix64, as its authors
//! advise.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Rng {
    state: [u64; 4],
}

impl Rng {
    /// A generator that gives the same numbers every time for the same seed.
    pub fn new(seed: u64) -> Rng {
        let mut mixer = seed;
        let mut next = || {
            mixer = mixer.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = mixer;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        Rng {
            state: [next(), next(), next(), next()],
        }
    }

    /// A seed that differs from one run to the next.
    pub fn seed_from_clock() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64)
    }

    fn next(&mut self) -> u64 {
        let [a, b, c, d] = &mut self.state;
        let result = b.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let shifted = *b << 17;
        *c ^= *a;
        *d ^= *b;
        *b ^= *c;
        *a ^= *d;
        *c ^= shifted;
        *d = d.rotate_left(45);
        result
    }

    /// A number from 0 up to, but not reaching, 1.
    pub fn unit(&mut self) -> f32 {
        // The top 24 bits: every value a float can hold exactly in that span.
        (self.next() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// A number from `low` up to, but not reaching, `high`.
    pub fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    /// A whole number from 0 up to, but not reaching, `count`. Zero if
    /// `count` is zero.
    pub fn below(&mut self, count: u32) -> u32 {
        // The high half of a 64 by 32 bit product: even enough for a game,
        // with no division.
        (((self.next() >> 32) * u64::from(count)) >> 32) as u32
    }

    /// True this share of the time: never for 0, always for 1.
    pub fn chance(&mut self, share: f32) -> bool {
        self.unit() < share
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        let firsts: Vec<u64> = (0..8).map(|_| a.next()).collect();
        let seconds: Vec<u64> = (0..8).map(|_| b.next()).collect();
        assert_eq!(firsts, seconds);
        assert_ne!(
            firsts,
            (0..8).map(|_| Rng::new(8).next()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_first_numbers_for_a_seed_never_change() {
        // If this fails, every recorded game and seeded test has changed
        // with it.
        let mut rng = Rng::new(1);
        assert_eq!(
            [rng.next(), rng.next(), rng.next()],
            [
                0xb3f2_af6d_0fc7_10c5,
                0x853b_5596_4736_4cea,
                0x92f8_9756_082a_4514,
            ]
        );
    }

    #[test]
    fn numbers_stay_inside_what_was_asked_for() {
        let mut rng = Rng::new(3);
        for _ in 0..10_000 {
            let unit = rng.unit();
            assert!((0.0..1.0).contains(&unit));
            let between = rng.between(-5.0, 12.0);
            assert!((-5.0..12.0).contains(&between));
            assert!(rng.below(6) < 6);
        }
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn every_outcome_turns_up_about_as_often() {
        let mut rng = Rng::new(11);
        let mut counts = [0u32; 6];
        for _ in 0..60_000 {
            counts[rng.below(6) as usize] += 1;
        }
        for count in counts {
            assert!((9_500..10_500).contains(&count), "{counts:?}");
        }
    }

    #[test]
    fn chance_follows_its_share() {
        let mut rng = Rng::new(5);
        assert!((0..1000).all(|_| !rng.chance(0.0)));
        assert!((0..1000).all(|_| rng.chance(1.0)));
        let hits = (0..20_000).filter(|_| rng.chance(0.25)).count();
        assert!((4_700..5_300).contains(&hits), "{hits}");
    }
}
