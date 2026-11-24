//! Fruity Vocoder: a band vocoder with the modulator on one side of the
//! input and the carrier on the other, as `docs/plugins/fruity-vocoder.md`
//! reads it from `Fruity Vocoder_x64.dll`.
//!
//! The left side is the modulator and the right the carrier, or the other
//! way round with the swap switch. Each of 4 to 128 bands splits both
//! through the same band passes (each `order` times), follows the
//! modulator band's level and puts the carrier band out at that level. The
//! sum is mono, on both sides, with the modulator and the carrier under it
//! at their own levels.
//!
//! The state: an i32 version (2), the band count, the filter order, the
//! listen mode (0 modulator, 1 carrier, 2 the mix) and a byte for the
//! swap; the level of each band as f32; then the twelve wheels as i32:
//!
//! | wheel | range | default |
//! |---|---|---|
//! | 0, 1 Starting and ending frequency | 0 to 65536 | 0, 65536 |
//! | 2 Frequency scale | 0 to 128 | 64 |
//! | 3 Invert | 0, 1 | 0 |
//! | 4 Formant, cents | -1200 to 1200 | 0 |
//! | 5 Bandwidth | 5 to 200 | 50 |
//! | 6, 7 Attack and release, ms | 10 to 1000 | 20, 100 |
//! | 8 Hold | 0, 1 | 0 |
//! | 9, 10, 11 Modulator, carrier and wet level | 0 to 160 | 0, 0, 128 |

use crate::{Effect, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Vocoder";

/// The wheels, the most bands and the most stages a band has.
const PARAMS: usize = 12;
const MAX_BANDS: usize = 128;
const MAX_ORDER: usize = 8;
/// The frames worked on at a time.
const BLOCK: usize = 1024;

const RANGES: [(i32, i32); PARAMS] = [
    (0, 65536),
    (0, 65536),
    (0, 128),
    (0, 1),
    (-1200, 1200),
    (5, 200),
    (10, 1000),
    (10, 1000),
    (0, 1),
    (0, 160),
    (0, 160),
    (0, 160),
];
const DEFAULTS: [i32; PARAMS] = [0, 65536, 64, 0, 0, 50, 20, 100, 0, 0, 0, 128];

/// FL's knob gain on `v / 128`, 1 at 128 exactly.
fn knob(v: i32) -> f32 {
    let x = v as f32 * 0.007_812_5;
    if x == 1.0 {
        1.0
    } else {
        (((f64::from(x) * f64::from(11f64.ln() as f32)).exp() - 1.0) * f64::from(0.1f32)) as f32
    }
}

/// A frequency in cents of FL's notes (5700 is 440 Hz), and back
/// (`FUN_006cff40`, `FUN_006cfef0`).
fn cents(hz: f32) -> f32 {
    ((f64::from(hz) / 440.0).ln() / (std::f64::consts::LN_2 / 1200.0) + 5700.0) as f32
}

fn hertz(cents: f32) -> f32 {
    ((f64::from(cents) - 5700.0) * (std::f64::consts::LN_2 / 1200.0)).exp() as f32 * 440.0
}

/// The band pass both sides of a band go through, `order` times (RBJ,
/// constant 0 dB peak, `FUN_005db530` type 2), run in doubles
/// (`FUN_005dbc10`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct BandPass {
    b0: f64,
    a: [f64; 2],
}

impl BandPass {
    /// At `hz`, `octaves` wide, for a rate (`FUN_005dbb00`).
    fn new(hz: f32, octaves: f32, rate: f64) -> Self {
        let w = f64::from(hz) * (std::f64::consts::TAU / rate);
        let (sin, cos) = w.sin_cos();
        let x = std::f64::consts::LN_2 / 2.0 * f64::from(octaves) * w / sin;
        let alpha = sin * x.sinh();
        let k = 1.0 / (alpha + 1.0);
        BandPass {
            b0: k * alpha,
            a: [k * cos * -2.0, k * (1.0 - alpha)],
        }
    }

    /// One stage over a block: x1, x2, y1, y2 kept.
    fn run(&self, state: &mut [f64; 4], input: &[f32], out: &mut [f32]) {
        let [mut x1, mut x2, mut y1, mut y2] = *state;
        for (x, y) in input.iter().zip(out.iter_mut()) {
            let x = f64::from(*x);
            let v = x * self.b0 + ((x1 * 0.0 - y2 * self.a[1]) + x2 * -self.b0) - y1 * self.a[0];
            (x2, x1, y2, y1) = (x1, x, y1, v);
            *y = v as f32;
        }
        *state = [x1, x2, y1, y2];
    }
}

/// One band: the modulator's and the carrier's filters and states, its
/// level and its envelope.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Band {
    on: bool,
    modulator: BandPass,
    carrier: BandPass,
    mod_state: [[f64; 4]; MAX_ORDER],
    car_state: [[f64; 4]; MAX_ORDER],
    /// The level the state gives, and that times the wet gain.
    level: f32,
    gain: f32,
    envelope: f64,
}

/// Fruity Vocoder. A new one has FL's defaults: 16 bands of order 2,
/// listening to the mix.
#[derive(Debug, Clone)]
pub struct FruityVocoder {
    params: [i32; PARAMS],
    bands: usize,
    order: usize,
    listen: i32,
    swap: bool,
    levels: Vec<f32>,
    rate: f64,
    band: Vec<Band>,
    /// The envelope's attack and release coefficients.
    attack: f32,
    release: f32,
    /// The modulator's and the carrier's levels under the mix.
    mod_level: f32,
    car_level: f32,
    scratch: [Vec<f32>; 4],
}

impl Default for FruityVocoder {
    fn default() -> Self {
        let mut plugin = FruityVocoder {
            params: DEFAULTS,
            bands: 16,
            order: 2,
            listen: 2,
            swap: false,
            levels: vec![1.0; MAX_BANDS],
            rate: 44_100.0,
            band: vec![Band::default(); MAX_BANDS],
            attack: 0.0,
            release: 0.0,
            mod_level: 0.0,
            car_level: 0.0,
            scratch: std::array::from_fn(|_| vec![0.0; BLOCK]),
        };
        plugin.setup();
        plugin
    }
}

impl FruityVocoder {
    /// The bands and the gains (`FUN_006d03a0`, `FUN_006cff90`,
    /// `FUN_006d00b0`). This allocates nothing.
    fn setup(&mut self) {
        let p = |i: usize| self.params[i];
        let wet = match self.listen {
            0 => {
                (self.mod_level, self.car_level) = (1.0, 0.0);
                0.0
            }
            1 => {
                (self.mod_level, self.car_level) = (0.0, 1.0);
                0.0
            }
            _ => {
                (self.mod_level, self.car_level) = (knob(p(9)), knob(p(10)));
                knob(p(11))
            }
        };
        let level = wet * 1500.0 / p(5) as f32;
        let span = (p(5) as f32 * self.order as f32) / 120_000.0;
        let scale = (f64::from((p(2) as f32) * 0.007_812_5) * 10_000f64.ln()).exp() - 1.0;
        let scale = (scale.round_ties_even() + 1.0) as f32;
        let most = (self.rate * 20_000.0 / 44_100.0) as f32;
        let ms = (self.rate * 0.001) as f32;
        let hundredth = 0.01f64.ln() as f32;
        let coef = |t: i32| (f64::from(hundredth / (t as f32 * ms))).exp() as f32;
        (self.attack, self.release) = (coef(p(6)), coef(p(7)));

        let edge = |i: usize| {
            let v = f64::from((f64::from(p(i)) / 65_536.0) as f32);
            ((v * 101f64.ln()).exp() - 1.0) as f32 * 159.9 + 10.0
        };
        let (mut low, high) = (edge(0), edge(1));
        if low > high {
            low = high - 1.0;
        }
        let n = self.bands;
        let mut car_on = [false; MAX_BANDS];
        // The carrier's bands first, then the modulator's, which plays only
        // where the carrier has a band.
        for carrier in [true, false] {
            let mut prev = 0.0f32;
            for i in 0..=n {
                let x = (f64::from(i as f32 / n as f32) * f64::from(scale + 1.0).ln()).exp() - 1.0;
                let at = low + (x as f32 / scale) * (high - low);
                let mut note = cents(at);
                let mut index = i as i64 - 1;
                if carrier {
                    note += p(4) as f32;
                    if p(3) > 0 {
                        index = n as i64 - index - 1;
                    }
                }
                if i > 0 {
                    let index = index as usize;
                    let hz = hertz((prev + note) * 0.5);
                    let mut on = hz >= 10.0 && hz < most;
                    if !carrier {
                        on &= car_on[index];
                    }
                    let band = &mut self.band[index];
                    if carrier {
                        car_on[index] = on;
                    }
                    if on {
                        let filter = BandPass::new(hz, span * (note - prev), self.rate);
                        if carrier {
                            band.carrier = filter;
                        } else {
                            band.modulator = filter;
                            band.on = true;
                            band.level = self.levels[index];
                            band.gain = band.level * level;
                        }
                    } else {
                        band.on = false;
                        band.gain = 0.0;
                        band.envelope = 0.0;
                        band.mod_state = [[0.0; 4]; MAX_ORDER];
                        band.car_state = [[0.0; 4]; MAX_ORDER];
                    }
                }
                prev = note;
            }
        }
    }

    fn chunk(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let (modulator, carrier) = if self.swap {
            (&*right, &*left)
        } else {
            (&*left, &*right)
        };
        let [acc, filtered, scratch, carried] = &mut self.scratch;
        acc[..n].fill(0.0);
        let hold = self.params[8] != 0;
        for band in &mut self.band[..self.bands] {
            if !band.on {
                continue;
            }
            if !hold {
                let mut input = modulator;
                for state in &mut band.mod_state[..self.order] {
                    band.modulator.run(state, input, &mut filtered[..n]);
                    scratch[..n].copy_from_slice(&filtered[..n]);
                    input = &scratch[..n];
                }
                // The follower (`FUN_006d0e70`), in x87's precision.
                let (attack, release) = (f64::from(self.attack), f64::from(self.release));
                let gain = band.gain;
                let mut env = band.envelope;
                for i in 0..n {
                    let x = f64::from(filtered[i].abs() * gain);
                    let c = if x <= env { release } else { attack };
                    env = (env - x) * c + x;
                    filtered[i] = env as f32;
                    // FL tests the envelope with the next input as one
                    // 64-bit word: it is let go only before a true 0.
                    let next_silent = i + 1 < n && filtered[i + 1].to_bits() == 0;
                    if (env as f32).to_bits() < 0x3380_0000 && next_silent {
                        env = 0.0;
                        filtered[i] = 0.0;
                    }
                }
                band.envelope = f64::from(env as f32);
            }
            let mut input = carrier;
            for state in &mut band.car_state[..self.order] {
                band.carrier.run(state, input, &mut carried[..n]);
                scratch[..n].copy_from_slice(&carried[..n]);
                input = &scratch[..n];
            }
            if hold {
                let env = band.envelope as f32;
                for (a, c) in acc[..n].iter_mut().zip(&carried[..n]) {
                    *a += c * env;
                }
            } else {
                for ((a, c), e) in acc[..n].iter_mut().zip(&carried[..n]).zip(&filtered[..n]) {
                    *a += c * e;
                }
            }
        }
        for (a, (m, c)) in acc[..n].iter_mut().zip(modulator.iter().zip(carrier)) {
            *a += m * self.mod_level;
            *a += c * self.car_level;
        }
        left.copy_from_slice(&acc[..n]);
        right.copy_from_slice(&acc[..n]);
    }
}

impl Effect for FruityVocoder {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let word = |at: usize| {
            bytes
                .get(at..at + 4)
                .map(|w| i32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        };
        let version = word(0).ok_or(StateError::Length {
            expected: 129,
            found: bytes.len(),
        })?;
        if version > 2 {
            return Err(StateError::Version { found: version });
        }
        if version < 2 {
            // FL reads nothing more of an older state.
            return Ok(());
        }
        let bands = word(4).unwrap_or(0);
        if !(1..=MAX_BANDS as i32).contains(&bands) {
            return Err(StateError::Range {
                param: 0,
                value: bands,
            });
        }
        let bands = bands as usize;
        let expected = 17 + 4 * bands + 4 * PARAMS;
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let order = word(8).unwrap_or(1).clamp(1, MAX_ORDER as i32) as usize;
        let listen = word(12).unwrap_or(2);
        let swap = bytes[16] != 0;
        let mut levels = [1.0f32; MAX_BANDS];
        for (i, level) in levels[..bands].iter_mut().enumerate() {
            let at = 17 + 4 * i;
            *level = f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        }
        let mut params = [0; PARAMS];
        for (i, param) in params.iter_mut().enumerate() {
            *param = word(17 + 4 * bands + 4 * i).unwrap_or(0);
        }
        if params[5] == 0 || params[6] == 0 || params[7] == 0 {
            return Err(StateError::Range { param: 5, value: 0 });
        }
        self.params = params;
        self.bands = bands;
        self.order = order;
        self.listen = listen;
        self.swap = swap;
        self.levels.copy_from_slice(&levels);
        self.band.fill(Band::default());
        self.setup();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = f64::from(hz);
            self.setup();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let chunks = left[..len]
            .chunks_mut(BLOCK)
            .zip(right[..len].chunks_mut(BLOCK));
        for (l, r) in chunks {
            self.chunk(l, r);
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = crate::knob_value(value, *range);
            self.setup();
        }
    }

    fn reset(&mut self) {
        for band in &mut self.band {
            band.mod_state = [[0.0; 4]; MAX_ORDER];
            band.car_state = [[0.0; 4]; MAX_ORDER];
            band.envelope = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, presets, projects, sine};

    #[test]
    fn cents_are_fl_s_notes() {
        assert!((cents(440.0) - 5700.0).abs() < 1e-3);
        assert!((hertz(6900.0) - 880.0).abs() < 1e-2);
    }

    #[test]
    fn the_band_pass_peaks_at_its_centre_at_0_db() {
        let f = BandPass::new(1000.0, 1.0, 44_100.0);
        let level = |hz: f64| {
            let input = sine(hz / 44_100.0, 44_100);
            let mut out = vec![0.0; input.len()];
            f.run(&mut [0.0; 4], &input, &mut out);
            out[22_050..].iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        assert!((level(1000.0) - 1.0).abs() < 0.01);
        assert!(level(4000.0) < 0.3);
        assert!(level(250.0) < 0.3);
    }

    #[test]
    fn the_default_bands_span_10_hz_to_16_khz() {
        let p = FruityVocoder::default();
        assert_eq!(p.band[..16].iter().filter(|b| b.on).count(), 16);
    }

    #[test]
    fn a_silent_modulator_silences_the_wet_sound() {
        let mut p = FruityVocoder::default();
        let carrier = sine(220.0 / 44_100.0, 4096);
        let (mut l, mut r) = (vec![0.0; 4096], carrier);
        p.process(&mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.0));
        assert_eq!(l, r);
    }

    #[test]
    fn a_tone_in_both_comes_out() {
        let mut p = FruityVocoder::default();
        // The level goes with the square of the input: a band's gain is
        // 1500 / BW, 30 here, on the follower.
        let tone: Vec<f32> = sine(1000.0 / 44_100.0, 44_100)
            .iter()
            .map(|x| x * 0.25)
            .collect();
        let (mut l, mut r) = (tone.clone(), tone);
        p.process(&mut l, &mut r);
        let peak = l[22_050..].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.05 && peak < 4.0, "{peak}");
    }

    #[test]
    fn fl_s_states_load_and_play() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, Fruity Vocoder's states skipped");
            return;
        };
        let mut saved = presets(NAME).unwrap();
        let in_projects: Vec<_> = projects(&root)
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        // Edlan & Ella Noel and the render test.
        assert_eq!(in_projects.len(), 2);
        saved.extend(in_projects);
        let input: Vec<f32> = sine(220.0 / 44_100.0, 8192)
            .iter()
            .map(|x| x * 0.25)
            .collect();
        for s in saved {
            let mut p = FruityVocoder::default();
            p.set_state(&s.state)
                .unwrap_or_else(|e| panic!("{}: {e}", s.file));
            let (mut l, mut r) = (input.clone(), input.clone());
            p.process(&mut l, &mut r);
            let peak = l.iter().fold(0f32, |m, x| m.max(x.abs()));
            assert!(l.iter().all(|x| x.is_finite()), "{}", s.file);
            assert!(peak < 8.0, "{}: {peak}", s.file);
        }
    }
}
