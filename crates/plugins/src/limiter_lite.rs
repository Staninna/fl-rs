//! `LimiterLite` of FL's `Shared/ildsp_x64.dll`: the limiter Emphasizer
//! is a shell over, as `docs/plugins/emphasizer.md` section 4 maps it.
//!
//! One gain for both sides, worked out from the louder of them: the input
//! gain glides at 20 Hz, the level (`1 - e^2 / 2` of the peak, `e` the
//! emphasis) runs through eight 3.125 ms peak holds and a one-pole whose
//! attack is `0.002 + 4 e^2` s and whose release is `0.1 + 3 (1 - e^2)` s,
//! a quarter of that when the gain is deep under where it last was; the
//! gain brings that level under -0.1 dB. The sound waits 2 ms for it, is
//! clipped hard at 1 or softly from 0.84, takes the output gain (20 Hz)
//! and, with level matching, the inverse of the input gain, then
//! crossfades (20 Hz) with the dry sound when bypassed and is scaled by
//! 0.9999. The LUFS and RMS meters are display only and left out.
//!
//! Held to the DLL's code under the emulator
//! (`tools/plugstate/limiter_lite_emu.py`).

use crate::low_lifter::msvc;

/// The delays' ring (`2^16` floats, a u16 index) and their length, 2 ms.
const RING: usize = 1 << 16;
const LOOKAHEAD: f32 = 0.002;
/// The ceiling the gain holds the level under, -0.1 dB (`0x1803e0674`).
const CEILING: f32 = 0.988_553_1;
/// The soft clip's knee, span and slope (`0x240350`).
const KNEE: f32 = 0.841_395_14;
const SPAN: f32 = 0.158_604_86;
const SLOPE: f32 = 6.304_977;
/// `-2 pi`, and `-40 pi` (20 Hz) for the glides.
const TURN: f64 = -std::f64::consts::TAU;
const GLIDE: f64 = -125.663_706_143_591_72;
/// `ln 10 / 20`, dB to the exponent of a gain (`0x1803e0454`).
const DB: f32 = f32::from_bits(0x3deb_c8e3);
/// The most frames worked out at once: the block the shell declares to
/// `init` (0x800).
const CHUNK: usize = 0x800;

/// A delay of whole frames in a ring of `2^16` (`0x180163db0`): the
/// input goes in, then what is `len` frames old comes out.
#[derive(Debug, Clone)]
struct Delay {
    ring: Vec<f32>,
    at: u16,
    len: u16,
}

impl Delay {
    fn new() -> Self {
        Delay {
            ring: vec![0.0; RING],
            at: 0,
            len: 0,
        }
    }

    fn prepare(&mut self, rate: f32) {
        let len = (rate * LOOKAHEAD).max(0.0);
        self.len = len as u64 as u16;
        self.ring.fill(0.0);
        self.at = 0;
    }

    fn run(&mut self, block: &mut [f32]) {
        for sample in block {
            self.ring[usize::from(self.at)] = *sample;
            *sample = self.ring[usize::from(self.at.wrapping_sub(self.len))];
            self.at = self.at.wrapping_add(1);
        }
    }
}

/// `LimiterLite`, with what the shell sets.
#[derive(Debug, Clone)]
pub(crate) struct LimiterLite {
    rate: f32,
    inv_rate: f32,
    /// The glides' pole (`0x70`).
    pole: f32,
    input_gain: f32,
    input_now: f32,
    output_gain: f32,
    output_now: f32,
    /// The emphasis's share of the level (`0x6c`) and the two release
    /// coefficients (`0x64`, `0x68`).
    share: f32,
    releases: [f32; 2],
    level_matching: bool,
    soft: bool,
    bypass: bool,
    bypass_now: f32,
    /// The LUFS meter's switch, kept for the shell.
    switch: f32,
    /// The gain of the frame before (`0x140074`) and the gain where it
    /// last went down (`0x140078`).
    last: f32,
    deepest: f32,
    /// The eight peak holds (`PeakHold8Lite` at `0x240230`).
    holds: [f32; 8],
    counts: [u64; 8],
    hold: f32,
    /// The level's one-pole (`ExpSmoother1Lite` at `0x2402e0`): its state
    /// and its release and attack coefficients.
    level: f32,
    coefficients: [f32; 2],
    /// The one-pole's attack and release times as last set (`0x240338`,
    /// `0x240340`), which `prepare` works the coefficients out from.
    times: [f32; 2],
    dry: [Delay; 2],
    wet: [Delay; 2],
    scratch: [Vec<f32>; 5],
}

impl LimiterLite {
    /// As `LimiterLite_CreateInstance` makes it and `init(rate)` sets it
    /// up. This allocates.
    pub(crate) fn new(rate: f32) -> Self {
        let mut limiter = LimiterLite {
            rate: 0.0,
            inv_rate: 0.0,
            pole: 0.0,
            input_gain: 1.0,
            input_now: 1.0,
            output_gain: 0.0,
            output_now: 0.0,
            share: 1.0,
            releases: [0.0; 2],
            level_matching: false,
            soft: false,
            bypass: false,
            bypass_now: 0.0,
            switch: 0.0,
            last: 0.0,
            deepest: 0.0,
            holds: [0.0; 8],
            counts: [0; 8],
            hold: 0.0,
            level: 0.0,
            coefficients: [0.0; 2],
            times: [0.01, 0.1],
            dry: [Delay::new(), Delay::new()],
            wet: [Delay::new(), Delay::new()],
            scratch: std::array::from_fn(|_| vec![0.0; CHUNK]),
        };
        limiter.prepare(rate);
        limiter
    }

    /// `prepare` (`0x180173610`): every parameter but the output gain, the
    /// emphasis's release coefficients and the switch back to 0, every
    /// state cleared, the coefficients at `rate`. The level's one-pole
    /// takes the times the emphasis last set, or its constructor's 0.01 s
    /// up and 0.1 s down.
    pub(crate) fn prepare(&mut self, rate: f32) {
        self.input_gain = 1.0;
        self.input_now = 1.0;
        self.share = 1.0;
        self.last = 0.0;
        self.deepest = 0.0;
        self.level_matching = false;
        self.soft = false;
        self.bypass = false;
        self.bypass_now = 0.0;
        self.rate = rate;
        self.inv_rate = 1.0 / rate;
        self.pole = (f64::from(self.inv_rate) * GLIDE).exp() as f32;
        for delay in self.dry.iter_mut().chain(&mut self.wet) {
            delay.prepare(rate);
        }
        let turn = f64::from(self.inv_rate) * TURN;
        self.coefficients = [
            (turn / f64::from(self.times[1])).exp() as f32,
            (turn / f64::from(self.times[0])).exp() as f32,
        ];
        self.hold = 0.025 * 0.125 * rate;
        self.holds = [0.0; 8];
        self.counts = [0; 8];
        self.level = 0.0;
    }

    /// `setParam` (`0x1801aabb0`): 0 input gain (dB), 1 emphasis, 2
    /// clipping (0 hard), 3 level matching, 4 bypass, 5 output gain (dB),
    /// 6 the meter's switch.
    pub(crate) fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.input_gain = msvc::exp(value * DB),
            1 => {
                let q = value * value;
                let release = (f64::from((1.0 - q) * 3.0) + 0.1) as f32;
                let attack = q * 4.0 + 0.002;
                self.share = 1.0 - q * 0.5;
                self.times = [attack, release];
                let turn = f64::from(self.inv_rate) * TURN;
                self.coefficients = [
                    (turn / f64::from(release)).exp() as f32,
                    (turn / f64::from(attack)).exp() as f32,
                ];
                self.hold = self.rate * 0.003_125;
                self.releases = [
                    msvc::exp((turn / f64::from(release)) as f32),
                    msvc::exp((turn / f64::from(release * 0.25)) as f32),
                ];
            }
            2 => self.soft = value != 0.0,
            3 => self.level_matching = value != 0.0,
            4 => self.bypass = value != 0.0,
            5 => self.output_gain = msvc::exp(value * DB),
            6 => self.switch = value,
            _ => {}
        }
    }

    /// The frames the output is late (`0x1801aaf40`): `roundf(rate *
    /// 0.002)`.
    pub(crate) fn latency(&self) -> usize {
        (self.rate * LOOKAHEAD).round().max(0.0) as usize
    }

    /// `reset` (`0x180173880`): the delays and the gain's holds and
    /// one-pole empty.
    pub(crate) fn reset(&mut self) {
        for delay in self.dry.iter_mut().chain(&mut self.wet) {
            delay.ring.fill(0.0);
            delay.at = 0;
        }
        self.holds = [0.0; 8];
        self.counts = [0; 8];
        self.level = 0.0;
    }

    /// `process` (`0x180173940`), in place.
    pub(crate) fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        for (l, r) in left[..len]
            .chunks_mut(CHUNK)
            .zip(right[..len].chunks_mut(CHUNK))
        {
            self.chunk(l, r);
        }
    }

    fn chunk(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let [dry_l, dry_r, wet_l, wet_r, gains] = &mut self.scratch;
        let (dry_l, dry_r) = (&mut dry_l[..n], &mut dry_r[..n]);
        let (wet_l, wet_r, gains) = (&mut wet_l[..n], &mut wet_r[..n], &mut gains[..n]);
        let p = self.pole;
        let inverse = 1.0 / self.share;
        dry_l.copy_from_slice(left);
        dry_r.copy_from_slice(right);
        self.dry[0].run(dry_l);
        self.dry[1].run(dry_r);
        for i in 0..n {
            self.input_now = (self.input_now - self.input_gain) * p + self.input_gain;
            wet_l[i] = self.input_now * left[i];
            wet_r[i] = self.input_now * right[i];
        }
        for i in 0..n {
            let (l, r) = (wet_l[i].abs(), wet_r[i].abs());
            gains[i] = self.share * if r <= l { l } else { r };
        }
        let (mut last, mut deepest) = (self.last, self.deepest);
        for gain in gains.iter_mut() {
            let x = {
                let x = *gain;
                let mut held = x;
                // `PeakHold8Lite` (`0x180165d20`): each hold keeps the
                // largest it saw for up to its length.
                for (h, count) in self.holds.iter_mut().zip(&mut self.counts) {
                    if *h <= held || self.hold <= *count as f32 {
                        *count = 0;
                    } else {
                        *count += 1;
                        held = *h;
                    }
                    *h = held;
                }
                held
            };
            let level = (self.level - x) * self.coefficients[usize::from(x > self.level)] + x;
            self.level = level;
            let g = CEILING / if level <= CEILING { CEILING } else { level };
            if g <= last || g.is_nan() || last.is_nan() {
                deepest = g;
            }
            let over = inverse * x * 0.5 / deepest;
            let over = if over <= 1.0 { 1.0 } else { over };
            self.coefficients[0] = self.releases[usize::from(deepest / over > g * g)];
            *gain = g;
            last = g;
        }
        self.last = last;
        self.deepest = deepest;
        self.wet[0].run(wet_l);
        self.wet[1].run(wet_r);
        for (out, wet) in [(&mut *left, &*wet_l), (&mut *right, &*wet_r)] {
            for ((o, w), g) in out.iter_mut().zip(wet).zip(gains.iter()) {
                let x = g * w;
                *o = if self.soft {
                    soft_clip(x)
                } else {
                    hard_clip(x)
                };
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.output_now = (self.output_now - self.output_gain) * p + self.output_gain;
            *l *= self.output_now;
            *r *= self.output_now;
        }
        if self.level_matching {
            let back = 1.0 / self.input_now;
            for v in left.iter_mut().chain(right.iter_mut()) {
                *v *= back;
            }
        }
        let open = 1.0 - f32::from(u8::from(self.bypass));
        for i in 0..n {
            self.bypass_now = (self.bypass_now - open) * p + open;
            let b = self.bypass_now;
            left[i] = ((left[i] - dry_l[i]) * b + dry_l[i]) * 0.9999;
            right[i] = ((right[i] - dry_r[i]) * b + dry_r[i]) * 0.9999;
        }
    }
}

/// `0x1801658e0`: held between -1 and 1.
fn hard_clip(x: f32) -> f32 {
    let v = if x < 1.0 { x } else { 1.0 };
    if v <= -1.0 { -1.0 } else { v }
}

/// `0x180165330`: past the knee, a curve that comes to 1.
fn soft_clip(x: f32) -> f32 {
    if -KNEE <= x {
        if KNEE < x {
            let y = (x - KNEE) * SLOPE;
            y / (y * y + 1.0).sqrt() * SPAN + KNEE
        } else {
            x
        }
    } else {
        let y = (x + KNEE) * SLOPE;
        y / (y * y + 1.0).sqrt() * SPAN - KNEE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emulator's test input: noise bursts every half second over a
    /// slow sine, louder than full scale, the right side a little apart.
    fn input(len: usize) -> (Vec<f32>, Vec<f32>) {
        let mut seed = 7u32;
        (0..len)
            .map(|i| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
                let env = (-((i % 22050) as f64) / 3000.0).exp();
                let a = 1.6 * env * noise + 0.6 * (i as f64 * 0.01).sin();
                (a as f32, (0.8 * a + 0.3 * (i as f64 * 0.003).sin()) as f32)
            })
            .unzip()
    }

    fn run(values: [f32; 7], len: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
        let mut limiter = LimiterLite::new(44_100.0);
        for (index, value) in values.into_iter().enumerate() {
            limiter.set_param(index, value);
        }
        let (mut l, mut r) = input(len);
        for (l, r) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            limiter.process(l, r);
        }
        (l, r)
    }

    #[test]
    fn the_limiter_is_fl_s_to_the_bit() {
        // `limiter_lite_emu.py` at 44.1 kHz, 512 frames a call, on three
        // seconds of the input: the hash of what it put out, both sides
        // in turn. The default preset; soft clipping with level matching;
        // 24 dB into hard clipping; and bypassed.
        for (values, hash) in [
            (
                [0.0, 0.5013, 0.0, 0.0, 0.0, 0.0, 1.0],
                0xb5a9_497b_83e0_101a,
            ),
            ([12.0, 1.0, 1.0, 1.0, 0.0, -3.0, 1.0], 0x8e3c_5af9_66a4_e27f),
            (
                [24.0, 0.0, 0.0, 0.0, 0.0, -12.7, 0.0],
                0x83d1_b761_9c3e_105a,
            ),
            ([6.0, 0.3, 0.0, 1.0, 1.0, 0.0, 1.0], 0x5945_20ae_a6f3_f89e),
        ] {
            let (l, r) = run(values, 44_100 * 3, 512);
            let mut fnv = 0xcbf2_9ce4_8422_2325_u64;
            for v in l.iter().zip(&r).flat_map(|(a, b)| [a, b]) {
                fnv ^= u64::from(v.to_bits());
                fnv = fnv.wrapping_mul(0x100_0000_01b3);
            }
            assert_eq!(fnv, hash, "{values:?}");
        }
    }

    #[test]
    fn the_block_length_changes_nothing_and_a_reset_empties_it() {
        // Level matching takes the input gain where a block ends, so a
        // gliding input gain is heard by the block; without it, not.
        let values = [12.0, 1.0, 1.0, 0.0, 0.0, -3.0, 1.0];
        assert_eq!(run(values, 30_000, 512), run(values, 30_000, 77));
        let mut limiter = LimiterLite::new(48_000.0);
        assert_eq!(limiter.latency(), 96);
        let (mut l, mut r) = input(2000);
        limiter.process(&mut l, &mut r);
        limiter.reset();
        let (mut l, mut r) = (vec![0.0; 500], vec![0.0; 500]);
        limiter.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| *v == 0.0));
    }

    /// Run the input through the limiter into `LL_OUT`, with the values
    /// in `LL_PARAMS`, to set beside the emulator's.
    #[test]
    #[ignore]
    fn run_file() {
        let out = std::env::var("LL_OUT").unwrap();
        let values: Vec<f32> = std::env::var("LL_PARAMS")
            .unwrap()
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        let (l, r) = run(values.try_into().unwrap(), 44_100 * 3, 512);
        let raw: Vec<u8> = l
            .iter()
            .zip(&r)
            .flat_map(|(a, b)| [a.to_le_bytes(), b.to_le_bytes()])
            .flatten()
            .collect();
        std::fs::write(out, raw).unwrap();
    }
}
