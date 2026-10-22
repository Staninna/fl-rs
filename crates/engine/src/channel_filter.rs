//! The filter of a sampler or audio clip channel: what FL's host runs on
//! every voice it plays itself, with the channel's mod X as the cutoff and
//! its mod Y as the resonance. `docs/rec-channel.md` has where it was read
//! (`FLEngine_x64.dll`):
//!
//! - The type is 219 @20, the channel's control 5, which the settings
//!   form's `FTypeSelect` (tag 5) sets. The voice takes a kind and whether
//!   it runs twice from the table at 0x1510e40: Fast LP, the low-pass,
//!   band-pass, high-pass and notch biquads, the low-pass twice, and a
//!   state-variable low-pass once and twice (`0xd27963`).
//! - The cutoff is `FinalLevels.FCut` and the resonance `FinalLevels.FRes`
//!   (`FUN_00d24650`): the note's mods and the channel's, `(219 @12 + 229
//!   @12) / 256` and `(219 @16 + 229 @16) / 256` (`FUN_00fea140`), plus
//!   what the mod X and mod Y envelopes add, which the engine does not
//!   play.
//! - `FUN_00981df0`, `FUN_00981c50` and `FUN_00981f20` make the
//!   coefficients of the Fast LP, the biquads and the state-variable
//!   filter, the kernels at 0x6a5eb0, 0x982020 and 0x982140 run them, and
//!   `FUN_00d247a0` runs a second stage and puts the twice low-pass's gain
//!   on the voice. 3x Osc carries a copy of this unit, and the formulas are
//!   those of `docs/plugins/3x-osc.md`, section 3.3; the constants here are
//!   FLEngine's own.
//!
//! FL works the coefficients out once a tick and moves the Fast LP's from
//! where they were to where they are going over the tick, by no more than
//! `0.001 * 44100 / rate` a sample (`FUN_006a3810`). The engine works them
//! out where it reads its controls, every [`crate::mix::BLOCK`]th sample and
//! where a voice starts, and moves them over [`crate::mix::BLOCK`] samples.
//! FL's kernels zero what is left in their state under 2^-24 at the end
//! of each of its blocks (`0x6a5f7a`, `FUN_006a0460`); the engine does not,
//! so that a voice comes out the same however its blocks fall.

use crate::mix::BLOCK;

/// The filter's kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fast,
    /// The RBJ biquad of `FUN_009815f0`: 1 low-pass, 2 band-pass, 3
    /// high-pass, 4 notch.
    Biquad(u8),
    Svf,
}

/// The kind of each filter type and whether it runs twice (0x1510e40).
const TYPES: [(Kind, bool); 8] = [
    (Kind::Fast, false),
    (Kind::Biquad(1), false),
    (Kind::Biquad(2), false),
    (Kind::Biquad(3), false),
    (Kind::Biquad(4), false),
    (Kind::Biquad(1), true),
    (Kind::Svf, false),
    (Kind::Svf, true),
];

/// The coefficients of a Fast LP: how fast it follows and how much of its
/// speed it keeps (`FUN_00981df0`). `s` is `44100 / rate`.
fn fast_lp(cut: f32, res: f32, s: f32) -> (f32, f32) {
    let ln601 = 601f64.ln() as f32;
    let per = 1.0f32 / 600.0;
    let f = if cut <= 0.0 {
        0.0
    } else if cut < 1.0 {
        let grown = ((f64::from(cut * ln601).exp() - 1.0) * f64::from(per)) as f32;
        (grown * s).min(1.0)
    } else {
        1.0
    };
    let fb = if res <= 0.0 {
        0.0
    } else {
        let x = (1.0 - res.min(1.0) * 0.625) * ln601;
        1.0 - ((f64::from(x).exp() - 1.0) * f64::from(per)) as f32
    };
    (f, fb)
}

/// The angle a sample and the Q of a biquad (`FUN_00981c50`).
fn biquad_wq(cut: f32, res: f32, s: f32) -> (f32, f32) {
    let ln51 = 51f64.ln() as f32;
    let ln51_res = (51f64.ln() * 80.0 / 128.0) as f32;
    let w = ((f64::from(cut * ln51).exp() - 1.0) * f64::from(0.0596f32) + 0.02) * f64::from(s);
    let (w, floor) = (w as f32, (f64::from(s) * 0.02) as f32);
    let w = if w < floor { floor } else { w.min(3.0) };
    let q = if res > 0.0 {
        let grown = f64::from(res.min(1.0) * ln51_res).exp() - 1.0;
        (grown * f64::from(0.4f32) + 0.1) as f32
    } else {
        0.1
    };
    (w, q)
}

/// `[b0, b1, b2, a1, a2]` of biquad `kind` at angle `w` and Q `q`
/// (`FUN_009815f0`, kinds 1 to 4, with `alpha = sin w / 2q`).
fn biquad(kind: u8, w: f32, q: f32) -> [f32; 5] {
    let (sin, cos) = w.sin_cos();
    let alpha = f64::from(sin / (q * 2.0));
    let cos = f64::from(cos);
    let n = 1.0 / (alpha + 1.0);
    let a1 = (n * cos * -2.0) as f32;
    let a2 = (n * (1.0 - alpha)) as f32;
    let [b0, b1, b2] = match kind {
        1 => {
            let b1 = (n * (1.0 - cos)) as f32;
            [b1 * 0.5, b1, b1 * 0.5]
        }
        2 => [(n * alpha) as f32, 0.0, (n * -alpha) as f32],
        3 => {
            let b1 = (n * -(cos + 1.0)) as f32;
            [-b1 * 0.5, b1, -b1 * 0.5]
        }
        _ => [n as f32, a1, n as f32],
    };
    [b0, b1, b2, a1, a2]
}

/// The state-variable filter's coefficient and damping (`FUN_00981f20`).
fn svf(cut: f32, res: f32, s: f32) -> (f32, f32) {
    let f = if cut > 0.0 {
        let grown = ((f64::from(cut) * 26f64.ln()).exp() - 1.0) as f32;
        (grown * 0.04 * s).min(1.0)
    } else {
        0.0
    };
    let q = if res <= 0.0 {
        1.0
    } else if res < 1.0 {
        1.0 - res * 0.8
    } else {
        0.2
    };
    (f, q)
}

/// The filter of one voice. It allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChannelFilter {
    kind: Kind,
    twice: bool,
    /// `44100 / rate`.
    s: f32,
    /// A Fast LP does nothing until its coefficients first leave the wide
    /// open 1 and 0, and then it runs for good.
    active: bool,
    /// Whether the coefficients have been worked out yet: the first time,
    /// a Fast LP starts where they are.
    set: bool,
    /// The Fast LP's speed now, what a sample adds to it, the samples it
    /// goes on adding it for, the speed it is headed for, whether this
    /// stretch gets there, and what it keeps of its speed.
    f: f32,
    step: f32,
    left: usize,
    to: f32,
    reaches: bool,
    fb: f32,
    /// The Fast LP's level and speed, left and right.
    fast: [[f64; 2]; 2],
    coefs: [f32; 5],
    /// Two inputs and two outputs back, left and right, of each stage.
    biquads: [[[f32; 4]; 2]; 2],
    svf_f: f32,
    svf_q: [f32; 2],
    /// The low and the band of each stage, left and right.
    svfs: [[[f32; 2]; 2]; 2],
    /// What the voice's gains are multiplied by: the twice low-pass's
    /// make-up, 1 for every other.
    gain: f32,
}

impl ChannelFilter {
    /// The filter of 219 @20 `kind` at this output rate. `None` for a type
    /// past the eight FL has.
    pub fn new(kind: i32, rate: u32) -> Option<Self> {
        let (kind, twice) = *TYPES.get(usize::try_from(kind).ok()?)?;
        Some(ChannelFilter {
            kind,
            twice,
            s: (44_100.0 / f64::from(rate.max(1))) as f32,
            active: kind != Kind::Fast,
            set: false,
            f: 1.0,
            step: 0.0,
            left: 0,
            to: 1.0,
            reaches: true,
            fb: 0.0,
            fast: [[0.0; 2]; 2],
            coefs: [0.0; 5],
            biquads: [[[0.0; 4]; 2]; 2],
            svf_f: 0.0,
            svf_q: [1.0; 2],
            svfs: [[[0.0; 2]; 2]; 2],
            gain: 1.0,
        })
    }

    /// What the voice's gains are multiplied by.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Take a cutoff and a resonance, `FinalLevels.FCut` and `FRes`, from
    /// the next sample on (`FUN_00d24650`).
    pub fn set(&mut self, cut: f32, res: f32) {
        let first = !std::mem::replace(&mut self.set, true);
        match self.kind {
            Kind::Fast => {
                let (f, fb) = fast_lp(cut, res, self.s);
                self.fb = fb;
                if f != 1.0 || fb != 0.0 {
                    self.active = true;
                }
                self.to = f;
                if first || !self.active {
                    (self.f, self.step, self.left) = (f, 0.0, 0);
                } else {
                    self.ramp();
                }
            }
            Kind::Biquad(kind) => {
                let (w, q) = biquad_wq(cut, res, self.s);
                self.coefs = biquad(kind, w, q);
                if self.twice {
                    let make_up = 1.0 - f64::from(q) * (3.2 - f64::from(w)) * 0.046_875;
                    self.gain = make_up as f32;
                }
            }
            Kind::Svf => {
                let (f, q) = svf(cut, res, self.s);
                self.svf_f = f;
                self.svf_q = [q, q + (1.0 - q) * 0.25];
            }
        }
    }

    /// Head for [`ChannelFilter::to`] over the next [`BLOCK`] samples, by
    /// no more than `0.001 s` a sample (`FUN_006a3810`).
    fn ramp(&mut self) {
        let len = BLOCK as f32;
        let step = (self.to - self.f) / len;
        let limit = (f64::from(self.s) * 0.001) as f32;
        self.reaches = step.abs() <= limit;
        self.step = if self.reaches {
            step
        } else {
            limit.copysign(step)
        };
        self.left = if step == 0.0 { 0 } else { BLOCK };
    }

    /// One frame through.
    pub fn run(&mut self, frame: [f32; 2]) -> [f32; 2] {
        if !self.active {
            return frame;
        }
        let mut out = frame;
        match self.kind {
            Kind::Fast => {
                let (f, fb) = (f64::from(self.f), f64::from(self.fb));
                for (side, state) in self.fast.iter_mut().enumerate() {
                    let [level, speed] = state;
                    *speed += (f64::from(frame[side]) - *level) * f;
                    *level += *speed;
                    *speed *= fb;
                    out[side] = *level as f32;
                }
                if self.left > 0 {
                    self.f += self.step;
                    self.left -= 1;
                    // A stretch that gets there ends on the speed itself,
                    // and one that does not goes on towards it.
                    if self.left == 0 && self.reaches {
                        (self.f, self.step) = (self.to, 0.0);
                    } else if self.left == 0 {
                        self.ramp();
                    }
                }
            }
            Kind::Biquad(_) => {
                let [b0, b1, b2, a1, a2] = self.coefs;
                let stages = if self.twice { 2 } else { 1 };
                for stage in &mut self.biquads[..stages] {
                    for (side, state) in stage.iter_mut().enumerate() {
                        let [x1, x2, y1, y2] = *state;
                        let x = out[side];
                        let y = x * b0 + (((x2 * b2 + x1 * b1) - y1 * a1) - y2 * a2);
                        *state = [x, x1, y, y1];
                        out[side] = y;
                    }
                }
            }
            Kind::Svf => {
                let stages = if self.twice { 2 } else { 1 };
                for (stage, q) in self.svfs[..stages].iter_mut().zip(self.svf_q) {
                    for (side, state) in stage.iter_mut().enumerate() {
                        let [low, band] = *state;
                        let low = band * self.svf_f + low;
                        let band = (-(band * q + low) + out[side]) * self.svf_f + band;
                        *state = [low, band];
                        out[side] = low;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(filter: &mut ChannelFilter, input: &[f32]) -> Vec<f32> {
        input.iter().map(|x| filter.run([*x, *x])[0]).collect()
    }

    #[test]
    fn a_fast_lp_wide_open_passes_its_input_untouched() {
        let mut filter = ChannelFilter::new(0, 44_100).unwrap();
        filter.set(1.0, 0.0);
        let input = [1.0, -0.5, 0.25, 0.0, 0.75];
        assert_eq!(run(&mut filter, &input), input);
        // Past the top it is open as well.
        filter.set(1.5, -1.0);
        assert_eq!(run(&mut filter, &input), input);
    }

    #[test]
    fn the_fast_lp_follows_fl_s_curve() {
        // Half way, (601^0.5 - 1) / 600 at 44.1 kHz.
        let (f, fb) = fast_lp(0.5, 0.0, 1.0);
        assert!((f64::from(f) - (601f64.sqrt() - 1.0) / 600.0).abs() < 1e-6);
        assert_eq!(fb, 0.0);
        // At full resonance it keeps 1 - (601^0.375 - 1) / 600 of its speed.
        let (_, fb) = fast_lp(0.5, 1.0, 1.0);
        assert!((f64::from(fb) - (1.0 - (601f64.powf(0.375) - 1.0) / 600.0)).abs() < 1e-6);
        // Twice the rate halves the speed.
        assert!((fast_lp(0.5, 0.0, 0.5).0 - f * 0.5).abs() < 1e-7);
        assert_eq!(fast_lp(0.0, 0.0, 1.0).0, 0.0);
    }

    #[test]
    fn a_closing_fast_lp_ramps_by_no_more_than_a_thousandth_a_sample() {
        let mut filter = ChannelFilter::new(0, 44_100).unwrap();
        filter.set(1.0, 0.0);
        filter.set(0.0, 0.0);
        assert_eq!(filter.step, -0.001);
        let ones = [1.0; 4];
        run(&mut filter, &ones);
        assert!((filter.f - 0.996).abs() < 1e-6);
        // It goes on past the block until it gets there, and stops.
        run(&mut filter, &vec![1.0; 1200]);
        assert!(filter.f.abs() < 1e-4 && filter.f > -1e-4);
        assert_eq!((filter.step, filter.left), (0.0, 0));
        // A step that fits is taken over the block.
        let mut filter = ChannelFilter::new(0, 44_100).unwrap();
        filter.set(1.0, 0.0);
        filter.set(0.95, 0.0);
        let to = fast_lp(0.95, 0.0, 1.0).0;
        assert!((filter.step - (to - 1.0) / BLOCK as f32).abs() < 1e-9);
        run(&mut filter, &vec![1.0; BLOCK]);
        assert_eq!(filter.f, to);
    }

    #[test]
    fn the_low_pass_biquad_passes_the_low_end_and_the_twice_one_is_made_up() {
        let mut filter = ChannelFilter::new(1, 48_000).unwrap();
        filter.set(0.3, 0.0);
        // Its gain at 0 Hz is 1.
        let out = run(&mut filter, &[1.0; 4000]);
        assert!((out[3999] - 1.0).abs() < 1e-4);
        assert_eq!(filter.gain(), 1.0);
        let mut twice = ChannelFilter::new(5, 48_000).unwrap();
        twice.set(0.3, 0.5);
        let (w, q) = biquad_wq(0.3, 0.5, 44_100.0 / 48_000.0);
        let want = (1.0 - f64::from(q) * (3.2 - f64::from(w)) * 0.046_875) as f32;
        assert_eq!(twice.gain(), want);
        // The high-pass takes it all away.
        let mut high = ChannelFilter::new(3, 48_000).unwrap();
        high.set(0.3, 0.0);
        assert!(run(&mut high, &[1.0; 4000])[3999].abs() < 1e-4);
    }

    #[test]
    fn the_state_variable_low_pass_settles_on_its_input() {
        for kind in [6, 7] {
            let mut filter = ChannelFilter::new(kind, 44_100).unwrap();
            filter.set(0.5, 0.5);
            let out = run(&mut filter, &[0.5; 20_000]);
            assert!((out[19_999] - 0.5).abs() < 1e-4, "{kind}");
        }
        assert_eq!(svf(1.0, 2.0, 1.0), (1.0, 0.2));
        assert_eq!(svf(-1.0, 0.0, 1.0), (0.0, 1.0));
    }

    #[test]
    fn a_type_fl_has_not_is_none() {
        assert!(ChannelFilter::new(8, 44_100).is_none());
        assert!(ChannelFilter::new(-1, 44_100).is_none());
    }
}
