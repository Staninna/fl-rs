//! Fruity Chorus: one band of the input in a delay line a side, read by
//! three taps that three LFOs sweep, under the input, as
//! `docs/plugins/fruity-chorus.md` has it.
//!
//! The state is 52 bytes: a little-endian i32 version, 1, and the plugin's
//! twelve parameters as i32. FL leaves a state alone whose version is 2 or
//! more, and so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Delay | 0 to 1024 | 512 |
//! | 1 Depth | 0 to 5000 | 2250 |
//! | 2 Stereo | 0 to 1024 | 341 |
//! | 3, 4, 5 LFO 1, 2, 3 Frequency | 0 to 5000 | 1500, 2500, 3500 |
//! | 6, 7, 8 LFO 1, 2, 3 Wave | 0 to 1024 | 23, 0, 0 |
//! | 9 Cross Type | 0 to 100 | 0 |
//! | 10 Cross Cutoff | 0 to 5000 | 2500 |
//! | 11 Wet only | 0 to 1024 | 0 |
//!
//! With `v` a wheel over its end: the delay is `29.95 v + 0.05` ms and the
//! depth `5 v` ms, and a tap goes from the delay to the delay and the depth
//! as its LFO goes round. An LFO turns `5 v^2` times a second and reads one
//! of four tables of 4096 steps, the one numbered `trunc(3 v)`: two
//! parabolas for a sine, a sum of three sines, and the cube of each. The
//! right side reads the same LFOs up to half a cycle further on, by the
//! stereo wheel.
//!
//! A one-pole low-pass at MIDI note `127 v` of the cutoff wheel splits the
//! input, at 0.7, in two. What is above it goes into the delay line, or
//! what is under it with the cross type wheel past a quarter. The three
//! taps come out at 0.6 each, on top of the whole input at 0.7, or alone
//! with the wet only wheel past a quarter. Nothing feeds back, and the
//! plugin is on time.
//!
//! The delay line is 4096 samples whatever the rate, and FL counts the
//! taps' places in whole numbers with 12 bits under the sample. So does
//! this: at 192 kHz the longest delay does not fit and comes round, as in
//! FL.
//!
//! FL starts each LFO at a number under 32767 from the Delphi runtime's
//! random numbers, which is in the first 8 of the 4096 steps. This takes
//! the first numbers that generator gives.

use crate::{Effect, StateError, knob_value, state_version};
use std::sync::LazyLock;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Chorus";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 12] = [
    (0, 1024),
    (0, 5000),
    (0, 1024),
    (0, 5000),
    (0, 5000),
    (0, 5000),
    (0, 1024),
    (0, 1024),
    (0, 1024),
    (0, 100),
    (0, 5000),
    (0, 1024),
];
const DEFAULTS: [i32; 12] = [512, 2250, 341, 1500, 2500, 3500, 23, 0, 0, 0, 2500, 0];

/// How long the state is, and the newest version FL reads.
const LEN: usize = 52;
const VERSION: i32 = 1;

/// How many steps a table has and how many samples a ring: both 4096, and
/// what keeps an index inside either.
const STEPS: usize = 4096;
const MASK: u32 = 0xfff;

/// What the input is taken at, and each tap.
const DRY: f64 = 0.7;
const WET: f64 = 0.6;

/// What FL has for pi, three pi and two pi where it builds the third
/// table.
#[expect(
    clippy::approx_constant,
    reason = "the digits FL builds its table with"
)]
const TURNS: [f64; 3] = [3.1415, 9.4245, 6.283];

/// A crossover state smaller than this becomes 0 after a block.
const TINY: f64 = 1e-11;

/// The four LFO shapes, each from 0 to 32766 over its 4096 steps: "sin",
/// "sin^3", "multi-sin" and "multi-sin^3".
static TABLES: LazyLock<Box<[[i32; STEPS]; 4]>> = LazyLock::new(|| {
    let mut tables = Box::new([[0; STEPS]; 4]);
    let entry = |w: f32| f64::from((w + 1.0) * 16_383.0) as i32;
    let cube = |w: f32| w * w * w;
    for i in 0..STEPS {
        let x = (i as f64 / 2048.0) as f32;
        // Two parabolas, one up and one down.
        let sine = if x < 1.0 {
            x * 4.0 * (1.0 - x)
        } else {
            (x - 1.0) * -4.0 * (2.0 - x)
        };
        let x = f64::from(x);
        let others = ((x * TURNS[1]).sin() + (x * TURNS[2]).sin()) * (1.0 / 3.0);
        let multi = ((x * TURNS[0]).sin() + others) * (3.0 / (2f64.sqrt() * 2.0 + 1.0));
        let multi = multi as f32;
        tables[0][i] = entry(sine);
        tables[1][i] = entry(cube(sine));
        tables[2][i] = entry(multi);
        tables[3][i] = entry(cube(multi));
    }
    tables
});

/// The Delphi runtime's `Random(range)`: the seed moved on, and the top of
/// its product with the range.
fn random(seed: &mut u32, range: u32) -> u32 {
    *seed = seed.wrapping_mul(0x0808_8405).wrapping_add(1);
    ((u64::from(*seed) * u64::from(range)) >> 32) as u32
}

/// What the render loop reads, worked out from the wheels and the rate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    /// The delay, in 4096ths of a sample.
    delay: i32,
    /// The depth: a table entry times this, 9 bits down, is 4096ths of a
    /// sample.
    depth: i32,
    /// How many table steps ahead the right side reads.
    stereo: i32,
    /// What a sample adds to each LFO's phase, and the table it reads.
    steps: [i32; 3],
    waves: [usize; 3],
    /// Whether the band under the cutoff is the one in the delay line.
    low: bool,
    wet_only: bool,
    /// The crossover's `k` and `(1 - k) / 2`.
    k: f32,
    k2: f32,
}

impl Settings {
    fn new(params: &[i32; 12], hz: u32) -> Self {
        let v = |index: usize| (f64::from(params[index]) / f64::from(RANGES[index].1)) as f32;
        let rate = hz as f32;
        let step = |index: usize| f64::from(v(index) * v(index) * 83_886_080.0 / rate) as i32;
        let wave = |index: usize| f64::from(v(index) * 3.0) as usize;

        let note = 127.0 * f64::from(v(10));
        let corner = f64::from((2f64.powf((note - 69.0) / 12.0) * 440.0) as f32);
        let corner = corner.min(f64::from(hz) / 4.1);
        let (sin, cos) = (corner / f64::from(hz) * 6.283_184).sin_cos();
        let k = ((1.0 - sin) / cos) as f32;
        Settings {
            delay: ((f64::from(v(0)) * 29.95 + 0.05) * f64::from(hz) * 4.096) as i32,
            depth: (f64::from(v(1) * 5.0 * rate) * 4.096 / 64.0) as i32,
            stereo: (f64::from(v(2) * 4096.0) * 0.5) as i32 - 1,
            steps: [step(3), step(4), step(5)],
            waves: [wave(6), wave(7), wave(8)],
            low: f64::from(v(9)) > 0.25,
            wet_only: f64::from(v(11)) > 0.25,
            k,
            k2: (f64::from(1.0 - k) * 0.5) as f32,
        }
    }

    /// How far an LFO at `phase` pushes the left tap and the right tap
    /// back, in 4096ths of a sample.
    fn sweep(&self, lfo: usize, phase: u32) -> [u32; 2] {
        let table = &TABLES[self.waves[lfo]];
        let at = (phase >> 12) & MASK;
        let ahead = at.wrapping_add_signed(self.stereo) & MASK;
        [at, ahead].map(|step| (table[step as usize].wrapping_mul(self.depth) as u32) >> 9)
    }
}

/// One side's crossover and delay line.
#[derive(Debug, Clone, PartialEq)]
struct Side {
    /// The low-pass and the input before this one.
    low: f32,
    last: f32,
    ring: Vec<f32>,
}

impl Side {
    fn new() -> Self {
        Side {
            low: 0.0,
            last: 0.0,
            ring: vec![0.0; STEPS],
        }
    }

    /// Split the input and put its band in the ring at `write`.
    fn take(&mut self, input: f32, write: usize, settings: &Settings) {
        self.low = settings.k2 * (input + self.last) + settings.k * self.low;
        self.last = input;
        self.ring[write] = if settings.low {
            self.low
        } else {
            input - self.low
        };
    }

    /// The ring at `place`, in 4096ths of a sample, between two samples.
    fn tap(&self, place: u32) -> f32 {
        let at = ((place >> 12) & MASK) as usize;
        let near = self.ring[at];
        let slope = f64::from(self.ring[(at + 1) & MASK as usize] - near);
        (f64::from(near) + slope * f64::from(place & MASK) * (1.0 / 4096.0)) as f32
    }

    fn clear(&mut self) {
        self.low = 0.0;
        self.last = 0.0;
        self.ring.fill(0.0);
    }
}

/// Fruity Chorus. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Chorus {
    params: [i32; 12],
    hz: u32,
    settings: Settings,
    /// Where each LFO is: 2^24 is one cycle.
    phases: [u32; 3],
    /// What the next starting phases come from.
    seed: u32,
    sides: [Side; 2],
    /// Where the next sample goes in the rings.
    write: usize,
}

impl Default for Chorus {
    fn default() -> Self {
        // The tables are made here and not by the first block.
        LazyLock::force(&TABLES);
        let mut plugin = Chorus {
            params: DEFAULTS,
            hz: 44_100,
            settings: Settings::new(&DEFAULTS, 44_100),
            phases: [0; 3],
            seed: 0,
            sides: [Side::new(), Side::new()],
            write: 0,
        };
        plugin.flush();
        plugin
    }
}

impl Chorus {
    /// Take parameters that are inside their ranges, or a new rate.
    fn set(&mut self, params: [i32; 12]) {
        self.params = params;
        self.settings = Settings::new(&params, self.hz);
    }

    /// FL's flush: the lines emptied and each LFO at a new random start.
    fn flush(&mut self) {
        for side in &mut self.sides {
            side.clear();
        }
        self.phases = [0; 3].map(|_| random(&mut self.seed, 0x7fff));
        self.write = 0;
    }
}

impl Effect for Chorus {
    /// The lines and the LFOs stay as they are, as in FL.
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
        let mut params = [0; 12];
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

    /// FL counts the rate in whole Hz. A rate that is not a number of 1 Hz
    /// or more is not taken.
    ///
    /// FL's own plugin keeps the delays and the LFO rates of the rate its
    /// wheels were last moved at. This one works them out again.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.hz = hz.round() as u32;
            self.set(self.params);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let settings = self.settings;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let input = [(f64::from(*l) * DRY) as f32, (f64::from(*r) * DRY) as f32];
            let mut back = [[0; 2]; 3];
            for (lfo, phase) in self.phases.iter_mut().enumerate() {
                back[lfo] = settings.sweep(lfo, *phase);
                *phase = phase.wrapping_add_signed(settings.steps[lfo]);
            }
            let from = ((self.write as u32) << 12).wrapping_sub(settings.delay as u32);
            let mut out = [0.0; 2];
            for (n, side) in self.sides.iter_mut().enumerate() {
                side.take(input[n], self.write, &settings);
                let tap = |lfo: usize| side.tap(from.wrapping_sub(back[lfo][n]));
                let wet = (f64::from(tap(0) + tap(1) + tap(2)) * WET) as f32;
                out[n] = if settings.wet_only {
                    wet
                } else {
                    wet + input[n]
                };
            }
            self.write = (self.write + 1) & MASK as usize;
            [*l, *r] = out;
        }
        for side in &mut self.sides {
            if f64::from(side.low.abs()) < TINY {
                side.low = 0.0;
            }
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
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

    /// Empties the lines and starts the LFOs again, each at the next of
    /// the random numbers.
    fn reset(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, presets, sine, state};

    /// A state of version 1 with these parameters.
    fn bytes(params: [i32; 12]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes
    }

    fn with(params: [i32; 12]) -> Chorus {
        let mut plugin = Chorus::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    /// The doc's chain for one side with places as fractions of a sample
    /// and not as whole numbers. `back` is how far each tap is behind the
    /// sample that just went in, for every frame.
    fn model(input: &[f32], settings: &Settings, back: impl Fn(usize) -> [f64; 3]) -> Vec<f64> {
        let (k, k2) = (f64::from(settings.k), f64::from(settings.k2));
        let (mut low, mut last) = (0.0, 0.0);
        let mut band = Vec::new();
        let mut out = Vec::new();
        for (n, x) in input.iter().enumerate() {
            let x = f64::from(*x) * 0.7;
            low = k2 * (x + last) + k * low;
            last = x;
            band.push(if settings.low { low } else { x - low });
            let at = |place: f64| {
                let whole = place.floor();
                let get = |i: f64| if i < 0.0 { 0.0 } else { band[i as usize] };
                get(whole) + (get(whole + 1.0) - get(whole)) * (place - whole)
            };
            let wet: f64 = back(n).iter().map(|b| at(n as f64 - b)).sum::<f64>() * 0.6;
            out.push(if settings.wet_only { wet } else { wet + x });
        }
        out
    }

    fn worst(got: &[f32], wanted: &[f64]) -> f64 {
        let apart = got.iter().zip(wanted);
        let apart = apart.map(|(got, want)| (f64::from(*got) - want).abs());
        apart.fold(0.0, f64::max)
    }

    #[test]
    fn the_tables_are_the_doc_s_four_shapes() {
        let tables = &**TABLES;
        // Two parabolas: up to the top a quarter of the way round, back to
        // the middle at half, down to 0 at three quarters.
        let sine = &tables[0];
        assert_eq!(
            (sine[0], sine[1024], sine[2048], sine[3072]),
            (16_383, 32_766, 16_383, 0)
        );
        assert_eq!(sine[512], (1.75f64 * 16_383.0) as i32);
        // Its cube is flat around the middle.
        assert_eq!(tables[1][512], ((1.0 + 0.75f64.powi(3)) * 16_383.0) as i32);
        assert_eq!(
            (tables[1][0], tables[1][1024], tables[1][3072]),
            (16_383, 32_766, 0)
        );
        // The sum of three sines, and its cube.
        let multi = |i: usize| {
            let x = i as f64 / 2048.0;
            let [once, thrice, twice] = TURNS.map(|turn| (x * turn).sin());
            let sum = once + (thrice + twice) / 3.0;
            sum * 3.0 / (2.0 * 2f64.sqrt() + 1.0)
        };
        for i in [0, 100, 700, 1024, 2000, 3333, 4095] {
            let wanted = (multi(i) + 1.0) * 16_383.0;
            assert!((f64::from(tables[2][i]) - wanted).abs() < 1.5, "{i}");
            let wanted = (multi(i).powi(3) + 1.0) * 16_383.0;
            assert!((f64::from(tables[3][i]) - wanted).abs() < 1.5, "{i}");
        }
        for table in tables {
            assert!(table.iter().all(|entry| (0..=32_766).contains(entry)));
        }
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_numbers() {
        let plugin = Chorus::default();
        let set = plugin.settings;
        // 15.025 ms and 2.25 ms at 44.1 kHz, in 4096ths of a sample.
        assert_eq!(set.delay, (15.025 * 44.1 * 4096.0) as i32);
        assert_eq!(set.depth, (2.25 * 44.1 * 64.0) as i32);
        // The top of a table pushes a tap back by all of the depth.
        let most = (32_766 * set.depth) as u32 >> 9;
        assert!((f64::from(most) / 4096.0 - 2.25 * 44.1).abs() < 0.02);
        // 60 degrees are 682 steps and a third, less the one FL takes off.
        assert_eq!(set.stereo, 681);
        // 0.45, 1.25 and 2.45 Hz.
        for (step, hz) in set.steps.iter().zip([0.45, 1.25, 2.45]) {
            let turns = f64::from(*step) * 44_100.0 / 16_777_216.0;
            assert!((turns - hz).abs() < 3e-3, "{turns}");
        }
        assert_eq!((set.waves, set.low, set.wet_only), ([0; 3], false, false));
        // The crossover at note 63.5, which is 320 Hz.
        let corner = 440.0 * 2f64.powf((63.5 - 69.0) / 12.0);
        assert!((corner - 320.2).abs() < 0.1);
        let w = corner / 44_100.0 * 6.283_184;
        assert!((f64::from(set.k) - (1.0 - w.sin()) / w.cos()).abs() < 1e-6);
        assert!((set.k2 - (1.0 - set.k) / 2.0).abs() < 1e-7);

        // The ends of the wheels.
        let ends = with([0, 5000, 0, 5000, 0, 5000, 1024, 682, 1023, 26, 5000, 257]).settings;
        assert_eq!(ends.delay, (0.05 * 44.1 * 4096.0) as i32);
        assert_eq!((ends.stereo, ends.steps[1]), (-1, 0));
        assert!((f64::from(ends.steps[0]) * 44_100.0 / 16_777_216.0 - 5.0).abs() < 1e-3);
        // Only a wheel at its end reads the fourth table.
        assert_eq!(ends.waves, [3, 1, 2]);
        assert!(ends.low && ends.wet_only);
        let under = with([1024, 0, 1024, 0, 0, 0, 0, 0, 0, 25, 0, 256]).settings;
        assert_eq!(under.delay, (30.0 * 44.1 * 4096.0) as i32);
        assert_eq!((under.depth, under.stereo), (0, 2047));
        assert!(!under.low && !under.wet_only);
        // The cutoff stops at the rate over 4.1.
        let stopped = 44_100.0 / 4.1 / 44_100.0 * 6.283_184f64;
        let wanted = (1.0 - stopped.sin()) / stopped.cos();
        assert!((f64::from(ends.k) - wanted).abs() < 1e-6);
    }

    #[test]
    fn with_the_depth_at_0_the_band_comes_back_late_at_1_8_on_the_input_at_0_7() {
        // No depth, so the three taps are one: the band, late by the
        // delay. Above the cutoff, under it, and alone.
        for (params, len) in [
            ([100, 0, 341, 1500, 2500, 3500, 0, 0, 0, 0, 2500, 0], 3000),
            ([512, 0, 0, 0, 0, 0, 500, 900, 1024, 100, 3000, 0], 3000),
            ([7, 0, 1024, 5000, 5000, 5000, 0, 0, 0, 0, 4000, 1024], 500),
        ] {
            let mut plugin = with(params);
            let settings = plugin.settings;
            let late = f64::from(settings.delay) / 4096.0;
            let tone = sine(0.0173, len);
            let wanted = model(&tone, &settings, |_| [late; 3]);
            let (mut l, mut r) = (tone.clone(), tone.clone());
            for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                plugin.process(l, r);
            }
            assert!(worst(&l, &wanted) < 1e-5, "{params:?}");
            assert!(l == r, "{params:?}");
            assert_eq!(plugin.latency(), 0);
        }
    }

    #[test]
    fn a_level_that_stays_is_all_under_the_cutoff() {
        let run = |cross: i32, wet_only: i32| {
            let mut plugin = with([
                512, 2250, 341, 1500, 2500, 3500, 0, 0, 0, cross, 2500, wet_only,
            ]);
            let (mut l, mut r) = (vec![1.0; 8000], vec![-1.0; 8000]);
            plugin.process(&mut l, &mut r);
            (l[7999], r[7999])
        };
        // Above the cutoff there is none of it: the input at 0.7.
        let (left, right) = run(0, 0);
        assert!((left - 0.7).abs() < 1e-5 && (right + 0.7).abs() < 1e-5);
        // Under it there is all of it: three taps at 0.6 of 0.7, and the
        // input. That is the most the plugin makes of full scale.
        let (left, right) = run(100, 0);
        assert!((left - 1.96).abs() < 1e-5 && (right + 1.96).abs() < 1e-5);
        let (left, _) = run(100, 1024);
        assert!((left - 1.26).abs() < 1e-5);
        assert!(run(0, 1024).0.abs() < 1e-5);
    }

    #[test]
    fn a_tap_follows_its_lfo_from_the_delay_to_the_delay_and_the_depth() {
        // One LFO at 5 Hz and two that stand still, on the band under a
        // cutoff well above the tone.
        let params = [200, 3000, 0, 5000, 0, 0, 0, 0, 0, 100, 5000, 1024];
        let mut plugin = with(params);
        let settings = plugin.settings;
        let phases = plugin.phases;
        // Where each tap is at frame n, from the tables.
        let back = |n: usize| -> [f64; 3] {
            [0, 1, 2].map(|lfo| {
                let step = settings.steps[lfo] as u32;
                let phase = phases[lfo].wrapping_add(step.wrapping_mul(n as u32));
                let [left, _] = settings.sweep(lfo, phase);
                f64::from(settings.delay as u32 + left) / 4096.0
            })
        };
        // The moving tap covers all of the depth, and the two others stay
        // in the middle of it.
        let cycle = 16_777_216 / settings.steps[0] as usize;
        let reach = (0..cycle).map(|n| back(n)[0]);
        let (least, most) = reach.fold((f64::MAX, 0f64), |(a, b), d| (a.min(d), b.max(d)));
        let (delay, depth) = (f64::from(settings.delay) / 4096.0, 3.0 * 44.1);
        assert!((delay - 5.8996 * 44.1).abs() < 0.01, "{delay}");
        assert!(least == delay && (most - delay - depth).abs() < 0.05);
        assert!((back(0)[1] - delay - depth / 2.0).abs() < 0.5);
        assert_eq!(back(0)[1], back(5000)[1]);

        let tone = sine(0.011, 12_000);
        let wanted = model(&tone, &settings, back);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        for (l, r) in l.chunks_mut(500).zip(r.chunks_mut(500)) {
            plugin.process(l, r);
        }
        assert!(worst(&l, &wanted) < 1e-5);
        // With the stereo wheel at 0 the right side is one step of the
        // table behind the left: a tenth of a sample at the most.
        let apart = l.iter().zip(&r).map(|(l, r)| (l - r).abs());
        let apart = apart.fold(0.0, f32::max);
        assert!(apart > 0.0 && apart < 0.05, "{apart}");
    }

    #[test]
    fn the_stereo_wheel_puts_the_right_side_s_lfos_ahead() {
        let half = with([200, 3000, 1024, 5000, 0, 0, 0, 0, 0, 100, 5000, 1024]);
        let settings = half.settings;
        assert_eq!(settings.stereo, 2047);
        // Half a cycle on, less a step: the left at its longest has the
        // right at its shortest, and the two add up to the depth.
        let top = 1024 << 12;
        let [left, right] = settings.sweep(0, top);
        let depth = (3.0 * 44.1 * 4096.0) as u32;
        assert!(left.abs_diff(depth) < 100 && right < 200, "{left} {right}");
        for phase in [0u32, 5 << 20, 9 << 20, 0xfff << 12] {
            let [left, right] = settings.sweep(0, phase);
            assert!((left + right).abs_diff(depth) < 1500, "{phase}");
        }
        // A phase past 2^24 comes round.
        assert_eq!(settings.sweep(0, top), settings.sweep(0, top + (1 << 24)));
    }

    #[test]
    fn the_lfos_start_where_delphi_s_random_numbers_put_them() {
        let mut plugin = Chorus::default();
        assert_eq!(plugin.phases, [0, 1028, 28_213]);
        plugin.process(&mut [0.5; 300], &mut [0.5; 300]);
        assert_ne!(plugin.phases, [0, 1028, 28_213]);
        assert!(plugin.sides[0].ring.iter().any(|v| *v != 0.0));
        // A flush empties the lines and takes the next three.
        plugin.reset();
        assert_eq!(plugin.phases, [6637, 8942, 22_008]);
        assert_eq!(plugin.write, 0);
        for side in &plugin.sides {
            assert!(side.ring.iter().all(|v| *v == 0.0));
            assert_eq!((side.low, side.last), (0.0, 0.0));
        }
        // All of them are in the first 8 steps of the 4096.
        assert!(plugin.phases.iter().all(|phase| phase >> 12 < 8));
    }

    #[test]
    fn the_rate_moves_the_delays_and_not_the_times() {
        let mut plugin = Chorus::default();
        let slow = plugin.settings;
        plugin.set_sample_rate(88_200.0);
        let fast = plugin.settings;
        assert!((fast.delay - 2 * slow.delay).abs() <= 1);
        assert!((fast.depth - 2 * slow.depth).abs() <= 1);
        assert_eq!(fast.stereo, slow.stereo);
        for (fast, slow) in fast.steps.iter().zip(slow.steps) {
            assert!((2 * fast - slow).abs() <= 2);
        }
        assert!(fast.k > slow.k);
        // A rate that is not one leaves the plugin as it is.
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin, before);

        // At 192 kHz the longest delay is longer than the line, and the
        // plugin still gives numbers.
        let mut plugin = with([
            1024, 5000, 1024, 5000, 5000, 5000, 1024, 1024, 1024, 0, 0, 0,
        ]);
        plugin.set_sample_rate(192_000.0);
        assert!(plugin.settings.delay >> 12 > 4096);
        let (mut l, mut r) = (sine(0.01, 9000), sine(0.013, 9000));
        plugin.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 2.0));
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory_and_is_the_same_plugin_as_its_state() {
        let mut plugin = Chorus::default();
        let memory = |plugin: &Chorus| [0, 1].map(|n| plugin.sides[n].ring.as_ptr());
        let before = memory(&plugin);
        let moves = [
            1024.4,
            -3.0,
            2000.0,
            5000.0,
            17.0,
            0.0,
            512.0,
            1024.0,
            f32::NAN,
            100.0,
            4999.6,
            300.0,
        ];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
            plugin.process(&mut [0.25; 200], &mut [0.25; 200]);
        }
        plugin.set_param(12, 1.0);
        assert_eq!((plugin.param(12), plugin.param_count()), (0.0, 12));
        let params: Vec<f32> = (0..12).map(|i| plugin.param(i)).collect();
        assert_eq!(
            params,
            [
                1024.0, 0.0, 1024.0, 5000.0, 17.0, 0.0, 512.0, 1024.0, 0.0, 100.0, 5000.0, 300.0
            ]
        );
        assert_eq!(memory(&plugin), before);
        let same = with([1024, 0, 1024, 5000, 17, 0, 512, 1024, 0, 100, 5000, 300]);
        assert_eq!(plugin.settings, same.settings);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([40, 4375, 149, 3229, 0, 781, 587, 0, 1024, 65, 4531, 491]);
        let before = plugin.clone();
        for len in [3, 48, 56] {
            let mut wrong = bytes(DEFAULTS);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 52,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 1025), (1, -1), (9, 101), (11, 1025)] {
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
        // The twelve floats of an older build, which FL reads as a version
        // far too new.
        let floats: Vec<u8> = [0.5f32; 12].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(
            plugin.set_state(&floats),
            Err(StateError::Version { found: 0x3f00_0000 })
        );
        assert_eq!(plugin, before);
        // FL reads any version under 2.
        let mut older = bytes(DEFAULTS);
        older[0] = 0;
        assert_eq!(plugin.set_state(&older), Ok(()));
        assert_eq!(plugin.params, DEFAULTS);
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(own) = presets(NAME) else { return };
        // Eight of the plugin's own, the three of the plugin database, and
        // the mixer preset with the floats.
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert_eq!((own.len(), all.len()), (8, 12));
        let named = |file: &str| {
            let preset = own.iter().find(|p| p.file == file);
            let preset = preset.unwrap_or_else(|| panic!("no preset {file}"));
            let mut plugin = Chorus::default();
            plugin.set_state(&preset.state).unwrap();
            plugin
        };
        for preset in &all {
            let mut plugin = Chorus::default();
            let loaded = plugin.set_state(&preset.state);
            if preset.file == "Chorus send" {
                assert_eq!(preset.state.len(), 48);
                assert_eq!(loaded, Err(StateError::Version { found: 0x3f00_0000 }));
            } else {
                assert_eq!(
                    (preset.state.len(), loaded),
                    (52, Ok(())),
                    "{}",
                    preset.file
                );
            }
        }
        // The default preset has the first wave at 0 where the wheel
        // starts at 23. Both are the first table.
        let default = named("Default");
        assert_eq!(default.params[6], 0);
        assert_eq!(default.settings, Chorus::default().settings);
        let detune = named("50 Detune").settings;
        assert_eq!(detune.waves, [1, 0, 3]);
        assert!(detune.low && detune.wet_only && detune.steps[1] == 0);
        let enhance = named("Low Enhance").settings;
        assert!(enhance.low && !enhance.wet_only);
        assert_eq!(named("Light HF").settings.waves, [0, 0, 2]);
        assert_eq!(named("CE Chorus").settings.stereo, -1);
    }
}
