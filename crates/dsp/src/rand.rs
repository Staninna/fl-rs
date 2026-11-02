//! The MSVC runtime's `rand()`, which many of FL's plugins draw their
//! noise and jitter from, each on a seed of its own.

/// One step of MSVC's `rand()` on `seed`: the seed moves on by its linear
/// congruence and the next number, 0 to 32767, is its bits 16 to 30.
#[inline]
pub fn msvc(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(214_013).wrapping_add(2_531_011);
    (*seed >> 16) & 0x7fff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_gives_msvc_s_sequence() {
        // `srand(1)` and five `rand()` calls under MSVC.
        let mut seed = 1;
        let got: Vec<u32> = (0..5).map(|_| msvc(&mut seed)).collect();
        assert_eq!(got, [41, 18467, 6334, 26500, 19169]);
    }

    #[test]
    fn it_stays_in_fifteen_bits() {
        let mut seed = 0x3920a;
        for _ in 0..10_000 {
            assert!(msvc(&mut seed) <= 0x7fff);
        }
    }
}
