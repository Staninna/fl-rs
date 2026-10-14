//! Spreader: what is above a crossover has its side added to its mid and
//! its mid to its side, both a few milliseconds late, which makes a mono
//! sound wide, as `docs/plugins/spreader.md` has it.
//!
//! The state is 30 bytes: a little-endian i32 version, 0x10003, the
//! plugin's five parameters as i32, a word of the editor's that FL does not
//! read back, and two bytes for the editor's two monitors. FL also reads 24
//! bytes with version 0x10002 and no editor part, and 12 bytes with no
//! version, which are the first three parameters of an older build with
//! the two switches at 0, and so switched off. So does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Spread | 0 to 10000 | 5000 |
//! | 1 Stereo separation | -10000 to 10000 | 0 |
//! | 2 Low-Frequency Bypass | 1 to 1000000 | 1 |
//! | 3 Mono | 0 or 1 | 0 |
//! | 4 Enabled | 0 or 1 | 1 |
//!
//! FL's plugin hands these to a processor in `ildsp_x64.dll`, which Faust
//! made. This is that processor, sample for sample:
//!
//! ```text
//! low, high = a fourth-order crossover of each side at the bypass frequency
//! S = highL - highR;   M = highL + highR
//! A = M / 2 + mono * (lowL + lowR) / 2 + shelf(w * late(S) / 2)
//! B = 2 sep * (S / 2 + shelf(w * late(M) / 2))
//! outL = (1 - mono) * lowL + A + B
//! outR = (1 - mono) * lowR + A - B
//! ```
//!
//! `w` is the spread wheel from 0 to 1, and `late` holds a signal back by
//! `w` times 6.35 ms. When that time changes, the processor fades over
//! 23 ms from a tap at the old time to one at the new. `shelf` takes 8 dB
//! off above 15 kHz. `sep` is the separation wheel from 0 at its low end
//! through a half in the middle to 1. The bypass frequency is 1000 Hz
//! times the wheel's first half and 500 Hz to 10 kHz over its second, and
//! never under 10 Hz. The two bands of the crossover add up to the input
//! through an allpass, so with the spread at 0 each side is as loud as it
//! came and on the side it came on.
//!
//! Every wheel is followed through a one-pole of 2 ms. A flush and a new
//! rate put those at 0 with all the rest, as in FL, so the plugin comes in
//! from "switched off" over a few milliseconds. Switched off, it passes
//! its input. It is on time.

use crate::{Effect, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Spreader";

/// The range of each parameter's control. The DFM has the bypass wheel
/// from 1000, and every new instance saves 1.
const RANGES: [(i32, i32); 5] = [
    (0, 10_000),
    (-10_000, 10_000),
    (1, 1_000_000),
    (0, 1),
    (0, 1),
];
const DEFAULTS: [i32; 5] = [5000, 0, 1, 0, 1];

/// How long the state FL writes is, the one of the version before it, and
/// the one with no version.
const LEN: usize = 30;
const LEN_2: usize = 24;
const LEN_OLD: usize = 12;

/// How long the two delay lines are: more than the longest delay at
/// 192 kHz, the highest rate the processor counts with.
const LINE: usize = 2048;
const MASK: u32 = LINE as u32 - 1;

/// The crossover's damping, and twice it: what takes the band-pass off the
/// input to leave the allpass the two bands add up to.
const DAMPING: f32 = std::f32::consts::SQRT_2;
const ALLPASS: f32 = 2.828_427;

/// 8 dB down, for what the shelf keeps above its corner, and twice that.
const SHELF: f32 = 0.398_107_17;
const SHELF_TWICE: f32 = 0.796_214_34;

/// The lowest bypass frequency the processor takes, in Hz.
const LOWEST: f32 = 10.0;

/// Where the bypass wheel puts the crossover, in Hz, before the floor:
/// 1000 Hz times the first half of the wheel, and 500 Hz to 10 kHz over
/// the second.
pub fn bypass_hz(value: i32) -> f32 {
    let (min, max) = RANGES[2];
    let v = (f64::from(value - min) / f64::from(max - min)).clamp(0.0, 1.0);
    let v = f64::from(v as f32);
    let hz = if v <= 0.5 {
        1000.0 * v
    } else {
        500.0 + 9500.0 * ((v - 0.5) * 2.0)
    };
    hz as f32
}

/// What the processor reads its five controls as: the width, the
/// separation, the bypass frequency, the mono switch and the on switch.
fn zones(params: &[i32; 5]) -> [f32; 5] {
    [
        (f64::from(params[0]) / 10_000.0) as f32,
        (f64::from(params[1] + 10_000) / 20_000.0) as f32,
        bypass_hz(params[2]).max(LOWEST),
        params[3] as f32,
        params[4] as f32,
    ]
}

/// What the processor works out from the rate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Consts {
    /// What a smoother keeps of itself every sample, and one less that.
    keep: f32,
    take: f32,
    /// Pi over the rate.
    turn: f32,
    /// The longest delay, in samples, and how much of a fade a sample is.
    longest: i32,
    fade: f32,
    /// The 15 kHz split: `1 - c`, `c / 2` and `1 / (1 + c)` with `c` one
    /// over the tangent of its angle.
    h0: f32,
    h1: f32,
    h2: f32,
}

impl Consts {
    fn new(hz: u32) -> Self {
        let rate = (hz as f32).clamp(1.0, 192_000.0);
        let over = 1.0 / rate;
        let keep = (over * -500.0).exp();
        let c = 1.0 / (over * 47_123.89).tan();
        Consts {
            keep,
            take: 1.0 - keep,
            turn: over * std::f32::consts::PI,
            longest: (rate * 0.006_349_206_4) as i32,
            fade: 1.0 / ((rate * 0.023_219_954) as i32) as f32,
            h0: 1.0 - c,
            h1: c * 0.5,
            h2: 1.0 / (c + 1.0),
        }
    }
}

/// One side's crossover: two state-variable low-passes in a row.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Cross {
    s: [f32; 4],
}

impl Cross {
    /// The allpass the two bands add up to, and the low band. `g` is the
    /// tangent of the corner's angle, `gk` that and the damping, `d` one
    /// over `1 + g * gk`.
    fn step(&mut self, x: f32, g: f32, gk: f32, d: f32) -> (f32, f32) {
        let [s1, s2, s3, s4] = self.s;
        let v = (x - (s1 * gk + s2)) * g * d;
        let band = s1 + v;
        let allpass = x - band * ALLPASS;
        let part = band * g;
        let first = part + s2;
        let v2 = (first - (s3 * gk + s4)) * g * d;
        let part2 = (s3 + v2) * g;
        self.s = [
            (v + v) + s1,
            (part + part) + s2,
            (v2 + v2) + s3,
            (part2 + part2) + s4,
        ];
        (allpass, s4 + part2)
    }
}

/// The 15 kHz split of one delayed signal: a first-order low-pass and
/// high-pass of the same input.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Split {
    last: f32,
    low: f32,
    high: f32,
}

impl Split {
    /// Take `z`, which is twice the signal: the halving is in the
    /// coefficients.
    fn step(&mut self, z: f32, c: &Consts) {
        self.high = ((z - self.last) * c.h1 - c.h0 * self.high) * c.h2;
        self.low = ((self.last + z) * 0.5 - c.h0 * self.low) * c.h2;
        self.last = z;
    }
}

/// The two taps and the fade between them: Faust's `sdelay`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Fade {
    /// What a sample adds to `at`, 0 when no fade is under way.
    step: f32,
    /// 0 at the first tap, 1 at the second.
    at: f32,
    taps: [i32; 2],
}

impl Fade {
    /// One sample on, with `wanted` the delay the width asks for.
    fn step(&mut self, wanted: i32, fade: f32) {
        let (at, [first, second]) = (self.at, self.taps);
        self.step = if self.step != 0.0 {
            if at > 0.0 && at < 1.0 { self.step } else { 0.0 }
        } else if at == 0.0 {
            if wanted == first { 0.0 } else { fade }
        } else if at == 1.0 && wanted != second {
            -fade
        } else {
            0.0
        };
        self.at = (at + self.step).clamp(0.0, 1.0);
        // The tap that is not heard takes the new delay.
        self.taps = [
            if at >= 1.0 && second != wanted {
                wanted
            } else {
                first
            },
            if at <= 0.0 && first != wanted {
                wanted
            } else {
                second
            },
        ];
    }
}

/// What follows the five controls, one pole each.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Smooth {
    width: f32,
    separation: f32,
    bypass: f32,
    mono: f32,
    on: f32,
}

/// Spreader. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Spreader {
    params: [i32; 5],
    /// The editor's part of the state, which nothing here reads.
    editor: [u8; 6],
    hz: u32,
    consts: Consts,
    zones: [f32; 5],
    smooth: Smooth,
    cross: [Cross; 2],
    /// The side and the mid of what is above the crossover, in rings, and
    /// the count of the samples so far.
    side: Vec<f32>,
    mid: Vec<f32>,
    count: u32,
    fade: Fade,
    /// The 15 kHz splits of the delayed side and the delayed mid.
    split_side: Split,
    split_mid: Split,
}

impl Default for Spreader {
    fn default() -> Self {
        Spreader {
            params: DEFAULTS,
            editor: [0, 0, 0, 0, 1, 1],
            hz: 44_100,
            consts: Consts::new(44_100),
            zones: zones(&DEFAULTS),
            smooth: Smooth::default(),
            cross: [Cross::default(); 2],
            side: vec![0.0; LINE],
            mid: vec![0.0; LINE],
            count: 0,
            fade: Fade::default(),
            split_side: Split::default(),
            split_mid: Split::default(),
        }
    }
}

impl Spreader {
    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 5]) {
        self.params = params;
        self.zones = zones(&params);
    }

    /// The processor's clear: everything but the controls at 0.
    fn clear(&mut self) {
        self.smooth = Smooth::default();
        self.cross = [Cross::default(); 2];
        self.side.fill(0.0);
        self.mid.fill(0.0);
        self.count = 0;
        self.fade = Fade::default();
        self.split_side = Split::default();
        self.split_mid = Split::default();
    }

    /// A number too small for an f32 to hold in full becomes 0, before it
    /// slows the sums down. FL runs with such numbers rounded away.
    fn flush(&mut self) {
        let smooth = &mut self.smooth;
        let [left, right] = &mut self.cross;
        let (side, mid) = (&mut self.split_side, &mut self.split_mid);
        let states = [
            &mut smooth.width,
            &mut smooth.separation,
            &mut smooth.bypass,
            &mut smooth.mono,
            &mut smooth.on,
            &mut side.last,
            &mut side.low,
            &mut side.high,
            &mut mid.last,
            &mut mid.low,
            &mut mid.high,
        ];
        for state in states.into_iter().chain(&mut left.s).chain(&mut right.s) {
            if state.is_subnormal() {
                *state = 0.0;
            }
        }
    }
}

impl Effect for Spreader {
    /// What is sounding stays, and the controls go to the state's through
    /// their smoothers, as in FL.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let word = state_version(bytes, LEN)?;
        let version = word.saturating_sub(0x1_0000).max(1);
        let expected = match version {
            1 => LEN_OLD,
            2 => LEN_2,
            3 => LEN,
            _ => return Err(StateError::Version { found: word }),
        };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let words = bytes.as_chunks::<4>().0;
        let read = |at: usize| i32::from_le_bytes(words[at]);
        // With no version there are three parameters from the first word
        // on, and the two switches are 0.
        let params = if version == 1 {
            [read(0), read(1), read(2), 0, 0]
        } else {
            [1, 2, 3, 4, 5].map(read)
        };
        for (param, (value, range)) in params.iter().zip(RANGES).enumerate() {
            if *value < range.0 || *value > range.1 {
                return Err(StateError::Range {
                    param,
                    value: *value,
                });
            }
        }
        if version == 3 {
            self.editor.copy_from_slice(&bytes[24..]);
        }
        self.set(params);
        Ok(())
    }

    /// FL hands its processor the rate in whole Hz, and the processor
    /// starts over: see [`Spreader::clear`]. A rate that is not a number
    /// of 1 Hz or more is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 && hz as u32 != self.hz {
            self.hz = hz as u32;
            self.consts = Consts::new(self.hz);
            self.clear();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let c = self.consts;
        let [width, separation, bypass, mono, on] = self.zones.map(|zone| c.take * zone);
        let longest = c.longest as f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let smooth = &mut self.smooth;
            smooth.bypass = smooth.bypass * c.keep + bypass;
            let g = (smooth.bypass * c.turn).tan();
            let gk = g + DAMPING;
            smooth.on = smooth.on * c.keep + on;
            let (in_l, in_r) = (*l, *r);
            let d = 1.0 / (gk * g + 1.0);
            let (allpass_l, low_l) = self.cross[0].step(smooth.on * in_l, g, gk, d);
            let (allpass_r, low_r) = self.cross[1].step(smooth.on * in_r, g, gk, d);

            let at = (self.count & MASK) as usize;
            self.side[at] = (low_r + allpass_l) - (allpass_r + low_l);
            smooth.width = smooth.width * c.keep + width;
            self.fade.step((smooth.width * longest) as i32, c.fade);
            let [first, second] = self.fade.taps.map(|tap| {
                let back = tap.max(0).min(c.longest) as u32;
                (self.count.wrapping_sub(back) & MASK) as usize
            });
            let near = self.side[first];
            let late = (near - (near - self.side[second]) * self.fade.at) * smooth.width;
            self.split_side.step(late, &c);
            let (low_s, high_s) = (self.split_side.low, self.split_side.high);

            let allpass = allpass_r + allpass_l;
            let low = low_r + low_l;
            let top = high_s * SHELF_TWICE + allpass;
            self.mid[at] = allpass - low;
            let near = self.mid[first];
            let late = (near - (near - self.mid[second]) * self.fade.at) * smooth.width;
            smooth.mono = smooth.mono * c.keep + mono;
            smooth.separation = smooth.separation * c.keep + separation;
            self.split_mid.step(late, &c);
            let (low_m, high_m) = (self.split_mid.low, self.split_mid.high);

            let dry = 1.0 - smooth.on;
            let apart = low_s - (allpass_l - low_l);
            let apart = (apart + apart) + top;
            let same = (((smooth.mono * low + low_s) + (top + low_s)) - low) * 0.5;
            let other = ((low - apart) * 0.5 + (high_m + high_s) * SHELF) + (low_m + low_s);
            let other = other * (smooth.separation + smooth.separation);
            let stays = 1.0 - smooth.mono;
            *l = (dry * in_l + same) + (stays * low_l + other);
            *r = ((stays * low_r + dry * in_r) + same) - other;
            self.count = self.count.wrapping_add(1);
        }
        self.flush();
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

    /// FL's flush: the processor's clear.
    fn reset(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, partial, presets, sine, state};

    /// A state of version 0x10003 with these parameters.
    fn bytes(params: [i32; 5]) -> Vec<u8> {
        let mut bytes = state(&[0x1_0003]);
        bytes.extend(state(&params));
        bytes.extend([0, 0, 0, 0, 1, 1]);
        bytes
    }

    fn with(params: [i32; 5]) -> Spreader {
        let mut plugin = Spreader::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    /// Run a plugin over a block in pieces, and give what came out.
    fn run(plugin: &mut Spreader, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (left.to_vec(), right.to_vec());
        for (l, r) in l.chunks_mut(441).zip(r.chunks_mut(441)) {
            plugin.process(l, r);
        }
        (l, r)
    }

    /// A tone that turns a whole number of times in 4096 samples.
    fn tone(turns_in_4096: f64, len: usize, level: f32) -> Vec<f32> {
        let tone = sine(turns_in_4096 / 4096.0, len);
        tone.iter().map(|v| v * level).collect()
    }

    /// The level of the partial that turns `turns_in_4096` times in the
    /// last 4096 samples.
    fn level(samples: &[f32], turns_in_4096: f64) -> f64 {
        partial(&samples[samples.len() - 4096..], turns_in_4096 / 4096.0)
    }

    #[test]
    fn the_rate_gives_the_doc_s_constants() {
        let c = Consts::new(44_100);
        assert!((f64::from(c.keep) - (-500.0f64 / 44_100.0).exp()).abs() < 1e-7);
        assert_eq!(c.take, 1.0 - c.keep);
        // 6.35 ms is 280 samples, and a fade is 1024.
        assert_eq!((c.longest, c.fade), (280, 1.0 / 1024.0));
        let split = 1.0 / (std::f64::consts::PI * 15_000.0 / 44_100.0).tan();
        assert!((f64::from(c.h0) - (1.0 - split)).abs() < 1e-5);
        assert!((f64::from(c.h1) - split / 2.0).abs() < 1e-5);
        assert!((f64::from(c.h2) - 1.0 / (1.0 + split)).abs() < 1e-5);
        // The processor counts with no more than 192 kHz, where the
        // longest delay is 1219 samples: inside the lines.
        let top = Consts::new(384_000);
        assert_eq!(top, Consts::new(192_000));
        assert!(top.longest == 1219 && (top.longest as usize) < LINE);
        assert_eq!(Consts::new(48_000).longest, 304);
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_controls() {
        assert_eq!(zones(&DEFAULTS), [0.5, 0.5, 10.0, 0.0, 1.0]);
        assert_eq!(zones(&[0, -10_000, 1, 1, 0]), [0.0, 0.0, 10.0, 1.0, 0.0]);
        assert_eq!(
            zones(&[10_000, 10_000, 1_000_000, 0, 1])[..3],
            [1.0, 1.0, 10_000.0]
        );
        // The first half of the bypass wheel is 0 to 500 Hz and the second
        // 500 Hz to 10 kHz. Under 10 Hz the processor takes 10.
        assert_eq!((bypass_hz(1), bypass_hz(1_000_000)), (0.0, 10_000.0));
        for (value, hz) in [
            (106_000, 106.0),
            (500_000, 500.0),
            (516_274, 809.2),
            (717_250, 4627.7),
        ] {
            assert!((bypass_hz(value) - hz).abs() < 0.1, "{value}");
        }
        assert_eq!(zones(&[0, 0, 5000, 0, 1])[2], 10.0);
        assert!((zones(&[0, 0, 20_000, 0, 1])[2] - 20.0).abs() < 1e-3);
    }

    #[test]
    fn switched_off_the_input_comes_out_as_it_went_in() {
        let (left, right) = (tone(93.0, 5000, 0.8), tone(411.0, 5000, 0.3));
        let mut plugin = with([10_000, 7000, 300_000, 1, 0]);
        let (l, r) = run(&mut plugin, &left, &right);
        assert!(l[1..] == left[1..] && r[1..] == right[1..]);
        assert_eq!(plugin.latency(), 0);

        // Switched off while it sounds, it gets there in a few
        // milliseconds.
        let mut plugin = with([10_000, 7000, 300_000, 1, 1]);
        run(&mut plugin, &left, &right);
        plugin.set_param(4, 0.0);
        let (l, r) = run(&mut plugin, &left, &right);
        let apart = |out: &[f32], sent: &[f32]| {
            let apart = out[4000..].iter().zip(&sent[4000..]);
            apart.map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
        };
        assert!(apart(&l, &left) < 1e-6 && apart(&r, &right) < 1e-6);
    }

    #[test]
    fn with_the_spread_at_0_each_side_is_as_loud_as_it_came_on_its_own_side() {
        // A tone on the left and another, half as loud, on the right.
        let (left, right) = (tone(93.0, 16_384, 1.0), tone(232.0, 16_384, 0.5));
        for bypass in [1, 106_000, 600_000] {
            let mut plugin = with([0, 0, bypass, 0, 1]);
            let (l, r) = run(&mut plugin, &left, &right);
            assert!((level(&l, 93.0) - 1.0).abs() < 1e-3, "{bypass}");
            assert!((level(&r, 232.0) - 0.5).abs() < 1e-3, "{bypass}");
            assert!(level(&l, 232.0) < 1e-3 && level(&r, 93.0) < 1e-3);
        }
        // And what is alike on both sides stays alike.
        let mut plugin = with([0, 0, 1, 0, 1]);
        let (l, r) = run(&mut plugin, &left, &left);
        let apart = l.iter().zip(&r).map(|(l, r)| (l - r).abs());
        assert!(apart.fold(0.0, f32::max) < 1e-6);
    }

    #[test]
    fn a_mono_sound_gets_a_side_that_is_its_own_echo() {
        // Half a second for the width and the delay to settle, then a
        // click on both sides.
        let mut plugin = with(DEFAULTS);
        run(&mut plugin, &[0.0; 22_050], &[0.0; 22_050]);
        // The smoother stops a hair under a half, so the delay is the 139
        // samples under the 140 that half of 6.35 ms would be.
        let late = (plugin.smooth.width * 280.0) as usize;
        assert_eq!((late, plugin.fade.taps[0], plugin.fade.at), (139, 139, 0.0));

        let mut click = vec![0.0; 600];
        click[0] = 1.0;
        let (l, r) = run(&mut plugin, &click, &click);
        let mid: Vec<f32> = l.iter().zip(&r).map(|(l, r)| (l + r) / 2.0).collect();
        let side: Vec<f32> = l.iter().zip(&r).map(|(l, r)| (l - r) / 2.0).collect();
        // The mid is the click, and has no echo.
        assert!((mid[0] - 1.0).abs() < 0.01 && mid[late].abs() < 0.01);
        // The side has nothing until the echo, which is the click at half
        // the width through the shelf.
        assert!(side[..late].iter().all(|v| v.abs() < 1e-6));
        let c = plugin.consts;
        let first = 0.5 * (c.h2 + SHELF * 2.0 * c.h1 * c.h2);
        assert!((side[late] - first).abs() < 0.01, "{} {first}", side[late]);
        assert!(side[late] > 0.3);

        // A click on the side alone comes back in the mid.
        let mut plugin = with(DEFAULTS);
        run(&mut plugin, &[0.0; 22_050], &[0.0; 22_050]);
        let other: Vec<f32> = click.iter().map(|v| -v).collect();
        let (l, r) = run(&mut plugin, &click, &other);
        let mid: Vec<f32> = l.iter().zip(&r).map(|(l, r)| (l + r) / 2.0).collect();
        assert!(mid[..late].iter().all(|v| v.abs() < 1e-6));
        assert!((mid[late] - first).abs() < 0.01);
    }

    #[test]
    fn the_spread_wheel_sets_how_late_and_how_loud_the_echo_is() {
        let echo = |spread: i32| {
            let mut plugin = with([spread, 0, 1, 0, 1]);
            run(&mut plugin, &[0.0; 22_050], &[0.0; 22_050]);
            let mut click = vec![0.0; 600];
            click[0] = 1.0;
            let (l, r) = run(&mut plugin, &click, &click);
            let side: Vec<f32> = l.iter().zip(&r).map(|(l, r)| (l - r) / 2.0).collect();
            let at = side.iter().position(|v| v.abs() > 1e-3);
            at.map(|at| (at, side[at]))
        };
        assert_eq!(echo(0), None);
        let (quarter, half, all) = (
            echo(2500).unwrap(),
            echo(5000).unwrap(),
            echo(10_000).unwrap(),
        );
        assert_eq!((quarter.0, half.0, all.0), (69, 139, 279));
        assert!((half.1 / quarter.1 - 2.0).abs() < 0.01 && (all.1 / half.1 - 2.0).abs() < 0.01);
    }

    #[test]
    fn the_separation_wheel_scales_the_side() {
        // A tone on the left alone, with no spread: the side that comes
        // out is none of it, all of it, and twice it.
        let (left, right) = (tone(93.0, 16_384, 1.0), vec![0.0; 16_384]);
        let side = |separation: i32| {
            let mut plugin = with([0, separation, 1, 0, 1]);
            let (l, r) = run(&mut plugin, &left, &right);
            let side: Vec<f32> = l.iter().zip(&r).map(|(l, r)| l - r).collect();
            let mid: Vec<f32> = l.iter().zip(&r).map(|(l, r)| l + r).collect();
            (level(&side, 93.0), level(&mid, 93.0))
        };
        for (wheel, times) in [
            (-10_000, 0.0),
            (-5000, 0.5),
            (0, 1.0),
            (5000, 1.5),
            (10_000, 2.0),
        ] {
            let (side, mid) = side(wheel);
            assert!((side - times).abs() < 1e-3, "{wheel}: {side}");
            assert!((mid - 1.0).abs() < 1e-3, "{wheel}: {mid}");
        }
    }

    #[test]
    fn mono_sums_what_is_under_the_bypass_frequency() {
        // The bypass at 2 kHz: a 484 Hz tone on the left is under it and a
        // 9.4 kHz tone on the right above.
        let (left, right) = (tone(45.0, 16_384, 1.0), tone(873.0, 16_384, 0.5));
        let mut stereo = with([0, 0, 578_948, 0, 1]);
        assert!((stereo.zones[2] - 2000.0).abs() < 0.1);
        let (l, r) = run(&mut stereo, &left, &right);
        assert!((level(&l, 45.0) - 1.0).abs() < 0.01 && level(&r, 45.0) < 0.01);

        let mut mono = with([0, 0, 578_948, 1, 1]);
        let (l, r) = run(&mut mono, &left, &right);
        // The low tone is on both sides at a half, and the high one where
        // it was.
        assert!((level(&l, 45.0) - 0.5).abs() < 0.01 && (level(&r, 45.0) - 0.5).abs() < 0.01);
        assert!((level(&r, 873.0) - 0.5).abs() < 0.01 && level(&l, 873.0) < 0.01);
    }

    #[test]
    fn a_new_delay_is_faded_in_between_two_taps() {
        let mut fade = Fade::default();
        let step = 1.0 / 4.0;
        // Nothing to do while the delay is the first tap's.
        fade.step(0, step);
        assert_eq!(fade, Fade::default());
        // A new delay goes to the second tap, and the fade starts.
        fade.step(7, step);
        assert_eq!((fade.step, fade.at, fade.taps), (step, 0.25, [0, 7]));
        // What the width asks for meanwhile waits.
        fade.step(9, step);
        fade.step(9, step);
        assert_eq!((fade.at, fade.taps), (0.75, [0, 7]));
        fade.step(9, step);
        assert_eq!((fade.step, fade.at, fade.taps), (step, 1.0, [0, 7]));
        // At the second tap the fade stops, the first takes the new delay,
        // and the next fade goes back to it.
        fade.step(9, step);
        assert_eq!((fade.step, fade.at, fade.taps), (0.0, 1.0, [9, 7]));
        fade.step(9, step);
        assert_eq!((fade.step, fade.at, fade.taps), (-step, 0.75, [9, 7]));
        for _ in 0..4 {
            fade.step(9, step);
        }
        assert_eq!((fade.step, fade.at, fade.taps), (0.0, 0.0, [9, 7]));
        fade.step(9, step);
        assert_eq!((fade.step, fade.at, fade.taps), (0.0, 0.0, [9, 7]));
    }

    #[test]
    fn a_flush_and_a_new_rate_start_the_processor_over() {
        let mut plugin = with([8000, 3000, 106_000, 1, 1]);
        let fresh = plugin.clone();
        let (left, right) = (tone(93.0, 3000, 1.0), tone(232.0, 3000, 0.5));
        let (first_l, first_r) = run(&mut plugin, &left, &right);
        assert_ne!(plugin, fresh);
        plugin.reset();
        assert_eq!(plugin, fresh);
        // So the same block comes out the same again.
        let (again_l, again_r) = run(&mut plugin, &left, &right);
        assert!(first_l == again_l && first_r == again_r);

        plugin.set_sample_rate(48_000.0);
        assert_eq!(
            (plugin.consts.longest, plugin.smooth),
            (304, Smooth::default())
        );
        assert!(plugin.side.iter().all(|v| *v == 0.0));
        // The same rate again, or a rate that is not one, changes nothing.
        run(&mut plugin, &left, &right);
        let before = plugin.clone();
        plugin.set_sample_rate(48_000.0);
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory_and_is_the_same_plugin_as_its_state() {
        let mut plugin = Spreader::default();
        let memory = |plugin: &Spreader| (plugin.side.as_ptr(), plugin.mid.as_ptr());
        let before = memory(&plugin);
        let moves = [10_000.4, -20_000.0, 0.0, 7.0, f32::NAN];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
            plugin.process(&mut [0.25; 200], &mut [-0.25; 200]);
        }
        plugin.set_param(5, 1.0);
        assert_eq!((plugin.param(5), plugin.param_count()), (0.0, 5));
        assert_eq!(
            [0, 1, 2, 3, 4].map(|i| plugin.param(i)),
            [10_000.0, -10_000.0, 1.0, 1.0, 0.0]
        );
        assert_eq!(memory(&plugin), before);
        plugin.reset();
        assert_eq!(plugin, with([10_000, -10_000, 1, 1, 0]));
        assert_eq!(with(DEFAULTS), Spreader::default());
    }

    #[test]
    fn the_three_states_fl_reads_load_and_the_others_are_turned_down() {
        let mut plugin = with([8000, 3000, 106_000, 1, 1]);
        let before = plugin.clone();
        for len in [3, 24, 29, 34] {
            let mut wrong = bytes(DEFAULTS);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 30,
                    found: len
                })
            );
        }
        for (param, value) in [
            (0, 10_001),
            (1, -10_001),
            (2, 0),
            (2, 1_000_001),
            (3, 2),
            (4, -1),
        ] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params)),
                Err(StateError::Range { param, value })
            );
        }
        let mut newer = bytes(DEFAULTS);
        newer[0] = 4;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 0x1_0004 })
        );
        assert_eq!(plugin, before);

        // Version 0x10002 has no editor part.
        let mut second = state(&[0x1_0002]);
        second.extend(state(&[2500, -4000, 300_000, 1, 1]));
        assert_eq!(plugin.set_state(&second), Ok(()));
        assert_eq!(plugin.params, [2500, -4000, 300_000, 1, 1]);
        // With no version there are three parameters, and the plugin is
        // switched off.
        assert_eq!(plugin.set_state(&state(&[5000, 1000, 1])), Ok(()));
        assert_eq!(plugin.params, [5000, 1000, 1, 0, 0]);
        assert_eq!(plugin.zones[4], 0.0);
        assert_eq!(
            plugin.set_state(&state(&[5000, 1000, 1, 0])),
            Err(StateError::Length {
                expected: 12,
                found: 16
            })
        );
        // The monitors' two bytes are kept.
        let mut hidden = bytes(DEFAULTS);
        hidden[28] = 0;
        plugin.set_state(&hidden).unwrap();
        assert_eq!(plugin.editor, [0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn the_states_fl_ships_load() {
        let Some(own) = presets(NAME) else { return };
        // The plugin has no presets of its own. The plugin databases have
        // the default three times and once in the oldest format.
        assert!(own.is_empty());
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        let mut lens: Vec<usize> = all.iter().map(|preset| preset.state.len()).collect();
        lens.sort_unstable();
        assert_eq!(lens, [12, 30, 30, 30]);
        for preset in &all {
            let mut plugin = Spreader::default();
            plugin.set_state(&preset.state).unwrap();
            if preset.state.len() == 30 {
                assert_eq!(plugin, Spreader::default(), "{}", preset.file);
            } else {
                assert_eq!(plugin.params, [5000, 1000, 1, 0, 0]);
            }
        }
    }
}
