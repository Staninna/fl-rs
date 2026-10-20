//! DirectWave's three program effects, as `docs/plugins/directwave.md`
//! section 4.1 reads them: a delay (`FUN_180011f10`), a chorus
//! (`FUN_1800119c0`) and a plate reverb (`FUN_18002d0b0`). The engine keeps
//! one of each (`engine+0xa8710`, `+0x8a9050`, `+0xaa9210`) and runs them
//! once a step for every part whose program has zones, after the part's
//! voices, on the program's settings: each reads what the voices sent it
//! and adds its output to the part's dry buffers. They draw from the same
//! `rand` as the voices, one number a sample each for the delay and the
//! chorus.

// The reverb walks its twelve lines and seven taps by number, as FL's
// does.
#![allow(clippy::needless_range_loop)]

/// `2 pi` as DirectWave works it out at load: `atanf(1) * 8`.
const TAU: f32 = std::f32::consts::TAU;
/// `pi`: `atanf(1) * 4`.
const PI: f32 = std::f32::consts::PI;

/// What a step hands every effect: the rate, the tempo and the length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Context {
    pub rate: f32,
    pub tempo: f32,
}

/// One biquad stage (`FUN_180014d50`): `y1, y2, x1, x2`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Stage([f32; 4]);

impl Stage {
    fn run(&mut self, x: f32, c: &[f32; 5]) -> f32 {
        let s = &mut self.0;
        let y = ((s[2] * c[1] + x * c[0] + c[2] * s[3]) - s[0] * c[3]) - c[4] * s[1];
        (s[3], s[2], s[1], s[0]) = (s[2], x, s[0], y);
        y
    }
}

/// An allpass read between two points of a delay line, where `older` is
/// the point after `newer`: `f x[n] + x[n + 1] - f y`, with `f = 1 - frac`.
fn allpass_read(f: f32, newer: f32, older: f32, last: f32) -> f32 {
    (f * newer + older) - f * last
}

/// The delay (`FUN_180011f10`): two lines of 65536 points read through an
/// allpass, a highpass and a lowpass biquad in each side's feedback.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Delay {
    lines: [Vec<f32>; 2],
    /// The delay in samples and the feedback, each gliding to the knob.
    time: f32,
    feedback: f32,
    /// Where the next point goes; it counts down.
    at: u32,
    /// What each side's allpass read last.
    taps: [f32; 2],
    /// Highpass then lowpass, left and right.
    stages: [[Stage; 2]; 2],
}

const DELAY_POINTS: usize = 65_536;

impl Default for Delay {
    fn default() -> Self {
        Delay {
            lines: [vec![0.0; DELAY_POINTS], vec![0.0; DELAY_POINTS]],
            time: 0.0,
            feedback: 0.0,
            at: 0,
            taps: [0.0; 2],
            stages: [[Stage::default(); 2]; 2],
        }
    }
}

impl Delay {
    /// `knobs`: the time in beats, the feedback, the highpass and the
    /// lowpass knobs; `cross` the ping-pong flag. `sends` are the voices'
    /// left and right, `out` the part's dry buffers.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        knobs: [f32; 4],
        cross: bool,
        ctx: Context,
        sends: [&[f32]; 2],
        out: [&mut [f32]; 2],
        rand: &mut impl FnMut() -> i32,
    ) {
        let rate = ctx.rate;
        let time = ((rate * 60.0) / ctx.tempo * knobs[0]).max(rate * 0.001);
        let feedback = knobs[1];
        let glide = 16.0 / rate;
        let q = 2f32.sqrt() * 0.5;
        let high = super::directwave::coefficients(2, super::directwave::cutoff(knobs[2]), q, rate);
        let low = super::directwave::coefficients(1, super::directwave::cutoff(knobs[3]), q, rate);
        let [out_left, out_right] = out;
        for n in 0..out_left.len() {
            let noise = rand() as f32 * 1e-14;
            let whole = self.time as i32;
            let at = whole.wrapping_add(self.at as i32) as u32;
            let (newer, older) = (
                (at & 0xffff) as usize,
                (at.wrapping_add(1) & 0xffff) as usize,
            );
            let f = 1.0 - (self.time - whole as f32);
            let right = allpass_read(f, self.lines[1][newer], self.lines[1][older], self.taps[1]);
            let left = allpass_read(f, self.lines[0][newer], self.lines[0][older], self.taps[0]);
            self.taps = [left, right];
            let write = (self.at & 0xffff) as usize;
            let into_left = noise + sends[0][n];
            let into_right = if cross {
                let x = right * self.feedback + into_left;
                self.lines[0][write] = self.filter(0, x, &high, &low);
                left * self.feedback + noise
            } else {
                let x = left * self.feedback + into_left;
                self.lines[0][write] = self.filter(0, x, &high, &low);
                right * self.feedback + (noise + sends[1][n])
            };
            self.lines[1][write] = self.filter(1, into_right, &high, &low);
            self.at = self.at.wrapping_sub(1);
            out_left[n] += self.taps[0];
            out_right[n] += self.taps[1];
            self.time = ((time - self.time) * glide + noise) + self.time;
            self.feedback = ((feedback - self.feedback) * glide + noise) + self.feedback;
        }
    }

    fn filter(&mut self, side: usize, x: f32, high: &[f32; 5], low: &[f32; 5]) -> f32 {
        let [a, b] = &mut self.stages[side];
        b.run(a.run(x, high), low)
    }
}

/// The chorus (`FUN_1800119c0`): two lines of 16384 points read through an
/// allpass at delays a sine and a cosine move, with feedback.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Chorus {
    lines: [Vec<f32>; 2],
    delays: [f32; 2],
    feedback: f32,
    at: u32,
    phase: f32,
    taps: [f32; 2],
}

const CHORUS_POINTS: usize = 16_384;

impl Default for Chorus {
    fn default() -> Self {
        Chorus {
            lines: [vec![0.0; CHORUS_POINTS], vec![0.0; CHORUS_POINTS]],
            delays: [0.0; 2],
            feedback: 0.0,
            at: 0,
            phase: 0.0,
            taps: [0.0; 2],
        }
    }
}

impl Chorus {
    /// `knobs`: the delay, the depth, the speed and the feedback.
    pub fn run(
        &mut self,
        knobs: [f32; 4],
        ctx: Context,
        sends: [&[f32]; 2],
        out: [&mut [f32]; 2],
        rand: &mut impl FnMut() -> i32,
    ) {
        let rate = ctx.rate;
        let ms = rate * 0.001;
        let depth = knobs[1] * knobs[1] * 24.0 * ms * 0.5;
        let centre = (knobs[0] * knobs[0] * 47.9 + 0.1) * ms;
        let speed = knobs[2] * TAU * knobs[2] * 20.0 / rate;
        let glide = 16.0 / rate;
        let middle = depth + centre;
        let mask = CHORUS_POINTS as u32 - 1;
        let [out_left, out_right] = out;
        for n in 0..out_left.len() {
            let noise = rand() as f32 * 1e-14;
            let to_left = self.phase.sin() * depth + middle;
            let to_right = (TAU * 0.25 + self.phase).sin() * depth + middle;
            let read = |line: &[f32], delay: f32, last: f32, at: u32| {
                let whole = delay as i32;
                let point = at.wrapping_add(whole as u32);
                let f = 1.0 - (delay - whole as f32);
                let (newer, older) = (point & mask, point.wrapping_add(1) & mask);
                allpass_read(f, line[newer as usize], line[older as usize], last)
            };
            let left = read(&self.lines[0], self.delays[0], self.taps[0], self.at);
            let right = read(&self.lines[1], self.delays[1], self.taps[1], self.at);
            self.taps = [left, right];
            out_left[n] += left;
            out_right[n] += right;
            let write = (self.at & mask) as usize;
            self.lines[0][write] = (left * self.feedback + sends[0][n]) + noise;
            self.lines[1][write] = (right * self.feedback + sends[1][n]) + noise;
            self.phase += speed;
            if TAU < self.phase {
                self.phase -= TAU;
            }
            self.at = self.at.wrapping_sub(1);
            self.delays[0] = ((to_left - self.delays[0]) * glide + noise) + self.delays[0];
            self.delays[1] = ((to_right - self.delays[1]) * glide + noise) + self.delays[1];
            self.feedback = ((knobs[3] - self.feedback) * glide + noise) + self.feedback;
        }
    }
}

/// The lengths of the reverb's twelve lines at a rate of 20000 and the
/// largest room: four input allpasses, then per half of the tank an
/// allpass, a delay, an allpass and a delay (`0x1801076e0`).
const LINES: [i32; 12] = [
    142, 107, 379, 277, 672, 4453, 1800, 3720, 908, 4217, 2656, 3163,
];
/// Where the left and the right output read, the same way
/// (`0x1801076a0`, `0x1801076c0`).
const LEFT_TAPS: [i32; 7] = [266, 2974, 1913, 2496, 1690, 187, 1566];
const RIGHT_TAPS: [i32; 7] = [353, 3627, 1728, 3173, 1811, 335, 621];

/// The plate reverb (`FUN_18002d0b0`, its lines laid out by
/// `FUN_18002c9a0`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Reverb {
    /// The rate in whole Hz the lines were made for, and the room size
    /// they were last cleared at.
    rate: i32,
    size: i32,
    lines: [Vec<f32>; 12],
    predelay: Vec<f32>,
    /// Where each line writes next, counting down, and how long it is now.
    at: [i32; 12],
    lengths: [i32; 12],
    predelay_at: i32,
    predelay_len: i32,
    left_taps: [i32; 7],
    right_taps: [i32; 7],
    /// The input lowpass, its highpass (last output, last input), the two
    /// halves' shelves (last input, last output) and their damping.
    input: f32,
    high: [f32; 2],
    shelves: [[f32; 2]; 2],
    damping: [f32; 2],
}

impl Reverb {
    /// `FUN_18002c9a0`: the lines for a rate, made once for each rate,
    /// which is where the reverb allocates.
    pub fn prepare(&mut self, rate: f32) {
        let rate = rate as i32;
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        for (k, line) in self.lines.iter_mut().enumerate() {
            let len = rate.wrapping_mul(LINES[k]) / 20_000;
            self.lengths[k] = len;
            *line = vec![0.0; len.max(1) as usize];
        }
        self.at = [0; 12];
        self.predelay_len = rate.wrapping_mul(250) / 1000;
        self.predelay = vec![0.0; self.predelay_len.max(1) as usize];
        self.predelay_at = 0;
        for k in 0..7 {
            self.left_taps[k] = rate.wrapping_mul(LEFT_TAPS[k]) / 20_000;
            self.right_taps[k] = rate.wrapping_mul(RIGHT_TAPS[k]) / 20_000;
        }
    }

    /// `knobs`: the room size, the damping, the diffusion and the decay.
    /// Nothing happens before [`Reverb::prepare`] at this rate.
    pub fn run(&mut self, knobs: [f32; 4], ctx: Context, sends: [&[f32]; 2], out: [&mut [f32]; 2]) {
        let rate = ctx.rate;
        if rate as i32 != self.rate {
            return;
        }
        let [size, damp, diffusion, decay] = knobs;
        let ln = 0.001f32.ln();
        let t = (PI * 94.5 / rate).tan();
        let high = (t - 1.0) / (t + 1.0);
        let b = 2.0 - (TAU * 22_000.0 / rate).cos();
        let low = 1.0 - (b - (b * b - 1.0).sqrt());

        self.predelay_len = ((rate * 0.0 * 0.001) as i32).max(1);
        self.predelay_at %= self.predelay_len;
        let room = (size * 99.0 + 1.0) as i32;
        let base = room.wrapping_add(15).wrapping_mul(self.rate) / 115;
        let capacity = |k: usize| self.lines[k].len() as i32;
        for k in 0..12 {
            let len = (base.wrapping_mul(LINES[k]) / 20_000).clamp(1, capacity(k));
            self.lengths[k] = len;
            self.at[k] %= len;
        }
        for k in 0..7 {
            self.left_taps[k] = base.wrapping_mul(LEFT_TAPS[k]) / 20_000;
            self.right_taps[k] = base.wrapping_mul(RIGHT_TAPS[k]) / 20_000;
        }
        if room != self.size {
            self.size = room;
            for (line, len) in self.lines.iter_mut().zip(self.lengths) {
                line[..len as usize].fill(0.0);
            }
        }
        let len = &self.lengths;
        let (mut g_in, mut g_a, mut g_b) = (0.0, 0.0, 0.0);
        if 0.0 < diffusion {
            let gain = |a: usize, b: usize, k: f32| {
                ((len[a] + len[b]) as f32 * 0.5 / rate * ln / (diffusion * k)).exp()
            };
            g_in = gain(0, 1, 0.07);
            g_a = gain(8, 4, 0.35);
            g_b = gain(6, 10, 0.51);
        }
        let loop_len = (len[11] + len[9] + len[7] + len[5]) as f32;
        let g = (loop_len / rate * ln / ((decay * 199.0 * decay + 1.0) / 10.0))
            .exp()
            .powf(0.25);
        // FL divides the gain by itself: 1, or NaN for a gain of 0.
        #[allow(clippy::eq_op)]
        let one = g / g;
        let q = if 1.0 < one { 1.0 } else { one };
        let lift = (one - 1.0) * 0.5;
        let t = (PI * 500.0 / rate).tan();
        let shelf = (t - q) / (t + q);
        let b = 2.0 - ((damp * 216.0 + 5.0) * 100.0 * TAU / rate).cos();
        let damping = 1.0 - (b - (b * b - 1.0).sqrt());
        let d = 1.0 - g;
        let (w07, w06, w025, w02, w05) = (
            d * 0.7 + g,
            d * 0.6 + g,
            d * 0.25 + g,
            d * 0.2 + g,
            d * 0.5 + g,
        );

        let [out_left, out_right] = out;
        for n in 0..out_left.len() {
            let tiny = 1e-30f32;
            let x = ((sends[1][n] + sends[0][n]) * 0.5 + tiny - self.input) * low + self.input;
            self.input = x;
            let h = (x - self.high[0]) * high + self.high[1];
            (self.high[1], self.high[0]) = (x, h);
            let v = (x - h) * 0.5 + tiny;
            // The input allpasses.
            let r0 = self.line(0) - v * g_in;
            let o0 = r0 * g_in;
            self.put(0, o0 + v);
            let r1 = self.line(1) - o0;
            let o1 = r1 * g_in;
            self.put(1, o1 + r0);
            let r2 = self.line(2) - o1;
            let o2 = r2 * g_in;
            self.put(2, o2 + r1);
            let r3 = self.line(3) - o2;
            self.put(3, r3 * g_in + r2);
            let input = self.predelay[self.predelay_at as usize];
            self.predelay[self.predelay_at as usize] = r3;
            self.predelay_at -= 1;
            if self.predelay_at < 0 {
                self.predelay_at = self.predelay_len - 1;
            }
            // The two halves of the tank, each fed by the other.
            let a = g * self.line(11);
            let feed = self.half(0, a, input, (lift, shelf, damping));
            let r4 = self.line(4) - feed * g_a;
            self.put(4, r4 * g_a + feed);
            let z = self.line(5);
            self.put(5, r4);
            let z = z * g;
            let r6 = self.line(6) - z * g_b;
            self.put(6, r6 * g_b + z);
            self.put(7, r6);
            let a = g * self.line(7);
            let feed = self.half(1, a, input, (lift, shelf, damping));
            let r8 = self.line(8) - feed * g_a;
            self.put(8, r8 * g_a + feed);
            let z = self.line(9);
            self.put(9, r8);
            let z = z * g;
            let r10 = self.line(10) - z * g_b;
            self.put(10, r10 * g_b + z);
            self.put(11, r10);
            // The taps.
            let (l, r) = (&self.left_taps, &self.right_taps);
            let tap = |k: usize, by: i32| {
                let at = by + self.at[k];
                let at = if at < self.lengths[k] {
                    at
                } else {
                    at - self.lengths[k]
                };
                self.lines[k][at as usize]
            };
            out_right[n] += ((((w025 * tap(5, r[1]) + tap(5, r[0])) - w07 * tap(6, r[2]))
                - w02 * tap(7, r[3]))
                - w06 * tap(9, r[4]))
                + tap(10, r[5])
                + w05 * tap(11, r[6]);
            out_left[n] += ((((tap(9, l[0]) - w025 * tap(9, l[1])) - w07 * tap(10, l[2]))
                - w02 * tap(11, l[3]))
                + w06 * tap(5, l[4]))
                + tap(6, l[5])
                + w05 * tap(7, l[6]);
        }
    }

    /// The point a line writes next, which is its oldest.
    fn line(&self, k: usize) -> f32 {
        self.lines[k][self.at[k] as usize]
    }

    /// Write a line's next point and move on.
    fn put(&mut self, k: usize, x: f32) {
        self.lines[k][self.at[k] as usize] = x;
        self.at[k] -= 1;
        if self.at[k] < 0 {
            self.at[k] = self.lengths[k] - 1;
        }
    }

    /// The shelf and the damping at the head of a half of the tank.
    fn half(
        &mut self,
        k: usize,
        a: f32,
        input: f32,
        (lift, shelf, damping): (f32, f32, f32),
    ) -> f32 {
        let [last_in, last_out] = &mut self.shelves[k];
        let y = (a - *last_out) * shelf + *last_in;
        (*last_in, *last_out) = (a, y);
        let state = &mut self.damping[k];
        *state = ((((y + a) * lift + a + input) - *state) * damping) + *state;
        *state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sends `tools/plugstate/directwave_fx_emu.py` feeds FL's effects:
    /// a C `rand`-style generator from 7, left block then right block,
    /// nothing in the second half of the blocks so the tails ring.
    fn sends(blocks: usize) -> Vec<[Vec<f32>; 2]> {
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345) & 0x7fff_ffff;
            ((f64::from(seed) / 2_147_483_647.0 - 0.5) * 0.8) as f32
        };
        (0..blocks)
            .map(|block| {
                let mut side = || (0..256).map(|_| next()).collect::<Vec<_>>();
                let (l, r) = (side(), side());
                if block >= blocks / 2 {
                    [vec![0.0; 256], vec![0.0; 256]]
                } else {
                    [l, r]
                }
            })
            .collect()
    }

    /// FNV-1a over the bits of what an effect adds to empty dry buffers,
    /// 256 samples a step, left then right.
    fn hash(effect: usize, knobs: [f32; 4], cross: bool, blocks: usize) -> u64 {
        let ctx = Context {
            rate: 44_100.0,
            tempo: 120.0,
        };
        let mut seed = 1u32;
        let mut rand = || {
            seed = seed.wrapping_mul(214_013).wrapping_add(2_531_011);
            ((seed >> 16) & 0x7fff) as i32
        };
        let (mut delay, mut chorus, mut reverb) =
            (Delay::default(), Chorus::default(), Reverb::default());
        reverb.prepare(44_100.0);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for [l, r] in sends(blocks) {
            let (mut ol, mut or) = (vec![0.0; 256], vec![0.0; 256]);
            let out = [&mut ol[..], &mut or[..]];
            match effect {
                0 => delay.run(knobs, cross, ctx, [&l, &r], out, &mut rand),
                1 => chorus.run(knobs, ctx, [&l, &r], out, &mut rand),
                _ => reverb.run(knobs, ctx, [&l, &r], out),
            }
            for byte in ol.iter().chain(&or).flat_map(|x| x.to_le_bytes()) {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            }
        }
        hash
    }

    #[test]
    fn the_effects_are_fl_s_to_the_bit() {
        // FL's own functions, run under an emulator on the same sends.
        assert_eq!(
            hash(0, [0.25, 0.6, 0.1, 0.8], false, 40),
            0xe251_93db_081e_58a4
        );
        assert_eq!(
            hash(0, [0.1, 0.7, 0.3, 0.6], true, 40),
            0x639f_5f82_cac2_4a62
        );
        assert_eq!(
            hash(1, [0.4, 0.5, 0.3, 0.4], false, 40),
            0x1987_963a_823d_e395
        );
        assert_eq!(
            hash(2, [0.6, 0.4, 0.7, 0.5], false, 40),
            0x3321_901e_4f38_f2a2
        );
        assert_eq!(
            hash(2, [1.0, 0.0, 0.0, 1.0], false, 20),
            0xabb7_f9e5_14fc_8713
        );
    }

    #[test]
    fn a_delay_repeats_what_it_hears_a_beat_fraction_later() {
        // A click into a delay of a quarter beat at 120 BPM, 5512.5
        // samples, comes back once the time has glided there.
        let ctx = Context {
            rate: 44_100.0,
            tempo: 120.0,
        };
        let mut delay = Delay::default();
        let mut rand = || 0;
        let silent = vec![0.0; 256];
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        // Let the time glide to the knob first. In floats it stops short,
        // where a step rounds to nothing, as FL's does.
        for _ in 0..400 {
            delay.run(
                [0.25, 0.0, 0.0, 1.0],
                false,
                ctx,
                [&silent, &silent],
                [&mut l, &mut r],
                &mut rand,
            );
        }
        assert!((delay.time - 5512.5).abs() < 1.0, "{}", delay.time);
        let mut click = vec![0.0; 256];
        click[0] = 1.0;
        let mut heard = Vec::new();
        for block in 0..30 {
            let send = if block == 0 { &click } else { &silent };
            l.fill(0.0);
            r.fill(0.0);
            delay.run(
                [0.25, 0.0, 0.0, 1.0],
                false,
                ctx,
                [send, send],
                [&mut l, &mut r],
                &mut rand,
            );
            heard.extend_from_slice(&l);
        }
        let loudest = (0..heard.len())
            .max_by(|&a, &b| heard[a].abs().total_cmp(&heard[b].abs()))
            .unwrap();
        assert!((5510..5516).contains(&loudest), "{loudest}");
    }
}
