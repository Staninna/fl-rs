//! A sampler or audio clip channel's time stretch: its time, pitch,
//! multiplier and mode (215 @96..@111, controls 14 and 15), as FL's
//! sampler plays them (`docs/rec-channel.md`, "Time stretch").
//!
//! FL plays a stretched sample one of three ways, by the mode
//! (`chan+0x2f0`):
//!
//! - Stretch and Stretch Pro (-1, -2) stretch as the voice plays, through
//!   élastique (`FUN_00d26890`, `FUN_0067db90`): the note's pitch and the
//!   stretch pitch shift the sound and leave its length alone, and the
//!   length is the sample's own times the multiplier, or the time.
//! - Resample (0) changes the speed: the multiplier slows the sample down,
//!   a time plays the whole sample in that time whatever the key, and the
//!   stretch pitch shifts the sample once at load and keeps its length
//!   (`FUN_00ff0640`).
//! - The other modes (e3, e2, slice stretch, slice map and auto) stretch
//!   the sample once at load (`FUN_00ff0640` into `FUN_00a97410`) to the
//!   time, or its own length times the multiplier, shifted by the stretch
//!   pitch, and the voice then plays that at the note's speed; with a time
//!   it plays it to fit the time whatever the key.
//!
//! The lengths, speeds and pitches here are FL's. The stretch itself is
//! not: FL runs zplane's élastique (`Shared/elastique_x64.dll`) and its own
//! slicer, and this runs a WSOLA (overlap-add of windowed frames, each one
//! placed where it matches the one before best) and reads the result at
//! the speed that puts the pitch where FL does. A stretch that changes the
//! length by under [`NEAR`] and the pitch not at all is played by speed
//! alone, as Resample would: an audio clip stretched to its own length at
//! the project's tempo is that. Stretch Pro's formant shift is not played.
//!
//! All of it happens while the graph is built: a voice plays a sample that
//! is stretched already.

use crate::sample::Sample;
use model::{Channel, ChannelKind, Project};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Stretch Pro, élastique Pro with formant shifting.
pub(crate) const PRO: i32 = -2;
/// Resample.
pub(crate) const RESAMPLE: i32 = 0;

/// A length change under this share, with no pitch shift, is played as a
/// change of speed: at most 3.5 cents off FL's pitch.
pub(crate) const NEAR: f64 = 0.002;

/// A channel's time stretch as FL's loader leaves it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stretch {
    /// `chan+0x2f0`: -2 Stretch Pro, -1 Stretch, 0 Resample, 1 e3 generic,
    /// 2 e3 mono, 3 slice stretch, 4 slice map, 5 auto, 6 e2 generic, 7 e2
    /// transient, 8 e2 mono, 9 e2 speech (the order of the mode names in
    /// the DLL, and of the loader's remap below).
    pub mode: i32,
    /// `chan+0x2e4`, in 48ths of a step: `time * (PPQ / 4) / 48` ticks.
    /// 0 for none.
    pub time: f64,
    /// `chan+0x2e8`, cents.
    pub pitch: i32,
    /// `chan+0x6f4`: `2^(chan+0x2ec / 10000)` (`FUN_01186110`), 1 for 0.
    pub multiplier: f64,
}

/// The stretch mode a writer of `version` stored as `mode`, as the loader
/// remaps it (`FUN_00f46490`, after the 215 branch): below 6.1.5 the modes
/// above 2 move up 4 and 1 and 2 move up 2; below 11.5.13 they go through
/// the table at `0x14e01fb` (a mode below 0 reads as 0, above 9 as 9), and
/// a 5 out of it is 6.
pub(crate) fn remap(mode: i32, version: Option<u32>) -> i32 {
    let Some(version) = version else {
        return mode;
    };
    let mut mode = mode;
    if version < 6_001_005 {
        if mode > 2 {
            mode += 4;
        } else if mode > 0 {
            mode += 2;
        }
    }
    if version < 11_005_013 {
        const TABLE: [i32; 10] = [0, 6, 6, 7, 7, 8, 9, 3, 4, 5];
        mode = TABLE[mode.clamp(0, 9) as usize];
        if mode == 5 {
            mode = 6;
        }
    }
    mode
}

/// The stretch time a 215 stores. Newer writers set 169 bit 0 and store an
/// f32, older ones an integer the loader converts (`FUN_00f46490`). The
/// engine does not keep 169, so this tells them apart by the bits: the
/// knob goes to 36864, and an integer up to 2^23 is a float of under
/// 2^-125, which no knob gives. Below 0, or not a number, is no time.
fn time(stored: [u8; 4]) -> f64 {
    let bits = u32::from_le_bytes(stored);
    let time = if bits < 1 << 23 {
        f64::from(bits)
    } else {
        f64::from(f32::from_le_bytes(stored))
    };
    if time > 0.0 { time } else { 0.0 }
}

impl Stretch {
    /// The channel's stretch, `None` where it plays its sample as a channel
    /// with no stretch does: on a channel that is not a sampler or an
    /// audio clip channel, and in Resample or a load-time mode with no time,
    /// pitch or multiplier (`FUN_00fea6e0` says no stretch, and
    /// `FUN_00d26890` sets the speed alone).
    pub fn of(project: &Project, channel: &Channel) -> Option<Self> {
        if !matches!(
            channel.kind,
            Some(ChannelKind::Sampler | ChannelKind::AudioClip)
        ) {
            return None;
        }
        let p = channel.parameters?;
        let version = (project.writer_version.as_deref()).and_then(flp::event::version_number);
        let stretch = Stretch {
            mode: remap(p.stretch_mode, version),
            time: time(p.stretch_time),
            pitch: p.stretch_pitch,
            multiplier: (f64::from(p.stretch_multiplier) / 10_000.0).exp2(),
        };
        let any = stretch.time > 0.0 || p.stretch_pitch != 0 || p.stretch_multiplier != 0;
        (stretch.mode < 0 || any).then_some(stretch)
    }

    /// How a voice plays the sample: `len` frames at `ratio`, the sample's
    /// rate over the output rate, starting where a tick lasts `per_tick`
    /// output samples in a song of `ppq`, at `cents` from where the sample
    /// plays at its own speed (the note's `FinalLevels.Pitch` less the
    /// root, or a clip's pitch and the channel's).
    pub fn plan(&self, len: usize, ratio: f64, per_tick: f64, ppq: u16, cents: f64) -> Plan {
        let len = len.max(1) as f64;
        let m = self.multiplier;
        // The time in output samples: `FUN_00d26890` and `FUN_00ff0640`
        // take PPQ / 4 as an integer.
        let ticks = self.time * f64::from(ppq / 4) / 48.0;
        let fit = (self.time > 0.0).then_some(ticks * per_tick * m);
        let shift = |c: f64| (c / 1200.0).exp2();
        match (self.mode, fit) {
            // élastique as the voice plays: the speed of the sample, the
            // pitch of the note and the stretch.
            (..0, Some(out)) => Plan {
                factor: out * ratio / len,
                cents: cents + f64::from(self.pitch),
                step: ratio,
            },
            (..0, None) => Plan {
                factor: m,
                cents: cents + f64::from(self.pitch),
                step: ratio,
            },
            // Resample: the stretch pitch at load, the rest by speed.
            (RESAMPLE, Some(out)) => Plan {
                factor: 1.0,
                cents: f64::from(self.pitch),
                step: len / out,
            },
            (RESAMPLE, None) => Plan {
                factor: 1.0,
                cents: f64::from(self.pitch),
                step: ratio / m * shift(cents),
            },
            // Stretched at load to the time, played to fit it.
            (_, Some(out)) => {
                let frames = (out * ratio).round().max(1.0);
                Plan {
                    factor: frames / len,
                    cents: f64::from(self.pitch),
                    step: frames / out,
                }
            }
            (_, None) => Plan {
                factor: m,
                cents: f64::from(self.pitch),
                step: ratio * shift(cents),
            },
        }
    }
}

/// What FL makes of a sample for one voice: the sample stretched to
/// `factor` times its length and shifted by `cents`, read at `step` of its
/// frames an output sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Plan {
    pub factor: f64,
    pub cents: f64,
    pub step: f64,
}

/// The samples the graph's voices play, stretched once for every sample,
/// length and pitch they ask for.
#[derive(Default)]
pub(crate) struct Stretcher {
    done: BTreeMap<(usize, u64), Arc<Sample>>,
}

/// A sample a voice plays in place of its channel's, the speed it plays at,
/// and how many of its frames a frame of the channel's sample makes, for a
/// start offset.
pub(crate) struct Played {
    pub sample: Arc<Sample>,
    pub step: f64,
    pub scale: f64,
}

impl Stretcher {
    /// What a voice on `channel` plays of `sample` when the channel has a
    /// stretch: [`Stretch::plan`] at output `rate`, where a tick lasts
    /// `per_tick` output samples, `cents` from the sample's own speed.
    /// `None` for a channel that plays its sample as it is.
    pub fn voice(
        &mut self,
        project: &Project,
        channel: &Channel,
        sample: &Arc<Sample>,
        rate: u32,
        per_tick: f64,
        cents: f64,
    ) -> Option<Played> {
        let stretch = Stretch::of(project, channel)?;
        let ratio = f64::from(sample.rate) / f64::from(rate);
        let ppq = project.header.ppq;
        let plan = stretch.plan(sample.left.len(), ratio, per_tick, ppq, cents);
        Some(self.play(sample, &plan))
    }

    /// The sample and speed that play `plan` for `sample`.
    pub fn play(&mut self, sample: &Arc<Sample>, plan: &Plan) -> Played {
        let near = (plan.factor - 1.0).abs() < NEAR;
        if plan.cents == 0.0 && (near || plan.factor == 1.0) || !plan.factor.is_finite() {
            // By speed alone: as long as the stretch makes it.
            let factor = if plan.factor.is_finite() && plan.factor > 0.0 {
                plan.factor
            } else {
                1.0
            };
            return Played {
                sample: Arc::clone(sample),
                step: plan.step / factor,
                scale: 1.0,
            };
        }
        // A pitch p is a stretch by p more, read p times as fast.
        let pitch = (plan.cents / 1200.0).exp2();
        let factor = (plan.factor * pitch).clamp(1.0 / 64.0, 64.0);
        let key = (Arc::as_ptr(sample) as usize, factor.to_bits());
        let stretched = self
            .done
            .entry(key)
            .or_insert_with(|| Arc::new(wsola(sample, factor)));
        Played {
            sample: Arc::clone(stretched),
            step: plan.step * pitch,
            scale: factor,
        }
    }
}

/// `sample` stretched to `factor` times its length at the same pitch: a
/// WSOLA with Hann frames of 40 ms at half overlap. Each frame is taken
/// from within 5 ms of where the stretch puts it, at the offset whose
/// first half matches best what follows the frame before in the sample.
pub(crate) fn wsola(sample: &Sample, factor: f64) -> Sample {
    let len = sample.left.len();
    let size = ((sample.rate as usize / 25).max(64)) & !3;
    let hop = size / 2;
    let reach = (size / 8) as isize;
    let out_len = (len as f64 * factor).ceil() as usize;
    let window: Vec<f32> = (0..size)
        .map(|i| {
            let x = std::f64::consts::TAU * i as f64 / size as f64;
            (0.5 - 0.5 * x.cos()) as f32
        })
        .collect();
    // The guide the frames are matched on: the mono sum, with zeros either
    // side so that no read falls off it, and its mean over every four
    // samples for a first, coarse search.
    let pad = 3 * size;
    let mut guide = vec![0.0f32; pad + len + pad];
    match &sample.right {
        Some(right) => {
            for (g, (l, r)) in guide[pad..].iter_mut().zip(sample.left.iter().zip(right)) {
                *g = 0.5 * (l + r);
            }
        }
        None => guide[pad..pad + len].copy_from_slice(&sample.left),
    }
    let coarse: Vec<f32> = guide
        .chunks_exact(4)
        .map(|four| 0.25 * four.iter().sum::<f32>())
        .collect();
    let at = |signal: &[f32], i: isize| -> f32 {
        if i >= 0 && (i as usize) < signal.len() {
            signal[i as usize]
        } else {
            0.0
        }
    };
    let mut left = vec![0.0f32; out_len];
    let mut right = sample.right.as_ref().map(|_| vec![0.0f32; out_len]);
    // The frames start half a frame before the output does, so that every
    // output sample has two frames over it, whose windows add up to 1.
    let mut last: Option<isize> = None;
    let mut k = 0usize;
    loop {
        let out = k as isize * hop as isize - hop as isize;
        if out >= out_len as isize {
            break;
        }
        // Past either end of the sample a frame is silence wherever it is.
        let nominal =
            ((out as f64 / factor).round() as isize).clamp(-(size as isize), (len + size) as isize);
        let from = match last {
            None => nominal,
            Some(last) => {
                // In the padded guide: where the frame before goes on, and
                // the offsets in reach of where this one belongs.
                let next = (last + hop as isize + pad as isize) as usize;
                let (low, high) = (
                    (nominal - reach + pad as isize) as usize,
                    (nominal + reach + pad as isize) as usize,
                );
                // Every fourth offset on the means of four, then the three
                // either side of the best on every sample.
                let quarter = hop / 4;
                let want = &coarse[(next + 2) / 4..][..quarter];
                let mut best = (f32::MIN, nominal + pad as isize);
                for c in low.div_ceil(4)..=high / 4 {
                    let score = dot(&coarse[c..][..quarter], want);
                    if score > best.0 {
                        best = (score, 4 * c as isize);
                    }
                }
                let around = best.1;
                let want = &guide[next..][..hop];
                best = (f32::MIN, around);
                for at in around - 3..=around + 3 {
                    if at < low as isize || at > high as isize {
                        continue;
                    }
                    let score = dot(&guide[at as usize..][..hop], want);
                    if score > best.0 {
                        best = (score, at);
                    }
                }
                best.1 - pad as isize
            }
        };
        for (i, w) in window.iter().enumerate() {
            let o = out + i as isize;
            if o < 0 || o >= out_len as isize {
                continue;
            }
            let s = from + i as isize;
            left[o as usize] += w * at(&sample.left, s);
            if let (Some(right), Some(source)) = (&mut right, &sample.right) {
                right[o as usize] += w * at(source, s);
            }
        }
        last = Some(from);
        k += 1;
    }
    Sample {
        left,
        right,
        rate: sample.rate,
    }
}

/// The sum of the products of `a` and `b`, eight lanes at a time.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    let (a8, b8) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail: f32 = (a8.remainder().iter().zip(b8.remainder()))
        .map(|(x, y)| x * y)
        .sum();
    for (x, y) in a8.zip(b8) {
        for lane in 0..8 {
            lanes[lane] += x[lane] * y[lane];
        }
    }
    lanes.iter().sum::<f32>() + tail
}

/// Whether the channel's stretch asks for a part the engine does not play:
/// Stretch Pro's formant shift, 215 @160 off its middle 0.5 (control 15,
/// `chan+0x324`, which `FUN_0067db90` makes a formant factor of
/// `2^(1 - 2 formant)` times the pitch). The other modes take no formant.
pub(crate) fn unplayed(project: &Project, channel: &Channel) -> bool {
    let Some(stretch) = Stretch::of(project, channel) else {
        return false;
    };
    let Some(p) = channel.parameters else {
        return false;
    };
    stretch.mode == PRO && p.len >= 168 && f64::from_le_bytes(p.formant) != 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stretch(mode: i32, time: f64, pitch: i32, multiplier: f64) -> Stretch {
        Stretch {
            mode,
            time,
            pitch,
            multiplier,
        }
    }

    #[test]
    fn old_writers_modes_are_remapped_as_the_loader_does() {
        assert_eq!(remap(-1, None), -1);
        assert_eq!(remap(5, Some(25_002_003)), 5);
        // Below 11.5.13 the table, and a 5 out of it is 6.
        assert_eq!(remap(1, Some(11_005_012)), 6);
        assert_eq!(remap(7, Some(11_005_012)), 3);
        assert_eq!(remap(9, Some(11_005_012)), 6);
        assert_eq!(remap(-1, Some(11_005_012)), 0);
        assert_eq!(remap(12, Some(11_005_012)), 6);
        // Below 6.1.5 a move up first: 3 becomes 7, then 3.
        assert_eq!(remap(3, Some(6_001_004)), 3);
        assert_eq!(remap(1, Some(6_001_004)), 7);
    }

    #[test]
    fn a_time_is_an_integer_or_a_float() {
        assert_eq!(time(1109i32.to_le_bytes()), 1109.0);
        assert_eq!(time(1536.5f32.to_le_bytes()), 1536.5);
        assert_eq!(time((-3.0f32).to_le_bytes()), 0.0);
        assert_eq!(time(f32::NAN.to_le_bytes()), 0.0);
    }

    #[test]
    fn a_time_is_192ths_of_a_beat() {
        // 768 = 4 beats at PPQ 96 = 384 ticks; at 100 output samples a
        // tick, 38400 output samples, times the multiplier 2.
        let s = stretch(-1, 768.0, 0, 2.0);
        let plan = s.plan(48_000, 1.0, 100.0, 96, 0.0);
        assert_eq!(plan.step, 1.0);
        assert!((plan.factor - 76_800.0 / 48_000.0).abs() < 1e-12);
        // Resample plays the whole sample in that time, whatever the key.
        let r = stretch(RESAMPLE, 768.0, 0, 1.0).plan(48_000, 1.0, 100.0, 96, 700.0);
        assert_eq!((r.factor, r.cents, r.step), (1.0, 0.0, 48_000.0 / 38_400.0));
        // A load-time mode stretches to the time and plays it to fit.
        let e = stretch(1, 768.0, 0, 1.0).plan(48_000, 0.5, 100.0, 96, 700.0);
        assert_eq!(e.factor, 19_200.0 / 48_000.0);
        assert_eq!(e.step, 0.5);
    }

    #[test]
    fn without_a_time_the_key_shifts_the_pitch_by_mode() {
        let octave = 1200.0;
        // élastique: the length stays, the note and the stretch pitch shift.
        let p = stretch(-1, 0.0, 100, 1.0).plan(1000, 1.0, 1.0, 96, octave);
        assert_eq!((p.factor, p.cents, p.step), (1.0, 1300.0, 1.0));
        // Resample: the multiplier slows it, the key speeds it up.
        let p = stretch(RESAMPLE, 0.0, 0, 2.0).plan(1000, 1.0, 1.0, 96, octave);
        assert_eq!((p.factor, p.step), (1.0, 1.0));
        // Load time: stretched by the multiplier, played at the key.
        let p = stretch(6, 0.0, -200, 2.0).plan(1000, 1.0, 1.0, 96, octave);
        assert_eq!((p.factor, p.cents, p.step), (2.0, -200.0, 2.0));
    }

    fn tone(len: usize, hz: f64, rate: u32) -> Sample {
        let left = (0..len)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32)
            .collect();
        Sample {
            left,
            right: None,
            rate,
        }
    }

    /// Zero crossings a second.
    fn crossings(signal: &[f32], rate: u32) -> f64 {
        let n = signal
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        n as f64 * f64::from(rate) / signal.len() as f64 / 2.0
    }

    #[test]
    fn wsola_keeps_the_pitch_and_changes_the_length() {
        let rate = 44_100;
        let source = tone(rate as usize, 440.0, rate);
        for factor in [0.5, 0.8, 1.5, 2.0] {
            let out = wsola(&source, factor);
            assert_eq!(out.left.len(), (rate as f64 * factor).ceil() as usize);
            // Away from the ends the tone is whole: its level and its pitch.
            let middle = &out.left[out.left.len() / 4..out.left.len() * 3 / 4];
            let peak = middle.iter().fold(0.0f32, |p, x| p.max(x.abs()));
            assert!((0.9..1.1).contains(&peak), "{factor}: {peak}");
            let hz = crossings(middle, rate);
            assert!((hz - 440.0).abs() < 4.0, "{factor}: {hz}");
            assert!(out.left.iter().all(|x| x.is_finite()));
        }
    }

    #[test]
    fn the_stretcher_plays_a_near_length_by_speed_and_shares_what_it_stretched() {
        let source = Arc::new(tone(4410, 440.0, 44_100));
        let mut stretcher = Stretcher::default();
        let near = Plan {
            factor: 1.001,
            cents: 0.0,
            step: 1.0,
        };
        let played = stretcher.play(&source, &near);
        assert!(Arc::ptr_eq(&played.sample, &source));
        assert!((played.step - 1.0 / 1.001).abs() < 1e-12);
        // An octave up: stretched to twice as long, read twice as fast.
        let up = Plan {
            factor: 1.0,
            cents: 1200.0,
            step: 1.0,
        };
        let a = stretcher.play(&source, &up);
        assert_eq!(a.sample.left.len(), 8820);
        assert_eq!((a.step, a.scale), (2.0, 2.0));
        let b = stretcher.play(&source, &up);
        assert!(Arc::ptr_eq(&a.sample, &b.sample));
    }

    #[test]
    #[ignore]
    fn wsola_speed() {
        let rate = 44_100;
        let len = rate as usize * 192;
        let left: Vec<f32> = (0..len)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
            .collect();
        let sample = Sample {
            right: Some(left.clone()),
            left,
            rate,
        };
        for factor in [0.5, 1.1, 2.0] {
            let t = std::time::Instant::now();
            let out = wsola(&sample, factor);
            eprintln!("{factor}: {} frames in {:?}", out.left.len(), t.elapsed());
        }
    }
}
