//! A plucked operator: a Karplus-Strong string the operator's table is
//! drawn into at the note's start (`0x86a810`), read at the note's pitch
//! through a one-pole loop filter the damping articulator sets
//! (`0x86aba0`, the pluck branch of the kernel `0x877270`). Section 12 of
//! `docs/plugins/sytrus.md`.

use crate::autogun_engine::rand_uniform;

/// The string's samples at a frequency of 1 Hz: it is drawn at 88.2 kHz
/// whatever the rate (`0x86ab6c`).
const DRAWN: f32 = 88_200.0;
/// The longest string: 5.4 Hz.
pub(super) const LONGEST: usize = 16_384;
/// How many strings an operator has room for at once.
const STRINGS: usize = 64;
/// No string.
pub(super) const NONE: u8 = u8::MAX;

/// An operator's strings, made once so a note takes one without
/// allocating.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Strings {
    buffers: Vec<Vec<f32>>,
    used: Vec<bool>,
}

impl Strings {
    /// Room for [`STRINGS`] strings of up to [`LONGEST`] samples. This
    /// allocates.
    pub(super) fn new() -> Self {
        Strings {
            buffers: vec![vec![0.0; LONGEST]; STRINGS],
            used: vec![false; STRINGS],
        }
    }

    /// A free string, or [`NONE`] when every one sounds.
    pub(super) fn take(&mut self) -> u8 {
        match self.used.iter().position(|used| !used) {
            Some(index) => {
                self.used[index] = true;
                index as u8
            }
            None => NONE,
        }
    }

    pub(super) fn give_back(&mut self, index: u8) {
        if let Some(used) = self.used.get_mut(usize::from(index)) {
            *used = false;
        }
    }

    pub(super) fn release_all(&mut self) {
        self.used.fill(false);
    }

    pub(super) fn buffer(&mut self, index: u8) -> Option<&mut [f32]> {
        self.buffers.get_mut(usize::from(index)).map(|b| &mut b[..])
    }
}

/// Where a voice is in its string (the operator state's +0x124..+0x16c).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pluck {
    /// The string it reads, its length and the place it reads next.
    pub(super) slot: u8,
    len: usize,
    at: usize,
    /// The read position's fraction (+0x154) and its step a sample as a
    /// whole (+0x15c) and a 32-bit fraction (+0x160), and the samples
    /// owed (+0x164).
    frac: u32,
    whole: u32,
    part: u32,
    owed: u32,
    /// The last two samples read (+0x16c, +0x168) and the loop filter's
    /// output (+0x148).
    previous: f32,
    current: f32,
    held: f32,
    /// The loop filter `y = held x keep + x x take` (+0x14c, +0x150), the
    /// keep the period was last worked out for (+0x128), and the string's
    /// length with the filter's delay (+0x12c).
    take: f32,
    keep: f32,
    keep_for: f32,
    period: f32,
}

impl Pluck {
    pub(super) const IDLE: Pluck = Pluck {
        slot: NONE,
        len: 0,
        at: 0,
        frac: 0,
        whole: 0,
        part: 0,
        owed: 0,
        previous: 0.0,
        current: 0.0,
        held: 0.0,
        take: 1.0,
        keep: 0.0,
        keep_for: 0.0,
        period: 0.0,
    };

    /// Draw the string (`0x86a810`): `round(88200 / Hz)` samples (at least
    /// 2) of one cycle of `table` from `phase` (cycles), mixed with noise
    /// from -1 to 1 at `1.5 noise` for a noise wheel at `noise` (its
    /// level), less its mean. `base` is the operator's frequency without
    /// its pitch articulator and `pitch` with it, in cycles a sample;
    /// `damping` the damping articulator kept to 0..1. The noise is
    /// `ippsRandUniform_Direct_32f(-1, 1)` (the pointer at `0x90d668`,
    /// called at `0x86a9fc`) over the string's samples, seeded by the
    /// start phase in 2^32ths of a cycle.
    pub(super) fn start(
        &mut self,
        buffer: &mut [f32],
        table: &[f32],
        (base, pitch, rate): (f32, f32, f32),
        phase: f64,
        noise: f32,
        damping: f32,
    ) {
        let len = (f64::from(DRAWN / (base * rate)).round_ties_even() as i64).max(2) as usize;
        let len = len.min(buffer.len());
        self.len = len;
        let mut at = (phase * 4_294_967_296.0).round_ties_even() as i64 as u32;
        let step = (f64::from(4_294_967_296f32 / len as f32)).round_ties_even() as i64 as u32;
        let (dry, wet) = if noise == 0.0 {
            (1.0, 0.0)
        } else {
            (1.0 - noise, (f64::from(noise) * 1.5) as f32)
        };
        if wet != 0.0 {
            let mut seed = at as i32;
            rand_uniform(&mut buffer[..len], -1.0, 1.0, &mut seed);
        }
        let mut sum = 0.0f32;
        // A table of 2^14 samples, read by the top 14 bits of the phase
        // and between two by the other 18; none for an operator that
        // draws no harmonic.
        let size = table.len().saturating_sub(1);
        let shift = 32 - size.trailing_zeros().min(31);
        let mask = (1u64 << shift) - 1;
        for sample in &mut buffer[..len] {
            let mut v = 0.0;
            if size >= 2 {
                let index = (at >> shift) as usize & (size - 1);
                let next = (at.wrapping_add(1 << shift) >> shift) as usize & (size - 1);
                let t = table[index];
                let frac = (u64::from(at) & mask) as f64 / (mask + 1) as f64;
                v = (f64::from(t) + f64::from((table[next] - t) * frac as f32)) as f32;
            }
            if wet != 0.0 {
                v = v * dry + *sample * wet;
            }
            *sample = v;
            sum += v;
            at = at.wrapping_add(step);
        }
        let mean = sum / len as f32;
        for sample in &mut buffer[..len] {
            *sample -= mean;
        }
        self.held = buffer[len - 1];
        self.at = 0;
        self.frac = 0;
        self.owed = 1;
        self.previous = 0.0;
        self.current = 0.0;
        self.keep_for = f32::NAN;
        self.damp(damping, pitch);
    }

    /// Set the loop filter from the damping and the read step from the
    /// pitch (`0x86aba0`): `take = d 0.99 + 0.01`, `keep = 1 - take`; the
    /// period is the length and the filter's phase delay at `1 / len`
    /// radians, and a sample reads `period x pitch` of the string.
    pub(super) fn damp(&mut self, damping: f32, pitch: f32) {
        self.take = damping * 0.99 + 0.01;
        self.keep = 1.0 - self.take;
        if self.keep_for != self.keep {
            self.keep_for = self.keep;
            let (sin, cos) = (1.0 / self.len as f64).sin_cos();
            let keep = f64::from(self.keep);
            self.period = (self.len as f64 * ((keep * sin) / (1.0 - keep * cos) + 1.0)) as f32;
        }
        let step = f64::from(self.period * pitch.max(0.0));
        let fixed = (step * 4_294_967_296.0) as i64;
        self.whole = (fixed >> 32) as u32;
        self.part = fixed as u32;
    }

    /// The next sample: move on by the step, run every string sample
    /// passed through the loop filter, and read between the last two.
    /// The SSE kernel takes the fraction as a signed 32-bit number.
    pub(super) fn next(&mut self, buffer: &mut [f32]) -> f32 {
        let (frac, carry) = self.frac.overflowing_add(self.part);
        self.frac = frac;
        let count = self
            .owed
            .wrapping_add(self.whole)
            .wrapping_add(u32::from(carry));
        if count != 0 {
            let mut current = self.current;
            for _ in 0..count.min(LONGEST as u32) {
                self.previous = current;
                current = buffer[self.at];
                let y = (f64::from(current) * f64::from(self.take)
                    + f64::from(self.held * self.keep)) as f32;
                buffer[self.at] = y;
                self.held = y;
                self.at += 1;
                if self.at >= self.len {
                    self.at = 0;
                }
            }
            self.current = current;
            self.owed = 0;
        }
        let weight = (frac as i32) as f32 * 2.328_306_4e-10;
        (f64::from(self.current - self.previous) * f64::from(weight) + f64::from(self.previous))
            as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::partial;

    /// One cycle of a sine, 16384 samples and the first again.
    fn sine_table() -> Vec<f32> {
        let mut table: Vec<f32> = (0..16_384)
            .map(|i| (std::f64::consts::TAU * i as f64 / 16_384.0).sin() as f32)
            .collect();
        table.push(table[0]);
        table
    }

    #[test]
    fn a_string_rings_at_its_pitch_and_dies_away_with_its_damping() {
        let rate = 44_100.0f32;
        let hz = 441.0f32;
        let f = hz / rate;
        let mut strings = Strings::new();
        let slot = strings.take();
        let mut pluck = Pluck::IDLE;
        let buffer = strings.buffer(slot).unwrap();
        pluck.start(buffer, &sine_table(), (f, f, rate), 0.0, 0.0, 1.0);
        // 88200 / 441 = 200 samples, two a sample at 44.1 kHz.
        assert_eq!(pluck.len, 200);
        assert_eq!(pluck.whole, 2);
        let out: Vec<f32> = (0..44_100).map(|_| pluck.next(buffer)).collect();
        // Fully open (damping 1) the loop keeps the sine as it is.
        let turns = f64::from(f);
        assert!((partial(&out[1000..5410], turns) - 1.0).abs() < 0.01);
        // Damped, it dies away and its period grows by the filter's delay.
        let mut damped = Pluck::IDLE;
        damped.start(buffer, &sine_table(), (f, f, rate), 0.0, 0.0, 0.2);
        assert!(damped.period > 200.0);
        let out: Vec<f32> = (0..44_100).map(|_| damped.next(buffer)).collect();
        let early = out[..2000].iter().fold(0f32, |p, v| p.max(v.abs()));
        let late = out[40_000..].iter().fold(0f32, |p, v| p.max(v.abs()));
        assert!(late < early * 0.5, "{early} {late}");
        strings.give_back(slot);
        assert_eq!(strings.take(), slot);
    }

    #[test]
    fn the_noise_is_ipp_s_uniform_numbers_seeded_by_the_start_phase() {
        let rate = 44_100.0f32;
        let f = 441.0 / rate;
        let mut strings = Strings::new();
        let slot = strings.take();
        let buffer = strings.buffer(slot).unwrap();
        // No table, so the string is the noise alone: 1.5 x the numbers
        // for a wheel at 1, less their mean.
        let phase = 0.25;
        let mut pluck = Pluck::IDLE;
        pluck.start(buffer, &[], (f, f, rate), phase, 1.0, 1.0);
        let mut numbers = vec![0.0f32; 200];
        let mut seed = 1 << 30;
        rand_uniform(&mut numbers, -1.0, 1.0, &mut seed);
        let wet: Vec<f32> = numbers.iter().map(|n| n * 1.5).collect();
        let mean = wet.iter().sum::<f32>() / 200.0;
        for (got, want) in buffer[..200].iter().zip(&wet) {
            assert_eq!(*got, want - mean);
        }
    }
}
