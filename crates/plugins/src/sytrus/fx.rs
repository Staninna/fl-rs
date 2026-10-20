//! Sytrus's post chain, as `Gen_Render` (`0x8737a0`) runs it after the
//! voices: the voices add into a dry and a send bus, and the effects work
//! the send bus into the dry one. Section 9 of `docs/plugins/sytrus.md`.
//!
//! In order, a block at a time:
//!
//! 1. EQ mode 2 or 3: the three-band EQ on the dry or the send bus.
//! 2. The chorus (`TStereoFlanger`) on the send bus, added to the dry one,
//!    and to the send bus too when a delay, the reverb or the FX send
//!    follows. It sleeps when the voices have been gone a while.
//! 3. The three delays (`TDelay_32FS`), each fed the send bus at its
//!    stereo offset and added to the dry bus, or to the send bus when it
//!    is serial and something follows.
//! 4. The reverb (`TFruityReverb`) on the sum of the send bus's sides,
//!    added to the dry bus at its wet level.
//! 5. EQ mode 1: the EQ on the dry bus.
//! 6. With 0x4f5 on, a two-pole high pass at about 33 Hz on the dry bus.
//!
//! The classes are `fl_fx`'s. FL runs the voices oversampled by `2^L` and
//! the effects at up to four times the rate when the oversampling select
//! (u8 0x438 while playing) is above 0; here both run at the rate.

use crate::fl_fx::{
    Delay32, DelayKernel, FlangerSettings, FruityReverb, ReverbSettings, StereoFlanger,
};
use crate::knob_gain;
use dsp::biquad::{Biquad64, Coefficients64};
use std::f64::consts::TAU;

use super::CHUNK;

/// What the state keeps for the effects that is not a REC parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Saved {
    /// Each delay unit's tempo (+0x14), serial (+0x15) and on (+0x16)
    /// bytes.
    pub(super) delay_flags: [[u8; 3]; 3],
    /// The reverb's nine ints at 0x5fec: low cut, high cut, predelay,
    /// room, diffusion, decay, high damping, colour, volume; its switch
    /// (0x6010) and whether its predelay counts in beats (0x6011).
    pub(super) reverb: [i32; 9],
    pub(super) reverb_on: bool,
    pub(super) reverb_tempo: bool,
    /// The EQ's mode (0x61cc) and its bands' types (+0xc).
    pub(super) eq_mode: i32,
    pub(super) eq_types: [i32; 3],
    /// u8 0x4f5: the high pass on the output.
    pub(super) high_pass: bool,
    /// The FX send's mixer track (0x420), -1 for none.
    pub(super) send_target: i32,
}

impl Default for Saved {
    /// What the constructor (`0x865c10`) and `0x865a90` leave.
    fn default() -> Self {
        Saved {
            delay_flags: [[1, 0, 0]; 3],
            reverb: [75, 40, 0, 40, 100, 15, 40, 2, 25],
            reverb_on: false,
            reverb_tempo: false,
            eq_mode: 0,
            eq_types: [0, 1, 2],
            high_pass: false,
            send_target: -1,
        }
    }
}

/// The REC parameters the effects read: the main part's nine wheels
/// (547..555: send pan, chorus order, depth, speed, delay, spread, cross,
/// volume, FX send), the delays' fifteen (556..570) and the EQ's nine
/// (587..595: the levels, the frequencies and the widths of the three
/// bands).
pub(super) struct Wheels<'a> {
    pub(super) main: &'a [i32],
    pub(super) delays: &'a [i32],
    pub(super) eq: &'a [i32],
}

/// The slowest tempo a delay or the reverb's predelay has room for.
const SLOWEST_TEMPO: f64 = 20.0;

/// The level curve with its sign, 1 at 1 exactly (`0x869540`).
fn signed_level(x: f32) -> f32 {
    let size = x.abs();
    let level = if size == 1.0 {
        1.0
    } else {
        knob_gain(f64::from(size)) as f32
    };
    if x == 0.0 { 0.0 } else { level.copysign(x) }
}

/// One delay unit (`0x5f30 + 0x38 e`).
#[derive(Debug, Clone, PartialEq, Default)]
struct Unit {
    on: bool,
    delay: Delay32,
    kernel: Option<DelayKernel>,
    /// The frames the left and the right input go in past the write
    /// position (+0x20, +0x24).
    offsets: [usize; 2],
    /// Its volume (+0x28).
    gain: f32,
    /// It feeds the send bus and not the dry one (+0x30).
    serial: bool,
}

/// One EQ band (`0x6018 + 0x90 b`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    on: bool,
    coefficients: Coefficients64,
    sides: [Biquad64; 2],
}

impl Default for Band {
    fn default() -> Self {
        Band {
            on: false,
            coefficients: Coefficients64::IDENTITY,
            sides: [Biquad64::default(); 2],
        }
    }
}

impl Band {
    /// Design the band as `0x86a440` and `0x625990` do: the frequency
    /// wheel `101^(f / 65536) - 1` times 159.9 plus 10 Hz, the level in
    /// hundredths of a dB, the width over 65536; type 6 a low shelf, 7 a
    /// peak and 8 a high shelf (`0x6253c0`).
    fn design(&mut self, kind: i32, [level, freq, width]: [i32; 3], rate: f64) {
        let curve = ((f64::from(freq) * (1.0 / 65_536.0)) as f32, 101f64.ln());
        let grown = (f64::from(curve.0) * curve.1).exp();
        let mut hz = ((grown - 1.0) as f32 as f64 * 159.9 + 10.0) as f32;
        let width = (f64::from(width) * (1.0 / 65_536.0)) as f32;
        let db = level as f32 * 0.01;
        let most = (rate * 0.4999) as f32;
        if hz > most {
            hz = most;
        }
        let (sin, cos) = (f64::from(hz) * (TAU / rate)).sin_cos();
        let a = 10f64.powf(f64::from(db) * 0.025);
        let was = self.on;
        self.on = level != 0 && (6..=8).contains(&kind);
        if !self.on {
            return;
        }
        self.coefficients = if kind == 7 {
            let q = ((f64::from(width) * 1001f64.ln()).exp() - 1.0) as f32;
            let alpha = sin / ((f64::from(q) * 0.0999 + 0.1) * 2.0);
            Coefficients64::peak(cos, alpha, a)
        } else {
            let slope = f64::from(width) * 1.9 + 0.1;
            let beta = ((a * a + 1.0) / slope - (a - 1.0) * (a - 1.0)).sqrt();
            if kind == 6 {
                Coefficients64::low_shelf(cos, beta * sin, a)
            } else {
                Coefficients64::high_shelf(cos, beta * sin, a)
            }
        };
        if !was {
            self.sides = [Biquad64::default(); 2];
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.on {
            self.sides[0].process_block(&self.coefficients, left);
            self.sides[1].process_block(&self.coefficients, right);
        }
    }
}

/// The post chain.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Effects {
    rate: f64,
    tempo: f64,
    chorus: StereoFlanger,
    chorus_on: bool,
    /// The chorus also feeds the send bus (u8 0x5f28).
    chorus_into_send: bool,
    units: [Unit; 3],
    reverb: FruityReverb,
    reverb_on: bool,
    reverb_wet: f32,
    eq_mode: i32,
    bands: [Band; 3],
    high_pass: bool,
    /// The high pass's low and speed a side, its step and its keep.
    pass: [[f64; 2]; 2],
    pass_step: f32,
    /// How long the effects keep taking the send bus after the last voice
    /// (0x6308, 0x630c), in frames.
    countdown: i32,
    hold: i32,
    /// The dry and the send bus, a scratch pair and the reverb's mono
    /// input, `CHUNK` frames each.
    buses: Vec<f32>,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            rate: 44_100.0,
            tempo: 120.0,
            chorus: StereoFlanger::default(),
            chorus_on: false,
            chorus_into_send: false,
            units: Default::default(),
            reverb: FruityReverb::new(44_100, 1),
            reverb_on: false,
            reverb_wet: 0.0,
            eq_mode: 0,
            bands: [Band::default(); 3],
            high_pass: false,
            pass: [[0.0; 2]; 2],
            pass_step: 0.0,
            countdown: 0,
            hold: 0,
            buses: vec![0.0; 7 * CHUNK],
        }
    }
}

impl Effects {
    /// Make the memory the state's effects need at this rate and start
    /// them fresh. The delays have room for their longest time down to 20
    /// BPM and the reverb for its longest predelay. This allocates.
    pub(super) fn build(&mut self, saved: &Saved, wheels: &Wheels<'_>, rate: f32, tempo: f64) {
        self.rate = f64::from(rate);
        let hz = rate.round() as i32;
        self.chorus.make_room(self.rate);
        self.chorus.fresh();
        self.chorus_on = false;
        let longest = (384.0 * 0.3125 / SLOWEST_TEMPO * self.rate).max(self.rate) as usize + 1;
        let offset = (0.5 * self.rate) as usize + 2;
        for (unit, flags) in self.units.iter_mut().zip(&saved.delay_flags) {
            let room = if flags[2] != 0 {
                CHUNK + offset + longest
            } else {
                0
            };
            if unit.delay.capacity() != room {
                unit.delay = Delay32::with_capacity(room);
            }
            unit.delay.fresh();
            unit.on = false;
        }
        let predelay = if saved.reverb_on { longest } else { 1 };
        if self.reverb.rate() != hz || saved.reverb_on {
            self.reverb = FruityReverb::new(hz, predelay);
        }
        self.reverb.fresh();
        self.reverb_on = false;
        self.bands = [Band::default(); 3];
        self.pass = [[0.0; 2]; 2];
        self.countdown = 0;
        self.configure(saved, wheels, tempo);
    }

    /// Take the state's settings, as the setters do after a load
    /// (`0x862c50`): the delays (`0x869540`), the reverb (`0x869840`), the
    /// chorus (`0x869a80`) and the EQ (`0x86a5b0`). This allocates
    /// nothing, so a host can move a wheel or the tempo on the audio
    /// thread.
    pub(super) fn configure(&mut self, saved: &Saved, wheels: &Wheels<'_>, tempo: f64) {
        self.tempo = tempo;
        let rate = self.rate;
        let tempo = f64::from(tempo as f32);
        // The delays.
        let mut last = -1;
        for (e, unit) in self.units.iter_mut().enumerate() {
            let [feedback, time, offset, volume, mode] =
                std::array::from_fn(|j| wheels.delays[5 * e + j]);
            let [beats, serial, on] = saved.delay_flags[e];
            let was = unit.on;
            unit.on = on != 0 && unit.delay.capacity() > 0;
            if !unit.on {
                continue;
            }
            if !was {
                unit.delay.fresh();
            }
            last = e as i32;
            let size = (f64::from(offset.unsigned_abs() as i32) * (1.0 / 512.0)) as f32;
            let size = ((f64::from(size) * 1001f64.ln()).exp() - 1.0) as f32;
            let size = (f64::from(size) * 50.0) as f32;
            let frames = (f64::from(size) * rate / 100_000.0).round_ties_even() as usize;
            let length = if beats != 0 {
                f64::from(time) * rate * 0.3125 / tempo
            } else {
                f64::from(time) * rate / 384.0
            };
            let length = length.round_ties_even().max(0.0) as usize;
            unit.delay.set_length(CHUNK + frames, length);
            unit.offsets = if offset > 0 { [0, frames] } else { [frames, 0] };
            unit.delay.set_feedback(feedback as f32 * (1.0 / 128.0));
            unit.gain = signed_level(volume as f32 * (1.0 / 128.0));
            unit.kernel = Some(match mode {
                0 => DelayKernel::Straight,
                1 => DelayKernel::Swapped,
                _ => DelayKernel::PingPong,
            });
            unit.serial = serial != 0;
        }
        // The reverb.
        let was = self.reverb_on;
        self.reverb_on = saved.reverb_on;
        if self.reverb_on {
            if !was {
                self.reverb.fresh();
            }
            let [
                low,
                high,
                predelay,
                room,
                diffusion,
                decay,
                damping,
                colour,
                volume,
            ] = saved.reverb;
            let predelay = if saved.reverb_tempo {
                f64::from(predelay) * rate * 0.3125 / tempo
            } else {
                f64::from(predelay) * rate / 384.0
            };
            self.reverb.set(ReverbSettings {
                low_cut: if low == 19 { 0.0 } else { low as f32 },
                high_cut: if high == 221 { 0.0 } else { high as f32 },
                predelay: predelay.round_ties_even() as i32,
                room,
                diffusion,
                decay: (f64::from(decay) * rate / 10.0) as f32,
                damping: if damping == 221 { 0.0 } else { damping as f32 },
                colour,
            });
            self.reverb_wet = volume as f32 * (1.0 / 256.0);
        }
        // Which delays feed the next (`0x869480`).
        let end = if self.reverb_on { 3 } else { last };
        for (e, unit) in self.units.iter_mut().enumerate() {
            unit.serial = unit.serial && (e as i32) < end;
        }
        self.chorus_into_send = last >= 0 || self.reverb_on || saved.send_target != -1;
        // The chorus.
        let main = wheels.main;
        self.hold = (rate.round() as i32) * 100 / 44_100;
        let was = self.chorus_on;
        self.chorus_on = main[1] >= 1;
        if self.chorus_on {
            if !was {
                self.chorus.fresh();
            }
            let settings = FlangerSettings::from_wheels(
                [main[1], main[2], main[3], main[4], main[5], main[6]],
                main[7] as f32 * (1.0 / 128.0),
            );
            self.chorus.set(&settings, rate);
            self.hold += CHUNK as i32 + self.chorus.length();
        }
        // The EQ, its types 6, 7 and 8 from the selects' 0, 1 and 2.
        self.eq_mode = saved.eq_mode;
        for (b, band) in self.bands.iter_mut().enumerate() {
            let fields = [wheels.eq[b], wheels.eq[3 + b], wheels.eq[6 + b]];
            band.design(saved.eq_types[b] + 6, fields, rate);
        }
        // The high pass: `1.0070235 / rate` and 0.96 (`0x8629d0`).
        self.high_pass = saved.high_pass;
        self.pass_step = (1.007_023_5 / rate) as f32;
    }

    /// The four buses the voices add into for the next `len` frames: dry
    /// left, dry right, send left, send right, emptied.
    pub(super) fn buses(&mut self, len: usize) -> [&mut [f32]; 4] {
        let (dry, rest) = self.buses.split_at_mut(2 * CHUNK);
        let (send, _) = rest.split_at_mut(2 * CHUNK);
        let (dl, dr) = dry.split_at_mut(CHUNK);
        let (sl, sr) = send.split_at_mut(CHUNK);
        [
            &mut dl[..len],
            &mut dr[..len],
            &mut sl[..len],
            &mut sr[..len],
        ]
        .map(|bus| {
            bus.fill(0.0);
            bus
        })
    }

    /// Run the chain over the `len` frames the voices put in the buses
    /// and add the dry bus to the output. `voices` is how many sound and
    /// `send` whether they use the send bus (u8 0x632c).
    pub(super) fn run(&mut self, voices: usize, send: bool, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len()).min(CHUNK);
        if !send {
            self.countdown = 0;
        } else if voices == 0 {
            if self.countdown > 0 {
                self.countdown -= len as i32;
            }
        } else {
            self.countdown = self.hold;
        }
        let (dry, rest) = self.buses.split_at_mut(2 * CHUNK);
        let (wet, rest) = rest.split_at_mut(2 * CHUNK);
        let (tmp, mono) = rest.split_at_mut(2 * CHUNK);
        let (dl, dr) = dry.split_at_mut(CHUNK);
        let (wl, wr) = wet.split_at_mut(CHUNK);
        let (tl, tr) = tmp.split_at_mut(CHUNK);
        let (dl, dr, wl, wr) = (
            &mut dl[..len],
            &mut dr[..len],
            &mut wl[..len],
            &mut wr[..len],
        );
        let (tl, tr, mono) = (&mut tl[..len], &mut tr[..len], &mut mono[..len]);

        if self.eq_mode > 1 {
            let (l, r) = if self.eq_mode == 3 {
                (&mut *wl, &mut *wr)
            } else {
                (&mut *dl, &mut *dr)
            };
            for band in &mut self.bands {
                band.run(l, r);
            }
        }
        if self.chorus_on {
            if self.countdown < 1 {
                self.chorus.skip(len);
            } else {
                for i in 0..len {
                    (tl[i], tr[i]) = self.chorus.frame(wl[i], wr[i]);
                }
                add(dl, tl, 1.0);
                add(dr, tr, 1.0);
                if self.chorus_into_send {
                    add(wl, tl, 1.0);
                    add(wr, tr, 1.0);
                }
            }
        }
        let cap = (44_100.0 / self.rate * 0.001) as f32;
        let mut gate = self.countdown;
        for unit in &mut self.units {
            let (true, Some(kernel)) = (unit.on, unit.kernel) else {
                continue;
            };
            if gate > 0 {
                unit.delay.add(0, wl, unit.offsets[0]);
                unit.delay.add(1, wr, unit.offsets[1]);
            }
            unit.delay.render(kernel, tl, tr, cap);
            let (l, r) = if unit.serial {
                gate = self.hold;
                (&mut *wl, &mut *wr)
            } else {
                (&mut *dl, &mut *dr)
            };
            add(l, tl, unit.gain);
            add(r, tr, unit.gain);
        }
        if self.reverb_on {
            for i in 0..len {
                mono[i] = wl[i] + wr[i];
            }
            self.reverb.process(mono, wl, wr);
            add(dl, wl, self.reverb_wet);
            add(dr, wr, self.reverb_wet);
        }
        if self.eq_mode == 1 {
            for band in &mut self.bands {
                band.run(dl, dr);
            }
        }
        if self.high_pass {
            let (g, k) = (f64::from(self.pass_step), f64::from(0.96f32));
            for (side, bus) in [&mut *dl, &mut *dr].into_iter().enumerate() {
                let [mut low, mut speed] = self.pass[side];
                for x in bus.iter_mut() {
                    let input = f64::from(*x);
                    speed += (input - low) * g;
                    low += speed;
                    speed *= k;
                    *x = (input - low) as f32;
                }
                // Under 2^-24 is 0 (`0x62e190`).
                let tiny = |v: f64| {
                    if v.abs() < 5.960_464_477_539_063e-8 {
                        0.0
                    } else {
                        v
                    }
                };
                self.pass[side] = [tiny(low), tiny(speed)];
            }
        }
        add(&mut left[..len], dl, 1.0);
        add(&mut right[..len], dr, 1.0);
    }

    /// Forget what is in the rings and the filters, and take the
    /// settings again. This allocates nothing.
    pub(super) fn reset(&mut self, saved: &Saved, wheels: &Wheels<'_>) {
        self.chorus_on = false;
        for unit in &mut self.units {
            unit.on = false;
        }
        self.reverb_on = false;
        self.bands = [Band::default(); 3];
        self.pass = [[0.0; 2]; 2];
        self.countdown = 0;
        self.configure(saved, wheels, self.tempo);
    }

    /// How long the effects sound on after the voices, in frames.
    pub(super) fn tail(&self) -> usize {
        let mut tail = 0.0f64;
        if self.chorus_on {
            tail += f64::from(self.chorus.length()) + CHUNK as f64;
        }
        for unit in self.units.iter().filter(|u| u.on) {
            let feedback = f64::from(unit.delay.feedback().abs()).min(0.999);
            // Down 60 dB.
            let repeats = if feedback > 0.0 {
                (1e-3f64.ln() / feedback.ln()).max(1.0)
            } else {
                1.0
            };
            tail += (unit.delay.delay() as f64 + CHUNK as f64) * (repeats + 1.0);
        }
        if self.reverb_on {
            tail += f64::from(self.reverb.settings().decay) * 1.5
                + f64::from(self.reverb.settings().predelay);
        }
        tail.min(60.0 * self.rate) as usize
    }
}

/// `to += from * gain`, or the plain sum at a gain of 1.
fn add(to: &mut [f32], from: &[f32], gain: f32) {
    if gain == 1.0 {
        for (to, from) in to.iter_mut().zip(from) {
            *to += *from;
        }
    } else {
        for (to, from) in to.iter_mut().zip(from) {
            *to += *from * gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{partial, sine};

    const RATE: f32 = 48_000.0;

    /// The default wheels: chorus order 0, the delays as the constructor
    /// leaves them, the EQ flat.
    struct Setup {
        main: [i32; 9],
        delays: [i32; 15],
        eq: [i32; 9],
    }

    impl Setup {
        fn new() -> Self {
            Setup {
                main: [0, 0, 512, 768, 8, 128, -32, 128, 128],
                delays: [64, 144, 0, 64, 0, 64, 144, 0, 64, 0, 64, 144, 0, 64, 0],
                eq: [0, 0, 0, 5777, 33145, 55825, 17500, 17500, 17500],
            }
        }

        fn wheels(&self) -> Wheels<'_> {
            Wheels {
                main: &self.main,
                delays: &self.delays,
                eq: &self.eq,
            }
        }
    }

    /// Run `send` (both sides) and `dry` through the chain in blocks of
    /// `CHUNK`, with one voice sounding, and return the left output.
    fn run(fx: &mut Effects, dry: &[f32], send: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; send.len()];
        let mut right = vec![0.0; send.len()];
        for (k, (l, r)) in out
            .chunks_mut(CHUNK)
            .zip(right.chunks_mut(CHUNK))
            .enumerate()
        {
            let len = l.len();
            let buses = fx.buses(len);
            let at = k * CHUNK;
            buses[0].copy_from_slice(&dry[at..at + len]);
            buses[1].copy_from_slice(&dry[at..at + len]);
            buses[2].copy_from_slice(&send[at..at + len]);
            buses[3].copy_from_slice(&send[at..at + len]);
            fx.run(1, true, l, r);
        }
        out
    }

    #[test]
    fn the_send_bus_is_heard_only_through_the_effects() {
        let setup = Setup::new();
        let mut fx = Effects::default();
        fx.build(&Saved::default(), &setup.wheels(), RATE, 120.0);
        let tone = sine(0.01, 4096);
        // No effect on: the send goes nowhere and the dry bus passes.
        assert!(run(&mut fx, &[0.0; 4096], &tone).iter().all(|v| *v == 0.0));
        assert_eq!(run(&mut fx, &tone, &[0.0; 4096]), tone);
    }

    #[test]
    fn a_delay_echoes_the_send_at_its_time_and_offset_and_feedback() {
        let mut setup = Setup::new();
        // Unit 1 on, in beats: 96 is half a beat, 0.25 s at 120 BPM;
        // feedback 64 is a half, volume 128 is 1; no offset.
        setup.delays[..5].copy_from_slice(&[64, 96, 0, 128, 0]);
        let mut saved = Saved::default();
        saved.delay_flags[0] = [1, 0, 1];
        let mut fx = Effects::default();
        fx.build(&saved, &setup.wheels(), RATE, 120.0);
        let mut send = vec![0.0; 48_000];
        send[0] = 1.0;
        let out = run(&mut fx, &[0.0; 48_000], &send);
        let first = out.iter().position(|v| *v != 0.0).unwrap();
        assert_eq!(first, 12_000);
        assert_eq!(out[12_000], 1.0);
        // The next repeat at half, the feedback reached at once as a new
        // ring takes it.
        assert!((out[24_000] - 0.5).abs() < 1e-6, "{}", out[24_000]);
        assert!(out[12_001..24_000].iter().all(|v| *v == 0.0));

        // At 60 BPM the same half beat is twice as long.
        fx.configure(&saved, &setup.wheels(), 60.0);
        fx.reset(&saved, &setup.wheels());
        let out = run(&mut fx, &[0.0; 48_000], &send);
        assert_eq!(out.iter().position(|v| *v != 0.0), Some(24_000));

        // A positive offset of 512 holds the right side back half a
        // second more: the left starts on time.
        setup.delays[2] = 512;
        fx.configure(&saved, &setup.wheels(), 120.0);
        assert_eq!(fx.units[0].offsets, [0, 24_000]);
    }

    #[test]
    fn the_reverb_and_the_chorus_take_the_send_and_the_eq_shapes_the_output() {
        let mut setup = Setup::new();
        let mut saved = Saved {
            reverb_on: true,
            ..Saved::default()
        };
        let mut fx = Effects::default();
        fx.build(&saved, &setup.wheels(), RATE, 120.0);
        let mut send = vec![0.0; 24_000];
        send[0] = 1.0;
        let out = run(&mut fx, &[0.0; 24_000], &send);
        // A tail that starts after the predelay of 0 and lasts.
        assert!(out[1000..].iter().any(|v| v.abs() > 1e-4));
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1.0));

        // The chorus, order 4: a tone on the send comes out around its
        // level, wet only.
        saved.reverb_on = false;
        setup.main[1] = 4;
        fx.build(&saved, &setup.wheels(), RATE, 120.0);
        let turns = 440.0 / f64::from(RATE);
        let tone = sine(turns, 24_000);
        let out = run(&mut fx, &[0.0; 24_000], &tone);
        let level = partial(&out[12_000..], turns);
        assert!(level > 0.2 && level < 2.0, "{level}");

        // EQ mode 1, band 2 a peak of +12 dB at 1 kHz: four times the
        // amplitude there and none of it two decades down.
        setup.main[1] = 0;
        saved.eq_mode = 1;
        let hz = 1000.0f64;
        setup.eq[1] = 1200;
        setup.eq[4] = (65_536.0 * ((hz - 10.0) / 159.9 + 1.0).ln() / 101f64.ln()).round() as i32;
        fx.build(&saved, &setup.wheels(), RATE, 120.0);
        let gain = |fx: &mut Effects, at: f64| {
            let tone = sine(at / f64::from(RATE), 48_000);
            let out = run(fx, &tone, &[0.0; 48_000]);
            partial(&out[24_000..], at / f64::from(RATE))
                / partial(&tone[24_000..], at / f64::from(RATE))
        };
        assert!(
            (gain(&mut fx, 1000.0) - 3.981).abs() < 0.05,
            "{}",
            gain(&mut fx, 1000.0)
        );
        assert!((gain(&mut fx, 10.0) - 1.0).abs() < 0.01);
        // The low shelf of band 1 at +6 dB and 1 kHz lifts the lows by two.
        setup.eq[1] = 0;
        setup.eq[0] = 600;
        setup.eq[3] = setup.eq[4];
        fx.configure(&saved, &setup.wheels(), 120.0);
        assert!(
            (gain(&mut fx, 20.0) - 1.995).abs() < 0.05,
            "{}",
            gain(&mut fx, 20.0)
        );
    }

    #[test]
    fn the_high_pass_takes_out_what_does_not_move() {
        let setup = Setup::new();
        let saved = Saved {
            high_pass: true,
            ..Saved::default()
        };
        let mut fx = Effects::default();
        fx.build(&saved, &setup.wheels(), RATE, 120.0);
        let out = run(&mut fx, &[1.0; 96_000], &[0.0; 96_000]);
        assert!(out[90_000..].iter().all(|v| v.abs() < 1e-3));
        // 1 kHz goes through.
        let turns = 1000.0 / f64::from(RATE);
        let tone = sine(turns, 48_000);
        let out = run(&mut fx, &tone, &[0.0; 48_000]);
        assert!((partial(&out[24_000..], turns) - 1.0).abs() < 0.01);
    }
}
