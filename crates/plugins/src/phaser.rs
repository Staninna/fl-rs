//! Fruity Phaser: a chain of first-order all-passes a side, their
//! coefficient swept by a sine LFO that the right side reads further on,
//! with feedback around the chain, as `docs/plugins/fruity-phaser.md`
//! has it.
//!
//! The state is 40 bytes: a little-endian i32 version, 1, and the plugin's
//! nine parameters as i32. FL leaves a state alone whose version is 2 or
//! more, and so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Sweep frequency | 0 to 5000 | 2500 |
//! | 1 Minimum depth | 0 to 1000 | 100 |
//! | 2 Maximum depth | 0 to 1000 | 800 |
//! | 3 Frequency range (small, large) | 0 to 1024 | 0 |
//! | 4 Stereo | 0 to 1024 | 512 |
//! | 5 Number of stages | 0 to 22 | 7 |
//! | 6 Feedback | 0 to 1000 | 400 |
//! | 7 Dry - wet | 0 to 1024 | 512 |
//! | 8 Output gain | 0 to 5000 | 4000 |
//!
//! With `v` a wheel over its end: the LFO turns `(e^(4v) - 1) / (e^4 - 1)`
//! of 2 Hz a second, or of 10 Hz with the range past half, and the right
//! side reads it up to half a cycle on. Each all-pass has the coefficient
//! `-(min + lfo (max - min))`, with the LFO from 0 to 1. There are the
//! stages wheel plus one of them, 3 at the least. The chain's last output
//! goes back to its input at `0.99 v`. The dry - wet wheel goes from all
//! of the chain to all of the input, and the output gain is
//! `2 v sqrt(1 - feedback^2)`.

use crate::{Effect, StateError, knob_value, state_version};
use std::sync::LazyLock;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Phaser";

const COUNT: usize = 9;

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); COUNT] = [
    (0, 5000),
    (0, 1000),
    (0, 1000),
    (0, 1024),
    (0, 1024),
    (0, 22),
    (0, 1000),
    (0, 1024),
    (0, 5000),
];
const DEFAULTS: [i32; COUNT] = [2500, 100, 800, 0, 512, 7, 400, 512, 4000];

/// How long the state is, and the newest version FL reads.
const LEN: usize = 40;
const VERSION: i32 = 1;

/// The most stages a side has.
const STAGES: usize = 23;

/// What FL has for two pi where it builds the LFO.
const TWO_PI: f64 = f64::from_bits(0x4019_21fb_5441_1744);

/// The LFO: a sine from 0 to 1 over 4096 steps, the step to the next
/// entry, and 4096ths of a step.
struct Lfo {
    value: [f32; 4096],
    slope: [f32; 4096],
    under: [f32; 4096],
}

static LFO: LazyLock<Box<Lfo>> = LazyLock::new(|| {
    let mut lfo = Box::new(Lfo {
        value: [0.0; 4096],
        slope: [0.0; 4096],
        under: [0.0; 4096],
    });
    for i in 0..4096 {
        let x = f64::from(i as i32) / 4096.0;
        lfo.under[i] = x as f32;
        lfo.value[i] = (((x * TWO_PI).sin() + 1.0) * 0.5) as f32;
    }
    for i in 0..4096 {
        lfo.slope[i] = lfo.value[(i + 1) % 4096] - lfo.value[i];
    }
    lfo
});

/// The LFO at a 24-bit phase.
fn lfo(phase: u32) -> f32 {
    let lfo = &**LFO;
    let i = ((phase >> 12) & 0xfff) as usize;
    lfo.value[i] + lfo.under[(phase & 0xfff) as usize] * lfo.slope[i]
}

/// `cvtsd2si`: to the nearest, ties to even.
fn round(x: f64) -> i32 {
    x.round_ties_even() as i32
}

/// What a wheel's place is, 0 to 1: its value times FL's f64 scale.
fn place(params: &[i32; COUNT], index: usize) -> f32 {
    let scale = 1.0 / f64::from(RANGES[index].1);
    (f64::from(params[index]) * scale).clamp(0.0, 1.0) as f32
}

/// One side's chain: each stage's last output and last input.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Chain {
    out: [f32; STAGES],
    input: f32,
    /// The chain's last output, which is fed back.
    last: f32,
}

impl Chain {
    const EMPTY: Chain = Chain {
        out: [0.0; STAGES],
        input: 0.0,
        last: 0.0,
    };

    /// `FUN_006bba00`: `x` through `n` all-passes of coefficient `a`.
    fn run(&mut self, x: f32, a: f32, n: usize) -> f32 {
        let mut before = self.out[0];
        self.out[0] = a * (x - self.out[0]) + self.input;
        self.input = x;
        for k in 1..n {
            let old = self.out[k];
            self.out[k] = a * (self.out[k - 1] - old) + before;
            before = old;
        }
        self.out[n - 1]
    }
}

/// Fruity Phaser. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Phaser {
    params: [i32; COUNT],
    /// The wheels' places, 0 to 1.
    v: [f32; COUNT],
    /// The LFO's rate in Hz, and how many stages run.
    rate: f32,
    stages: usize,
    /// 2^24 over the sample rate, which turns Hz into a phase step.
    scale: f32,
    /// The left LFO's phase: 2^24 is a cycle.
    phase: u32,
    chains: [Chain; 2],
}

impl Default for Phaser {
    fn default() -> Self {
        LazyLock::force(&LFO);
        let mut plugin = Phaser {
            params: DEFAULTS,
            v: [0.0; COUNT],
            rate: 0.0,
            stages: 3,
            scale: 0.0,
            phase: 0,
            chains: [Chain::EMPTY; 2],
        };
        plugin.set_sample_rate(44_100.0);
        plugin.set(DEFAULTS);
        plugin
    }
}

impl Phaser {
    /// Take parameters inside their ranges (`FUN_006bb670`).
    fn set(&mut self, params: [i32; COUNT]) {
        self.params = params;
        self.v = std::array::from_fn(|index| place(&params, index));
        let range = if f64::from(self.v[3]) <= 0.5 {
            2.0
        } else {
            10.0
        };
        let sweep = (f64::from(self.v[0] * 4.0).exp() - 1.0) / (4f64.exp() - 1.0);
        self.rate = (sweep * range) as f32;
        self.stages = (params[5] + 1).clamp(3, STAGES as i32) as usize;
    }
}

impl Effect for Phaser {
    /// The chains and the LFO stay as they are, as in FL.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let mut params = [0; COUNT];
        for (param, word) in bytes[4..].as_chunks::<4>().0.iter().enumerate() {
            let value = i32::from_le_bytes(*word);
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        self.set(params);
        Ok(())
    }

    /// FL counts the rate in whole Hz.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            let hz = f64::from(hz.round() as i32 as f32);
            self.scale = (4096.0 / hz * 4096.0) as f32;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let v = self.v;
        let feedback = (f64::from(v[6]) * 0.99) as f32;
        let root = f64::from(1.0 - v[6] * v[6]).sqrt();
        let gain = (f64::from(v[8]) * 2.0 * root) as f32;
        let (min, span) = (v[1], v[2] - v[1]);
        let step = round(f64::from(self.rate * self.scale)) as u32;
        let ahead = round(f64::from(v[4]) * 8_388_608.0) as u32;
        let n = self.stages;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x = [*l, *r];
            let sweep = [lfo(self.phase), lfo(self.phase.wrapping_add(ahead))];
            self.phase = self.phase.wrapping_add(step);
            let mut out = [0.0; 2];
            for side in 0..2 {
                let chain = &mut self.chains[side];
                let a = -min - sweep[side] * span;
                let input = x[side] + chain.last * feedback;
                chain.last = chain.run(input, a, n);
                let y = chain.last;
                out[side] = (v[7] * (x[side] - y) + y) * gain;
            }
            [*l, *r] = out;
        }
    }

    fn param_count(&self) -> usize {
        COUNT
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// FL's flush: the chains emptied and the LFO at its start.
    fn reset(&mut self) {
        self.chains = [Chain::EMPTY; 2];
        self.phase = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, partial, presets, sine, state};

    fn bytes(params: [i32; COUNT]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes
    }

    fn with(params: [i32; COUNT]) -> Phaser {
        let mut plugin = Phaser::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    fn run(plugin: &mut Phaser, l: &[f32], r: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (l.to_vec(), r.to_vec());
        for (l, r) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            plugin.process(l, r);
        }
        (l, r)
    }

    #[test]
    fn the_lfo_is_a_sine_from_0_to_1() {
        assert_eq!(lfo(0), 0.5);
        assert_eq!(lfo(1024 << 12), 1.0);
        assert!(lfo(3072 << 12) < 1e-7);
        // Half way between two entries.
        let between = lfo((100 << 12) + 2048);
        let (a, b) = (lfo(100 << 12), lfo(101 << 12));
        assert!((between - (a + b) / 2.0).abs() < 1e-7);
        // The last entry's step goes back to the first.
        assert!((lfo((4095 << 12) + 4095) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_numbers() {
        let p = Phaser::default();
        let sweep = ((2f64).exp() - 1.0) / (4f64.exp() - 1.0) * 2.0;
        assert!((f64::from(p.rate) - sweep).abs() < 1e-6);
        assert_eq!(p.stages, 8);
        // 2^24 / 44100 a Hz.
        assert!((p.scale - 380.435_4).abs() < 1e-3);
        let large = with([5000, 0, 1000, 1024, 0, 22, 0, 0, 5000]);
        assert!((large.rate - 10.0).abs() < 1e-6);
        assert_eq!(large.stages, 23);
        assert_eq!(with([0, 0, 0, 513, 0, 0, 0, 0, 0]).stages, 3);
        assert_eq!(with([0, 0, 0, 512, 0, 1, 0, 0, 0]).rate, 0.0);
        assert_eq!(with([0, 0, 0, 512, 0, 3, 0, 0, 0]).stages, 4);
    }

    #[test]
    fn with_no_sweep_it_is_the_doc_s_all_pass_chain() {
        // Minimum and maximum depth equal: the coefficient stands still.
        for params in [
            [2500, 300, 300, 0, 512, 7, 0, 0, 2500],
            [100, 650, 650, 1024, 0, 2, 700, 300, 5000],
        ] {
            let mut plugin = with(params);
            let v = plugin.v;
            let a = -f64::from(v[1]);
            let tone = sine(0.021, 3000);
            let (l, r) = run(&mut plugin, &tone, &tone, 128);
            let n = plugin.stages;
            let feedback = f64::from(v[6]) * 0.99;
            let gain = f64::from(v[8]) * 2.0 * (1.0 - f64::from(v[6]).powi(2)).sqrt();
            let wanted = chain_model(&tone, a, n, feedback, f64::from(v[7]), gain);
            let worst = l
                .iter()
                .zip(&wanted)
                .map(|(g, w)| (f64::from(*g) - w).abs());
            assert!(worst.fold(0.0, f64::max) < 1e-4, "{params:?}");
            assert_eq!(l, r);
        }
    }

    /// The chain written out stage by stage, each with its own last input
    /// and output.
    fn chain_model(x: &[f32], a: f64, n: usize, fb: f64, mix: f64, gain: f64) -> Vec<f64> {
        let (mut ins, mut outs) = (vec![0.0f64; n], vec![0.0f64; n]);
        let mut last = 0.0;
        let mut got = Vec::new();
        for x in x {
            let x = f64::from(*x);
            let mut s = x + last * fb;
            for k in 0..n {
                let y = a * (s - outs[k]) + ins[k];
                ins[k] = s;
                outs[k] = y;
                s = y;
            }
            last = s;
            got.push((mix * (x - last) + last) * gain);
        }
        got
    }

    #[test]
    fn the_chain_passes_every_frequency_whole() {
        // Wet only, no feedback, unit gain: a tone comes out at its level.
        let mut plugin = with([0, 500, 500, 0, 0, 7, 0, 0, 2500]);
        for turns in [0.01, 0.1, 0.3] {
            plugin.reset();
            let tone = sine(turns, 8000);
            let (l, _) = run(&mut plugin, &tone, &tone, 256);
            let peak = partial(&l[4000..], turns);
            assert!((peak - 1.0).abs() < 1e-3, "{turns} {peak}");
        }
        // Dry only is the input times the gain.
        let mut plugin = with([0, 500, 500, 0, 0, 7, 600, 1024, 5000]);
        let tone = sine(0.05, 500);
        let (l, _) = run(&mut plugin, &tone, &tone, 100);
        let gain = 2.0 * (1.0 - 0.6f32 * 0.6).sqrt();
        for (got, x) in l.iter().zip(&tone) {
            assert!((got - x * gain).abs() < 1e-5);
        }
    }

    #[test]
    fn the_lfo_sweeps_the_coefficient_and_the_right_side_reads_it_on() {
        let params = [3000, 100, 900, 0, 1024, 3, 0, 0, 2500];
        let mut plugin = with(params);
        let step = round(f64::from(plugin.rate * plugin.scale)) as u32;
        assert_eq!(round(f64::from(plugin.v[4]) * 8_388_608.0), 1 << 23);
        let tone = sine(0.013, 6000);
        let (l, r) = run(&mut plugin, &tone, &tone, 64);
        // Model each side with its own coefficient every frame.
        let side = |offset: u32| {
            let (mut ins, mut outs) = ([0.0f64; 4], [0.0f64; 4]);
            let mut got = Vec::new();
            for (n, x) in tone.iter().enumerate() {
                let phase = step.wrapping_mul(n as u32).wrapping_add(offset);
                let a = -0.1 - f64::from(lfo(phase)) * 0.8;
                let mut s = f64::from(*x);
                for k in 0..4 {
                    let y = a * (s - outs[k]) + ins[k];
                    ins[k] = s;
                    outs[k] = y;
                    s = y;
                }
                got.push(s);
            }
            got
        };
        for (got, wanted) in [(&l, side(0)), (&r, side(1 << 23))] {
            let worst = got
                .iter()
                .zip(&wanted)
                .map(|(g, w)| (f64::from(*g) - w).abs());
            assert!(worst.fold(0.0, f64::max) < 1e-4);
        }
        assert_ne!(l, r);
    }

    #[test]
    fn a_flush_empties_the_chains_and_starts_the_lfo_again() {
        let mut plugin = Phaser::default();
        run(&mut plugin, &[0.3; 900], &[0.1; 900], 300);
        assert!(plugin.phase != 0 && plugin.chains[0] != Chain::EMPTY);
        plugin.reset();
        assert_eq!((plugin.phase, plugin.chains), (0, [Chain::EMPTY; 2]));
    }

    #[test]
    fn the_rate_moves_the_lfo_step() {
        let mut plugin = Phaser::default();
        let slow = plugin.scale;
        plugin.set_sample_rate(88_200.0);
        assert!((plugin.scale * 2.0 - slow).abs() < 1e-3);
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_wheel_is_the_same_plugin_as_its_state() {
        let mut plugin = Phaser::default();
        let wanted = [5000, 0, 1000, 1024, 3, 22, 1000, 0, 17];
        for (index, value) in wanted.iter().enumerate() {
            plugin.set_param(index, *value as f32 + 0.3);
            plugin.process(&mut [0.25; 64], &mut [0.25; 64]);
        }
        plugin.set_param(9, 1.0);
        assert_eq!((plugin.param(9), plugin.param_count()), (0.0, 9));
        let same = with(wanted);
        assert_eq!(
            (plugin.v, plugin.rate, plugin.stages),
            (same.v, same.rate, same.stages)
        );
        // Feedback at its end still gives numbers.
        let (l, _) = run(&mut plugin, &sine(0.01, 5000), &sine(0.01, 5000), 512);
        assert!(l.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([1406, 594, 875, 1024, 512, 22, 541, 592, 3047]);
        let before = plugin.clone();
        for len in [3, 36, 44] {
            let mut wrong = bytes(DEFAULTS);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 40,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 5001), (5, 23), (8, -1)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params)),
                Err(StateError::Range { param, value })
            );
        }
        let mut newer = bytes(DEFAULTS);
        newer[0] = 2;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load_and_play() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert!(!own.is_empty());
        for preset in &all {
            let mut plugin = Phaser::default();
            if plugin.set_state(&preset.state).is_err() {
                assert_ne!(preset.state.len(), LEN, "{}", preset.file);
                continue;
            }
            let (l, r) = run(&mut plugin, &sine(0.01, 6000), &sine(0.02, 6000), 512);
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 20.0),
                "{}",
                preset.file
            );
        }
    }
}
