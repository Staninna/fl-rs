//! Ticks to samples.

use crate::mix::BLOCK;

/// The most cells a clock keeps for a tempo that moves: over three hours
/// of song at 48 kHz. Past them the tempo stays what it was in the last.
const CELLS: usize = 1 << 20;

/// Converts a position in ticks to a position in output samples.
///
/// The tempo is the project's for the whole song, unless an automation
/// clip drives it. Then the clock has the song cut into cells of [`BLOCK`]
/// output samples, each at the tempo the clip gives for the tick the cell
/// starts on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Clock {
    samples_per_tick: f64,
    /// The tick each cell starts on and the samples a tick lasts in it,
    /// from the first cell of the song on. The last cell has no end. Empty
    /// for a tempo that stays.
    cells: Vec<(f64, f64)>,
}

fn samples_per_tick(bpm: f64, ppq: u16, sample_rate: u32) -> f64 {
    f64::from(sample_rate) * 60.0 / (bpm * f64::from(ppq))
}

impl Clock {
    /// `None` when the tempo is not a positive number or `ppq` is 0.
    pub fn new(bpm: f64, ppq: u16, sample_rate: u32) -> Option<Self> {
        if !(bpm > 0.0 && bpm.is_finite()) || ppq == 0 {
            return None;
        }
        Some(Clock {
            samples_per_tick: samples_per_tick(bpm, ppq, sample_rate),
            cells: Vec::new(),
        })
    }

    /// The clock of a song whose tempo moves: `bpm_at` gives the tempo at a
    /// tick. It is asked once per cell, up to the cell that starts at or
    /// after tick `end` or output sample `limit`, and that cell's tempo
    /// stays for good. A tempo that is not a positive number reads as the
    /// one before it.
    pub fn with_tempo(
        mut self,
        ppq: u16,
        sample_rate: u32,
        end: u64,
        limit: usize,
        bpm_at: impl Fn(f64) -> f64,
    ) -> Self {
        let (mut tick, mut per_tick) = (0.0, self.samples_per_tick);
        loop {
            let bpm = bpm_at(tick);
            if bpm > 0.0 && bpm.is_finite() {
                per_tick = samples_per_tick(bpm, ppq, sample_rate);
            }
            self.cells.push((tick, per_tick));
            let cells = self.cells.len();
            if tick >= end as f64 || cells >= CELLS || cells.saturating_mul(BLOCK) >= limit {
                return self;
            }
            tick += BLOCK as f64 / per_tick;
        }
    }

    /// The output sample nearest to `tick`.
    pub fn sample(&self, tick: u64) -> usize {
        let tick = tick as f64;
        let started = self.cells.partition_point(|(start, _)| *start <= tick);
        let Some(cell) = started.checked_sub(1) else {
            return (tick * self.samples_per_tick).round() as usize;
        };
        let (start, per_tick) = self.cells[cell];
        ((cell * BLOCK) as f64 + (tick - start) * per_tick).round() as usize
    }

    /// The tick nearest to an output sample.
    pub fn tick(&self, sample: usize) -> u64 {
        self.tick_at(sample).round() as u64
    }

    /// The tick an output sample falls on, with the part of a tick it is
    /// into it.
    pub fn tick_at(&self, sample: usize) -> f64 {
        let Some(last) = self.cells.len().checked_sub(1) else {
            return sample as f64 / self.samples_per_tick;
        };
        let cell = (sample / BLOCK).min(last);
        let (start, per_tick) = self.cells[cell];
        start + (sample - cell * BLOCK) as f64 / per_tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_beat_at_120_bpm_is_24000_samples() {
        let clock = Clock::new(120.0, 96, 48_000).unwrap();
        assert_eq!(clock.sample(0), 0);
        assert_eq!(clock.sample(1), 250);
        assert_eq!(clock.sample(96), 24_000);
        assert_eq!(clock.sample(4 * 96), 96_000);
    }

    #[test]
    fn a_tick_that_falls_between_samples_rounds_to_the_nearest() {
        // 140 BPM at 44.1 kHz: 196.875 samples per tick.
        let clock = Clock::new(140.0, 96, 44_100).unwrap();
        assert_eq!(clock.sample(1), 197);
        assert_eq!(clock.sample(2), 394);
        assert_eq!(clock.sample(96), 18_900);
    }

    #[test]
    fn a_sample_goes_back_to_its_tick() {
        let clock = Clock::new(140.0, 96, 44_100).unwrap();
        for tick in [0, 1, 2, 95, 96, 100_000] {
            assert_eq!(clock.tick(clock.sample(tick)), tick);
        }
        // Half a tick is 98 samples.
        assert_eq!((clock.tick(98), clock.tick(99)), (0, 1));
    }

    #[test]
    fn ppq_scales_the_tick() {
        let clock = Clock::new(120.0, 48, 48_000).unwrap();
        assert_eq!(clock.sample(48), 24_000);
    }

    #[test]
    fn a_tempo_that_moves_stretches_the_ticks_it_covers() {
        // 120 BPM up to tick 96 and 60 from there on: 250 samples a tick,
        // then 500. Cells are 512 samples, so the tempo turns at the first
        // cell that starts at or after tick 96, which is sample 24,064.
        let steady = Clock::new(120.0, 96, 48_000).unwrap();
        let tempo = |tick: f64| if tick < 96.0 { 120.0 } else { 60.0 };
        let clock = steady
            .clone()
            .with_tempo(96, 48_000, 384, usize::MAX, tempo);
        assert_ne!(clock, steady);
        assert_eq!(clock.sample(0), 0);
        assert_eq!(clock.sample(48), 12_000);
        assert_eq!(clock.sample(96), 24_000);
        // 64 samples at the old tempo, then 500 a tick.
        assert_eq!(clock.sample(97), 24_064 + 372);
        assert_eq!(clock.sample(192), 24_064 + 372 + 95 * 500);
        for tick in [0, 1, 95, 96, 97, 200, 384, 5000] {
            assert_eq!(clock.tick(clock.sample(tick)), tick);
        }
        assert_eq!(clock.tick_at(125), 0.5);
        assert!((clock.tick_at(24_064 + 372 + 250) - 97.5).abs() < 1e-9);
        // Past the last cell the tempo stays.
        assert_eq!(clock.sample(10_096) - clock.sample(10_000), 48_000);

        // A tempo that never moves gives the samples of a steady clock.
        let same = steady
            .clone()
            .with_tempo(96, 48_000, 384, usize::MAX, |_| 120.0);
        for tick in [0, 1, 7, 96, 383, 384, 100_000] {
            assert_eq!(same.sample(tick), steady.sample(tick));
        }
        // And a tempo that is no number is the one before it.
        let broken = steady
            .clone()
            .with_tempo(96, 48_000, 384, usize::MAX, |_| f64::NAN);
        assert_eq!(broken.sample(384), 96_000);
    }

    #[test]
    fn a_moving_tempo_is_read_up_to_the_end_or_the_limit() {
        let steady = Clock::new(120.0, 96, 48_000).unwrap();
        // 384 ticks are 96,000 samples: 187.5 cells, and the cell that
        // starts past the end is the last.
        let whole = steady
            .clone()
            .with_tempo(96, 48_000, 384, usize::MAX, |_| 120.0);
        assert_eq!(whole.cells.len(), 189);
        let cut = steady.clone().with_tempo(96, 48_000, 384, 2000, |_| 120.0);
        assert_eq!(cut.cells.len(), 4);
        let none = steady.with_tempo(96, 48_000, 0, 0, |_| 60.0);
        assert_eq!(none.cells.len(), 1);
        assert_eq!(none.sample(96), 48_000);
    }

    #[test]
    fn no_clock_without_a_tempo_or_a_ppq() {
        assert!(Clock::new(0.0, 96, 48_000).is_none());
        assert!(Clock::new(f64::NAN, 96, 48_000).is_none());
        assert!(Clock::new(120.0, 0, 48_000).is_none());
    }
}
