//! The envelopes FL's plugins share: `TCustomMEnvelope` and the three
//! classes over it, as Fruity Love Philter, Fruity Convolver, Sytrus and
//! Maximus save them and as Love Philter's DLL runs them
//! (`docs/plugins/fruity-love-philter.md`).
//!
//! # The stream
//!
//! ```text
//! u32 kind; u32 version (0 to 3); u32 n
//! n points:  version 2, 3: f64 x, f64 y, f32 tension, u8 mode, u8, u8, i8
//!            version 1:    f32 x, f32 y, f32 tension, u8 mode, u8, u8, i8
//!            version 0:    f32 x, f32 y, f32 tension
//! u32 flags; i32 active; i32 decay; i32 loop start; i32 loop end
//! TMEnvelope adds i32[4]: the attack, decay, sustain and release knobs
//! TMLFO adds      i32[4]: speed, tension, skew and pulse width,
//!                 and from version 3 on an i32 phase
//! ```
//!
//! A point's `x` is how far it is after the point before it. The class
//! that reads the stream says what follows the 20 bytes, so the reader is
//! told the [`Class`]: the kind does not say. Love Philter's patterns are
//! kind 1, its LFOs kind 2 and its maps kind 3. Convolver's layers are kind
//! 6 and its equaliser kind 3, and both end with the 20 bytes. A version 0
//! or 1 point of a kind 6 envelope has its `x` as an f64.
//!
//! The flags: 1 counts `x` in beats and not in half seconds, 2 shares one
//! run between voices, 4 has an LFO's tension bend its whole swing as one
//! curve and not each half of it, and 8 holds an LFO still. `active`
//! switches the envelope on when it is not 0. The three indexes name points
//! and are -1 for none.
//!
//! # Running one
//!
//! A [`Track`] is an envelope made ready, and a [`Run`] is where one voice
//! is in it. FL moves a run on once a control tick by `dt = 1`, with the
//! envelope's `rate` in `x` units a tick: every segment lasts `x / rate`
//! ticks. A run that gets to the loop end jumps to the segment after the
//! loop start, and draws that segment's shape from the value it is at to
//! the value the segment ends on. With a loop end and no loop start it
//! stays on the loop end, and without either it stays on the last point.
//!
//! An [`Lfo`] is such an envelope, which is its level, times an oscillator
//! whose phase is 32 bits to the cycle. The level counts from 0.5: a point
//! at 1 swings the LFO by ±0.5, one at 0.5 stills it and one under 0.5
//! turns it upside down.
//!
//! What is left out, and [`MEnvelope::plain`] says when a stream asks for
//! it:
//!
//! - The sustain knob, which lifts the points after the decay marker.
//! - The second byte after a point's mode, which Sytrus's arpeggiator
//!   reads.
//! - A note's release: nothing here lets a note go, so a run never leaves
//!   its loop. Sytrus will need that.
//! - A smooth segment (mode 10) next to a loop marker takes its slope from
//!   its neighbour in the list and not from the one the loop wraps to.

use crate::StateError;
use dsp::curve::{Curve, Point};

/// The bytes of a state, read from the front.
pub(crate) struct Reader<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }

    /// The next `len` bytes. A state that ends before them is too short.
    pub(crate) fn bytes(&mut self, len: usize) -> Result<&'a [u8], StateError> {
        let end = self.at.saturating_add(len);
        let Some(taken) = self.bytes.get(self.at..end) else {
            return Err(StateError::Length {
                expected: end,
                found: self.bytes.len(),
            });
        };
        self.at = end;
        Ok(taken)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], StateError> {
        let taken = self.bytes(N)?;
        Ok(*taken.first_chunk::<N>().unwrap_or(&[0; N]))
    }

    pub(crate) fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take::<1>()?[0])
    }

    pub(crate) fn i32(&mut self) -> Result<i32, StateError> {
        Ok(i32::from_le_bytes(self.take()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, StateError> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, StateError> {
        Ok(f32::from_le_bytes(self.take()?))
    }

    pub(crate) fn f64(&mut self) -> Result<f64, StateError> {
        Ok(f64::from_le_bytes(self.take()?))
    }

    /// Nothing may follow what was read.
    pub(crate) fn end(&self) -> Result<(), StateError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(StateError::Length {
                expected: self.at,
                found: self.bytes.len(),
            })
        }
    }
}

/// The class that reads a stream, which says what follows its 20 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// `TCustomMEnvelope` and `TMMap`: nothing.
    Map,
    /// `TMEnvelope`: the four quick knobs.
    Envelope,
    /// `TMLFO`: its four knobs and, from version 3 on, its phase.
    Lfo,
}

/// The newest version of the stream there is.
const NEWEST: u32 = 3;

/// The kind whose old points have an f64 `x`.
const LAYER: u32 = 6;

/// What a `TMEnvelope`'s knobs are when nobody has moved them.
pub(crate) const ENVELOPE_KNOBS: [i32; 4] = [128, 128, 0, 128];

/// And a `TMLFO`'s.
pub(crate) const LFO_KNOBS: [i32; 4] = [40_000, 0, 0, 0];

/// `x` is in beats.
const TEMPO: u32 = 1;
/// An LFO's tension bends its whole swing as one curve.
const WHOLE: u32 = 4;
/// An LFO stands still. Only `TMLFO`'s update reads this bit of the
/// stream's flags (+0x44): with it set the phase step +0xa0 is 0 (Love
/// Philter `0x7887d5`, Sytrus `0x78a365`, Harmor `0x7a3070`). It is not
/// the bit that drops an LFO's decay marker: see [`MEnvelope::read`].
const FROZEN: u32 = 8;

/// One envelope as a stream has it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MEnvelope {
    pub(crate) kind: u32,
    pub(crate) version: u32,
    pub(crate) points: Vec<Point>,
    /// The second byte after each point's mode.
    pub(crate) arp: Vec<u8>,
    pub(crate) flags: u32,
    pub(crate) active: bool,
    /// The point the decay starts at, the one a loop turns back to and the
    /// one it turns back at. -1 for none.
    pub(crate) decay: i32,
    pub(crate) loop_start: i32,
    pub(crate) loop_end: i32,
    /// The four numbers a `TMEnvelope` or a `TMLFO` adds. A map has the
    /// envelope's defaults.
    pub(crate) knobs: [i32; 4],
    /// A `TMLFO`'s phase, in 2^32ths of a cycle.
    pub(crate) phase: i32,
}

impl MEnvelope {
    /// Read one envelope where the reader is. FL turns a version past 3
    /// down, and so does this.
    pub(crate) fn read(reader: &mut Reader<'_>, class: Class) -> Result<Self, StateError> {
        let kind = reader.u32()?;
        let version = reader.u32()?;
        if version > NEWEST {
            return Err(StateError::Version {
                found: version as i32,
            });
        }
        let count = reader.u32()? as usize;
        // No state has room for more points than it has bytes.
        let room = reader.bytes.len().saturating_sub(reader.at) / 12;
        if count > room {
            return Err(StateError::Length {
                expected: reader.at.saturating_add(count.saturating_mul(12)),
                found: reader.bytes.len(),
            });
        }
        let mut points = Vec::with_capacity(count);
        let mut arp = Vec::with_capacity(count);
        for _ in 0..count {
            let (x, y) = if version >= 2 {
                (reader.f64()?, reader.f64()?)
            } else if kind == LAYER {
                (reader.f64()?, f64::from(reader.f32()?))
            } else {
                (f64::from(reader.f32()?), f64::from(reader.f32()?))
            };
            let tension = reader.f32()?;
            let rest = if version >= 1 {
                reader.take::<4>()?
            } else {
                [0; 4]
            };
            points.push(Point {
                x,
                y,
                tension,
                mode: rest[0],
            });
            arp.push(rest[1]);
        }
        let flags = reader.u32()?;
        let active = reader.i32()? != 0;
        let (mut decay, loop_start, loop_end) = (reader.i32()?, reader.i32()?, reader.i32()?);
        // FL's reader (Sytrus `0x781be0`, the test at `0x781f53`) sets the
        // decay marker to -1 when bit 8 of the class's own mask (+0x20) is
        // clear, whatever the version or the stream's flags. The base
        // constructor sets the mask to 0x3f (`0x781a20`) and only `TMLFO`'s
        // clears that bit (`0x789fa0`; Love Philter `0x7884e0`), so an LFO
        // never keeps one.
        if class == Class::Lfo {
            decay = -1;
        }
        let mut knobs = ENVELOPE_KNOBS;
        let mut phase = 0;
        if class != Class::Map {
            for knob in &mut knobs {
                *knob = reader.i32()?;
            }
        }
        if class == Class::Lfo && version >= 3 {
            phase = reader.i32()?;
        }
        Ok(MEnvelope {
            kind,
            version,
            points,
            arp,
            flags,
            active,
            decay,
            loop_start,
            loop_end,
            knobs,
            phase,
        })
    }

    /// The level every point is at, when they are all at one.
    pub(crate) fn flat(&self) -> Option<f32> {
        let level = self.points.first()?.y as f32;
        let same = self.points.iter().all(|point| point.y as f32 == level);
        same.then_some(level)
    }

    /// The curve at `steps + 1` places, a `steps`th of an `x` unit apart
    /// from its first point on, each less `centre`: the table FL bakes a
    /// map into. A map is one unit long.
    pub(crate) fn table(&self, steps: usize, centre: f64) -> Vec<f32> {
        let curve = Curve::new(&self.points);
        let start = self.points.first().map_or(0.0, |point| point.x.max(0.0));
        let place = |i: usize| start + i as f64 / steps as f64;
        let value = |i: usize| (curve.value_at(place(i)) - centre) as f32;
        (0..=steps).map(value).collect()
    }

    /// Whether [`Track`] runs the envelope as FL does: no knob is moved
    /// that this leaves out, and no point is one for the arpeggiator. The
    /// sustain knob is a `TMEnvelope`'s, so `class` says whether the
    /// envelope has one.
    pub(crate) fn plain(&self, class: Class) -> bool {
        let sustain = class == Class::Envelope && self.knobs[2] != 0 && self.loop_end >= 0;
        !sustain && self.arp.iter().all(|byte| *byte == 0)
    }
}

/// A run that goes round a loop of no length more often than this in one
/// tick is put down on its point.
const TURNS: usize = 4096;

/// Where a run is in its envelope. FL numbers the stages: 1 is the attack,
/// 3 the decay and 9 the end.
const DECAY: i32 = 3;
const HELD: i32 = 5;
const RELEASE: i32 = 6;
const OVER: i32 = 9;

/// An envelope made ready to run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Track {
    curve: Curve,
    /// Every point's `x` and its value less the centre, as FL keeps them.
    lengths: Vec<f32>,
    values: Vec<f32>,
    decay: i32,
    loop_start: i32,
    loop_end: i32,
    /// The point a run turns back at or stays on.
    last: i32,
    /// The loop has no length, so a run that gets to it stays.
    stuck: bool,
    /// What the attack, decay, sustain and release knobs make a segment's
    /// length.
    scales: [f32; 4],
    active: bool,
    /// `x` is in beats, and in half seconds otherwise.
    pub(crate) tempo: bool,
}

/// Where one voice is in a [`Track`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Run {
    stage: i32,
    /// The stage the markers have got the run to.
    held: i32,
    /// The point that ends the segment the run is in.
    point: usize,
    /// How much of the segment is over, and how much more is after a tick.
    phase: f64,
    step: f64,
    /// The value the segment starts at and how far it rises.
    from: f32,
    rise: f32,
    /// The envelope's value now.
    pub(crate) out: f32,
    /// What this voice's attack and decay segments are stretched by, on
    /// top of the knobs: Sytrus's unison spreads them (`0x86af20`).
    stretch: [f32; 2],
}

impl Run {
    /// Stretch this run's attack and decay segments, as Sytrus scales a
    /// unison voice's copy of the knobs. This allocates nothing.
    pub(crate) fn stretch(&mut self, attack: f32, decay: f32) {
        self.stretch = [attack, decay];
    }

    /// Whether the run has played the envelope out.
    pub(crate) fn over(&self) -> bool {
        self.stage >= OVER
    }
}

impl Track {
    /// `centre` is what the envelope's values are counted from: an
    /// articulator that adds has 0.5 and one that multiplies 0.
    pub(crate) fn new(envelope: &MEnvelope, centre: f64) -> Self {
        let points = &envelope.points;
        let count = points.len() as i32;
        let mut last = envelope.loop_end;
        if last < 0 && envelope.loop_start >= 0 {
            last = count - 1;
        }
        let place = |index: i32| -> f64 {
            let upto = usize::try_from(index + 1).unwrap_or(0).min(points.len());
            points[..upto].iter().map(|point| point.x.max(0.0)).sum()
        };
        let stuck = last >= 0
            && envelope.loop_start >= 0
            && place(last) - place(envelope.loop_start) <= 1e-9;
        Track {
            curve: Curve::new(points),
            lengths: points.iter().map(|p| p.x.max(0.0) as f32).collect(),
            values: points.iter().map(|p| (p.y - centre) as f32).collect(),
            decay: envelope.decay,
            loop_start: envelope.loop_start,
            loop_end: envelope.loop_end,
            last,
            stuck,
            scales: envelope.knobs.map(|knob| knob as f32 / 128.0),
            active: envelope.active,
            tempo: envelope.flags & TEMPO != 0,
        }
    }

    /// Take the attack, decay, sustain and release knobs anew, as
    /// `TMEnvelope`'s `0x789de0` (Sytrus) does when a host moves one. This
    /// allocates nothing.
    pub(crate) fn set_knobs(&mut self, knobs: [i32; 4]) {
        self.scales = knobs.map(|knob| knob as f32 / 128.0);
    }

    /// Whether the envelope is switched on.
    pub(crate) fn active(&self) -> bool {
        self.active
    }

    /// A run at the envelope's first point. `rate` is how many `x` units a
    /// tick is.
    pub(crate) fn start(&self, rate: f64) -> Run {
        let mut run = Run {
            stage: OVER,
            held: OVER,
            point: 0,
            phase: 0.0,
            step: 0.0,
            from: 0.0,
            rise: 0.0,
            out: 0.0,
            stretch: [1.0; 2],
        };
        let (Some(first), true) = (self.lengths.first(), self.active) else {
            return run;
        };
        run.stage = if self.decay < 0 { DECAY } else { 1 };
        run.held = run.stage;
        if *first == 0.0 {
            (run.step, run.phase) = (1.0, 1.0);
        } else {
            run.step = rate / f64::from(*first);
        }
        run.from = self.values[0];
        run.out = run.from;
        run
    }

    /// A run that gets to the envelope's first point with its first tick,
    /// which is how Love Philter starts one: FL seeks it to a tick before
    /// its start.
    pub(crate) fn start_before(&self, rate: f64) -> Run {
        let mut run = self.start(rate);
        if run.stage != OVER {
            (run.step, run.phase) = (1.0, 0.0);
        }
        run
    }

    /// Let a run go, as a note's release does in Sytrus: it leaves the
    /// point it holds or loops at and plays the segments after it, from
    /// the value it is at now, with the release knob timing them. A run
    /// with no such point plays out as it was, and one that is over stays
    /// over. `rate` is as for [`Track::advance`]. This allocates nothing.
    pub(crate) fn release(&self, run: &mut Run, rate: f64) {
        if run.stage >= RELEASE {
            return;
        }
        run.stage = RELEASE;
        run.held = RELEASE;
        let count = self.values.len();
        if self.last < 0 || run.point >= count {
            return;
        }
        let next = (self.last + 1) as usize;
        if next >= count {
            run.stage = OVER;
            (run.phase, run.step) = (0.0, 0.0);
            run.from = run.out;
            run.rise = 0.0;
            return;
        }
        run.from = run.out;
        run.point = next;
        run.rise = self.values[next] - run.from;
        let length = f64::from(self.lengths[next]) * f64::from(self.scales[3]);
        if length <= 0.0 {
            (run.step, run.phase) = (1.0, 1.0);
        } else {
            (run.step, run.phase) = (rate / length, 0.0);
        }
    }

    /// How long the segments after the point a run holds at are, in `x`
    /// units with the release knob on them: what a note sounds on for
    /// after its release. The whole envelope when there is no such point.
    pub(crate) fn release_length(&self) -> f64 {
        let from = if self.last < 0 {
            0
        } else {
            (self.last + 1) as usize
        };
        let rest: f64 = self.lengths.iter().skip(from).map(|l| f64::from(*l)).sum();
        rest * f64::from(self.scales[3])
    }

    /// Move a run on by `dt` ticks. This allocates nothing.
    pub(crate) fn advance(&self, run: &mut Run, dt: f64, rate: f64) {
        let count = self.values.len();
        if run.point >= count {
            return;
        }
        run.phase += dt * run.step;
        let mut turns = 0;
        while run.phase >= 1.0 {
            // What of the tick is left once the segment is over.
            let left = if run.step == 0.0 {
                dt
            } else {
                (run.phase - 1.0) / run.step
            };
            run.from = self.values[run.point];
            let at = run.point as i32;
            let done = if at < self.last || self.last < 0 || run.stage > HELD {
                if self.decay <= at && self.decay >= 0 && run.held < DECAY {
                    run.stage = run.stage.max(DECAY);
                    run.held = DECAY;
                }
                if run.point + 1 < count {
                    run.point += 1;
                    false
                } else {
                    run.stage = OVER;
                    true
                }
            } else if self.loop_start < 0 {
                true
            } else {
                run.held = 4;
                run.point = (self.loop_start as usize + 1).min(count - 1);
                self.stuck
            };
            run.rise = self.values[run.point] - run.from;
            turns += 1;
            if done || turns > TURNS {
                (run.phase, run.step) = (0.0, 0.0);
                break;
            }
            let released = run.stage >= RELEASE && self.loop_end >= 0;
            let knob = usize::from(run.stage > 2) + 2 * usize::from(released);
            let mut scale = self.scales[knob];
            if knob < 2 {
                scale *= run.stretch[knob];
            }
            let length = f64::from(self.lengths[run.point]) * f64::from(scale);
            if length <= 0.0 {
                run.step = 1.0;
                run.phase = left + 1.0;
            } else {
                run.step = rate / length;
                run.phase = left * run.step;
            }
        }
        run.out = if run.rise == 0.0 {
            run.from
        } else {
            run.from + self.curve.progress(run.point, run.phase) as f32 * run.rise
        };
    }
}

/// The oscillator of an LFO: a triangle whose fall and rise the skew and
/// the pulse width move, bent by the tension.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Oscillator {
    /// The phase the fall ends at, the one the rise starts at and the one
    /// it ends at.
    fall: u32,
    low: u32,
    top: u32,
    /// What a step of the phase is worth while it falls and while it
    /// rises.
    down: f32,
    up: f32,
    tension: f32,
    /// `1 / |tension|` and `ln(|tension| + 1)`.
    inverse: f32,
    log: f32,
    /// The tension bends the whole swing as one curve, and each half of it
    /// otherwise.
    whole: bool,
}

/// 2^32, the phase of a whole cycle.
const CYCLE: f64 = 4_294_967_296.0;

impl Oscillator {
    /// `knobs` are a `TMLFO`'s tension, skew and pulse width.
    fn new(knobs: [i32; 3], whole: bool) -> Self {
        let [tension, skew, width] = knobs;
        // The tension knob's ±128 is ±(1e18^t - 1) with t = (21^a - 1) / 20.
        let a = tension.unsigned_abs() as f32 / 128.0;
        let t = ((f64::from(a) * 21f64.ln()).exp() - 1.0) as f32;
        let t = (f64::from(t) * 0.05) as f32;
        let bend = ((f64::from(t) * 1e18f64.ln()).exp() - 1.0) as f32;
        let tension = bend * tension.signum() as f32;
        let (skew, width) = (skew as f32 / 256.0, width as f32 / 128.0);

        let top = if width > 0.0 {
            f64::from((1.0 - width) * CYCLE as f32 - 1.0).round_ties_even() as i64 as u32
        } else {
            u32::MAX
        };
        let middle = (f64::from((1.0 - width) / 2.0)
            - f64::from(skew) * (1.0 - f64::from(width).abs()))
            * CYCLE;
        let middle = middle.min(f64::from(top.wrapping_sub(1))).max(1.0);
        let low = middle.round_ties_even() as i64 as u32;
        let fall = if width < 0.0 {
            let fall = (middle + f64::from(width * CYCLE as f32)).round_ties_even();
            (fall as i64 as u32).max(1)
        } else {
            low
        };
        let (inverse, log) = if tension == 0.0 {
            (0.0, 0.0)
        } else {
            let size = f64::from(tension).abs();
            ((1.0 / size) as f32, (size + 1.0).ln() as f32)
        };
        Oscillator {
            fall,
            low,
            top,
            down: (2.0 / f64::from(fall)) as f32,
            up: (2.0 / f64::from(top.wrapping_sub(low))) as f32,
            tension,
            inverse,
            log,
            whole,
        }
    }

    /// The phase a new voice starts at: half way up the rise, where the
    /// triangle crosses 0.
    fn start(&self) -> u32 {
        self.low
            .wrapping_add(self.top.wrapping_sub(self.low) >> 1)
            .wrapping_add(1)
    }

    /// The tension's bend of a level from 0 to 1.
    fn bend(&self, level: f64) -> f64 {
        let (inverse, log) = (f64::from(self.inverse), f64::from(self.log));
        if self.tension == 0.0 {
            level
        } else if self.tension > 0.0 {
            1.0 - (((1.0 - level) * log).exp() - 1.0) * inverse
        } else {
            ((level * log).exp() - 1.0) * inverse
        }
    }

    /// The oscillator at a phase: -1 to 1.
    fn at(&self, phase: u32) -> f32 {
        if self.top < phase {
            return 1.0;
        }
        let line = if phase < self.fall {
            1.0 - phase as f32 * self.down
        } else if phase < self.low {
            return -1.0;
        } else {
            (phase - self.low) as f32 * self.up - 1.0
        };
        if self.whole {
            let level = f64::from((line + 1.0) * 0.5);
            (self.bend(level) * 2.0 - 1.0) as f32
        } else {
            (self.bend(f64::from(line.abs())) as f32).copysign(line)
        }
    }
}

/// What an LFO's level is counted from. `TMLFO`'s constructor sets its
/// centre (+0x18) to 0.5 before the base constructor runs (Love Philter
/// `0x788410`, Sytrus `0x789fa0`, Harmor `0x7a2dd0`), and no articulator's
/// mode setter touches it: they set the centre of the envelope and the
/// maps only (Love Philter `0x8544d0`, `0x789cc0`; Sytrus `0x861300`).
const LEVEL_CENTRE: f64 = 0.5;

/// An LFO made ready to run: its level, which is an envelope, and its
/// oscillator.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Lfo {
    level: Track,
    oscillator: Oscillator,
    speed: i32,
    /// The phase the state adds to where a voice starts.
    phase: u32,
    /// The phase a voice starts at.
    start: u32,
    whole: bool,
    frozen: bool,
}

/// Where one voice is in an [`Lfo`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LfoRun {
    level: Run,
    phase: u32,
    /// The LFO's value now: its level less 0.5, times its oscillator.
    pub(crate) out: f32,
}

impl Lfo {
    pub(crate) fn new(envelope: &MEnvelope) -> Self {
        let [speed, tension, skew, width] = envelope.knobs;
        let whole = envelope.flags & WHOLE != 0;
        let oscillator = Oscillator::new([tension, skew, width], whole);
        Lfo {
            level: Track::new(envelope, LEVEL_CENTRE),
            phase: envelope.phase as u32,
            start: oscillator.start().wrapping_add(envelope.phase as u32),
            oscillator,
            speed,
            whole,
            frozen: envelope.flags & FROZEN != 0,
        }
    }

    /// Take the speed, tension, skew and pulse width knobs anew. This
    /// allocates nothing.
    pub(crate) fn set_knobs(&mut self, knobs: [i32; 4]) {
        let [speed, tension, skew, width] = knobs;
        self.speed = speed;
        self.oscillator = Oscillator::new([tension, skew, width], self.whole);
        self.start = self.oscillator.start().wrapping_add(self.phase);
    }

    pub(crate) fn active(&self) -> bool {
        self.level.active
    }

    /// Whether the LFO counts in beats.
    pub(crate) fn tempo(&self) -> bool {
        self.level.tempo
    }

    /// How far the phase moves in a tick at this rate. The speed knob's
    /// 65536 is `(10001^(v / 65536) - 1) * 6871947.6736` of the 2^32 a
    /// cycle has for every `x` unit: 26259 is 16 beats to the cycle and
    /// 21502 is 32.
    pub(crate) fn pace(&self, rate: f64) -> f64 {
        if self.frozen {
            return 0.0;
        }
        let knob = (f64::from(self.speed) * 1.525_878_906_25e-5) as f32;
        let speed = ((f64::from(knob) * 10_001f64.ln()).exp() - 1.0) as f32;
        let pace = f64::from(speed) * 6_871_947.673_6 * rate;
        pace.min(805_306_368.0).round_ties_even()
    }

    pub(crate) fn start(&self, rate: f64) -> LfoRun {
        LfoRun {
            level: self.level.start(rate),
            phase: self.start,
            out: 0.0,
        }
    }

    /// A run a tick before its start, as [`Track::start_before`] has it.
    pub(crate) fn start_before(&self, rate: f64) -> LfoRun {
        LfoRun {
            level: self.level.start_before(rate),
            phase: self.start.wrapping_sub(self.pace(rate) as u32),
            out: 0.0,
        }
    }

    /// Move a run on by `dt` ticks. The oscillator waits until the level
    /// has left its first point, which is the LFO's delay.
    pub(crate) fn advance(&self, run: &mut LfoRun, dt: f64, rate: f64) {
        let waiting = |level: &Run| level.point == 0 && level.stage < OVER && level.phase < 1.0;
        let mut turn = dt;
        if waiting(&run.level) {
            turn = (dt + (run.level.phase - 1.0) / run.level.step).max(0.0);
        }
        self.level.advance(&mut run.level, dt, rate);
        if !waiting(&run.level) {
            let moved = (turn * self.pace(rate)).round_ties_even() as i64;
            run.phase = run.phase.wrapping_add(moved as u32);
            run.out = run.level.out * self.oscillator.at(run.phase);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A stream of this version, as FL writes one.
    pub(crate) fn stream(
        kind: u32,
        version: u32,
        points: &[Point],
        block: [i32; 5],
        tail: &[i32],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        for word in [kind, version, points.len() as u32] {
            bytes.extend(word.to_le_bytes());
        }
        for point in points {
            if version >= 2 {
                bytes.extend(point.x.to_le_bytes());
                bytes.extend(point.y.to_le_bytes());
            } else {
                bytes.extend((point.x as f32).to_le_bytes());
                bytes.extend((point.y as f32).to_le_bytes());
            }
            bytes.extend(point.tension.to_le_bytes());
            if version >= 1 {
                bytes.extend([point.mode, 0, 0, 0]);
            }
        }
        for word in block.iter().chain(tail) {
            bytes.extend(word.to_le_bytes());
        }
        bytes
    }

    fn point(x: f64, y: f64, mode: u8) -> Point {
        Point {
            mode,
            ..Point::new(x, y)
        }
    }

    fn read(bytes: &[u8], class: Class) -> Result<MEnvelope, StateError> {
        let mut reader = Reader::new(bytes);
        let envelope = MEnvelope::read(&mut reader, class)?;
        reader.end()?;
        Ok(envelope)
    }

    /// An envelope that is switched on and counts in beats.
    fn envelope(points: &[Point], markers: [i32; 3]) -> MEnvelope {
        let [decay, start, end] = markers;
        let bytes = stream(1, 3, points, [1, -1, decay, start, end], &ENVELOPE_KNOBS);
        read(&bytes, Class::Envelope).unwrap()
    }

    #[test]
    fn every_version_and_class_of_the_stream_reads_to_its_last_byte() {
        let points = [point(0.0, 0.25, 0), point(1.5, 1.0, 2)];
        for version in 0..=3 {
            let map = stream(3, version, &points, [0, -1, -1, -1, -1], &[]);
            let size = [12, 16, 24, 24][version as usize];
            assert_eq!(map.len(), 12 + 2 * size + 20);
            let read_map = read(&map, Class::Map).unwrap();
            assert_eq!((read_map.kind, read_map.version), (3, version));
            assert_eq!(read_map.points[1].x, 1.5);
            assert_eq!(read_map.points[1].y, 1.0);
            // A version 0 point has no mode.
            assert_eq!(read_map.points[1].mode, if version == 0 { 0 } else { 2 });
            assert!(read_map.active);
            assert_eq!(read_map.knobs, ENVELOPE_KNOBS);

            let adsr = stream(1, version, &points, [3, 0, 1, 0, 1], &[64, 32, 16, 8]);
            let read_adsr = read(&adsr, Class::Envelope).unwrap();
            assert_eq!(adsr.len(), map.len() + 16);
            assert!(!read_adsr.active);
            assert_eq!(read_adsr.flags, 3);
            let markers = (read_adsr.decay, read_adsr.loop_start, read_adsr.loop_end);
            assert_eq!(markers, (1, 0, 1));
            assert_eq!(read_adsr.knobs, [64, 32, 16, 8]);

            // The phase is there from version 3 on.
            let tail: &[i32] = if version == 3 {
                &[26_259, 1, 2, 3, 1 << 30]
            } else {
                &[26_259, 1, 2, 3]
            };
            let lfo = stream(2, version, &points, [1, -1, -1, -1, -1], tail);
            let read_lfo = read(&lfo, Class::Lfo).unwrap();
            assert_eq!(read_lfo.knobs, [26_259, 1, 2, 3]);
            assert_eq!(read_lfo.phase, if version == 3 { 1 << 30 } else { 0 });
            // Read as the wrong class, it does not end where it should.
            assert!(read(&lfo, Class::Map).is_err());
        }
    }

    #[test]
    fn an_lfo_drops_its_decay_marker_and_the_other_classes_keep_it() {
        let points = [point(0.0, 0.0, 0), point(1.0, 1.0, 0), point(1.0, 0.5, 0)];
        for version in 0..=3 {
            // Flags 0 and 8, so the frozen bit is not what drops it.
            for flags in [0, FROZEN as i32] {
                let block = [flags, 1, 1, 0, 2];
                let tail: &[i32] = if version == 3 {
                    &[40_000, 0, 0, 0, 0]
                } else {
                    &[40_000, 0, 0, 0]
                };
                let lfo = read(&stream(2, version, &points, block, tail), Class::Lfo).unwrap();
                assert_eq!((lfo.decay, lfo.loop_start, lfo.loop_end), (-1, 0, 2));
                let adsr = stream(1, version, &points, block, &ENVELOPE_KNOBS);
                assert_eq!(read(&adsr, Class::Envelope).unwrap().decay, 1);
                let map = stream(3, version, &points, block, &[]);
                assert_eq!(read(&map, Class::Map).unwrap().decay, 1);
            }
        }
    }

    #[test]
    fn an_old_layer_has_its_x_as_a_double() {
        let mut bytes = Vec::new();
        for word in [6u32, 1, 1] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(2.5f64.to_le_bytes());
        bytes.extend(0.5f32.to_le_bytes());
        bytes.extend(0f32.to_le_bytes());
        bytes.extend([0, 0, 0, 0]);
        bytes.extend([0; 20]);
        let layer = read(&bytes, Class::Map).unwrap();
        assert_eq!((layer.points[0].x, layer.points[0].y), (2.5, 0.5));
    }

    #[test]
    fn a_stream_that_is_cut_short_or_too_new_is_turned_down() {
        let points = [point(0.0, 0.5, 0), point(1.0, 0.5, 0)];
        let bytes = stream(3, 3, &points, [0, -1, -1, -1, -1], &[]);
        for len in 0..bytes.len() {
            let cut = read(&bytes[..len], Class::Map);
            assert!(matches!(cut, Err(StateError::Length { .. })), "{len}");
        }
        let newer = stream(3, 4, &points, [0, -1, -1, -1, -1], &[]);
        assert_eq!(
            read(&newer, Class::Map),
            Err(StateError::Version { found: 4 })
        );
        // A count no state has the bytes for asks for no memory.
        let mut huge = bytes.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            read(&huge, Class::Map),
            Err(StateError::Length { .. })
        ));
    }

    #[test]
    fn a_map_is_baked_into_a_table_and_a_level_line_is_flat() {
        let ramp = read(
            &stream(
                3,
                3,
                &[point(0.0, 0.0, 0), point(1.0, 1.0, 0)],
                [0, -1, -1, -1, -1],
                &[],
            ),
            Class::Map,
        )
        .unwrap();
        assert_eq!(ramp.flat(), None);
        let table = ramp.table(1024, 0.5);
        assert_eq!(table.len(), 1025);
        assert_eq!((table[0], table[512], table[1024]), (-0.5, 0.0, 0.5));

        let level = MEnvelope {
            points: vec![point(0.0, 0.25, 0), point(1.0, 0.25, 0)],
            ..ramp
        };
        assert_eq!(level.flat(), Some(0.25));
        assert!(level.table(8, 0.0).iter().all(|v| *v == 0.25));
    }

    #[test]
    fn a_run_takes_a_segment_s_length_over_the_rate_in_ticks() {
        // Up over a beat and down over two, at a quarter of a beat a tick.
        let points = [point(0.0, 0.0, 0), point(1.0, 1.0, 0), point(2.0, 0.0, 0)];
        let track = Track::new(&envelope(&points, [-1, -1, -1]), 0.0);
        assert!(track.active() && track.tempo);
        let mut run = track.start(0.25);
        assert_eq!(run.out, 0.0);
        let mut heard = Vec::new();
        for _ in 0..14 {
            track.advance(&mut run, 1.0, 0.25);
            heard.push(run.out);
        }
        let wanted = [
            0.25, 0.5, 0.75, 1.0, 0.875, 0.75, 0.625, 0.5, 0.375, 0.25, 0.125, 0.0, 0.0, 0.0,
        ];
        for (tick, (heard, wanted)) in heard.iter().zip(wanted).enumerate() {
            assert!((heard - wanted).abs() < 1e-6, "tick {tick}: {heard}");
        }
    }

    #[test]
    fn a_loop_turns_back_from_its_end_into_the_segment_after_its_start() {
        // A gate: on for a beat, off for a beat, held steps.
        let points = [
            point(0.0, 1.0, 0),
            point(1.0, 0.0, 2),
            point(1.0, 1.0, 2),
            point(1.0, 0.0, 2),
        ];
        let track = Track::new(&envelope(&points, [-1, 0, 2]), 0.0);
        let mut run = track.start(0.5);
        let mut heard = Vec::new();
        for _ in 0..12 {
            heard.push(run.out);
            track.advance(&mut run, 1.0, 0.5);
        }
        // Two ticks a beat. The loop is points 0 to 2, two beats long, and
        // the fourth point is never played.
        assert_eq!(
            heard,
            [1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0]
        );

        // A loop whose ends differ ramps from where it is to the point after
        // its start.
        let saw = [point(0.0, 0.0, 0), point(1.0, 1.0, 0), point(1.0, 0.5, 0)];
        let track = Track::new(&envelope(&saw, [-1, 0, 2]), 0.0);
        let mut run = track.start(0.5);
        let mut heard = Vec::new();
        for _ in 0..8 {
            track.advance(&mut run, 1.0, 0.5);
            heard.push(run.out);
        }
        assert_eq!(heard, [0.5, 1.0, 0.75, 0.5, 0.75, 1.0, 0.75, 0.5]);
    }

    #[test]
    fn a_release_leaves_the_held_point_from_the_value_the_run_is_at() {
        // Up over a beat, hold, then down over a beat.
        let points = [point(0.0, 0.0, 0), point(1.0, 1.0, 0), point(1.0, 0.0, 0)];
        let track = Track::new(&envelope(&points, [-1, -1, 1]), 0.0);
        assert_eq!(track.release_length(), 1.0);
        let mut run = track.start(0.5);
        for _ in 0..4 {
            track.advance(&mut run, 1.0, 0.5);
        }
        assert_eq!(run.out, 1.0);
        assert!(!run.over());
        track.release(&mut run, 0.5);
        let mut heard = Vec::new();
        for _ in 0..3 {
            track.advance(&mut run, 1.0, 0.5);
            heard.push(run.out);
        }
        assert_eq!(heard, [0.5, 0.0, 0.0]);
        assert!(run.over());

        // Let go half way up, it falls from there, and a second release
        // changes nothing.
        let mut run = track.start(0.5);
        track.advance(&mut run, 1.0, 0.5);
        assert_eq!(run.out, 0.5);
        track.release(&mut run, 0.5);
        track.release(&mut run, 0.5);
        track.advance(&mut run, 1.0, 0.5);
        assert_eq!(run.out, 0.25);

        // Without a held point the envelope plays out as it was, and the
        // release is all of it: two beats, the first point being at 0.
        let plain = Track::new(&envelope(&points, [-1, -1, -1]), 0.0);
        assert_eq!(plain.release_length(), 2.0);
        let mut run = plain.start(0.5);
        plain.advance(&mut run, 1.0, 0.5);
        plain.release(&mut run, 0.5);
        plain.advance(&mut run, 1.0, 0.5);
        assert_eq!(run.out, 1.0);
    }

    #[test]
    fn a_loop_end_without_a_start_holds_and_a_loop_of_no_length_stays() {
        let points = [point(0.0, 0.0, 0), point(1.0, 1.0, 0), point(1.0, 0.0, 0)];
        let track = Track::new(&envelope(&points, [-1, -1, 1]), 0.0);
        let mut run = track.start(1.0);
        for _ in 0..5 {
            track.advance(&mut run, 1.0, 1.0);
            assert_eq!(run.out, 1.0);
        }

        let still = [point(0.0, 0.25, 0), point(0.0, 0.75, 0)];
        let track = Track::new(&envelope(&still, [-1, 0, 1]), 0.0);
        let mut run = track.start(1.0);
        for _ in 0..3 {
            track.advance(&mut run, 1.0, 1.0);
            assert!(run.out.is_finite());
        }
    }

    #[test]
    fn an_envelope_that_is_off_or_empty_runs_at_nothing() {
        let off = MEnvelope {
            active: false,
            ..envelope(&[point(0.0, 1.0, 0)], [-1, -1, -1])
        };
        let track = Track::new(&off, 0.0);
        let mut run = track.start(1.0);
        track.advance(&mut run, 1.0, 1.0);
        assert_eq!(run.out, 0.0);

        let empty = Track::new(&envelope(&[], [-1, -1, -1]), 0.0);
        let mut run = empty.start(1.0);
        empty.advance(&mut run, 1.0, 1.0);
        assert_eq!(run.out, 0.0);
    }

    #[test]
    fn the_quick_knobs_stretch_the_segments_and_the_sustain_knob_is_not_plain() {
        let points = [point(0.0, 0.0, 0), point(1.0, 1.0, 0)];
        let mut slow = envelope(&points, [-1, -1, -1]);
        // No decay marker, so the decay knob times every segment.
        slow.knobs = [128, 256, 0, 128];
        let track = Track::new(&slow, 0.0);
        let mut run = track.start(0.5);
        track.advance(&mut run, 1.0, 0.5);
        assert!((run.out - 0.25).abs() < 1e-6);
        assert!(slow.plain(Class::Envelope));

        let mut lifted = envelope(&points, [-1, 0, 1]);
        lifted.knobs[2] = 64;
        assert!(!lifted.plain(Class::Envelope));
        lifted.knobs[2] = 0;
        lifted.arp[1] = 1;
        assert!(!lifted.plain(Class::Envelope));
    }

    /// An LFO that is on, with one point at full level.
    fn lfo(knobs: [i32; 4], flags: i32, phase: i32) -> Lfo {
        let tail = [knobs[0], knobs[1], knobs[2], knobs[3], phase];
        let bytes = stream(2, 3, &[point(0.0, 1.0, 0)], [flags, -1, -1, -1, -1], &tail);
        Lfo::new(&read(&bytes, Class::Lfo).unwrap())
    }

    #[test]
    fn the_speed_knob_is_16_beats_to_the_cycle_at_26259() {
        for (speed, beats) in [(26_259, 16.0), (21_502, 32.0)] {
            let pace = lfo([speed, 0, 0, 0], 1, 0).pace(1.0);
            assert!((CYCLE / pace - beats).abs() < 0.01 * beats, "{speed}");
        }
        // The pace has a ceiling, and a frozen LFO has none.
        assert_eq!(lfo([65_536, 0, 0, 0], 1, 0).pace(1.0), 805_306_368.0);
        assert_eq!(lfo([26_259, 0, 0, 0], 1 | 8, 0).pace(1.0), 0.0);
    }

    #[test]
    fn an_lfo_is_a_triangle_that_starts_at_0_on_its_way_up() {
        let lfo = lfo([26_259, 0, 0, 0], 1, 0);
        assert!(lfo.active() && lfo.tempo());
        // A 64th of a cycle a tick.
        let rate = CYCLE / 64.0 / lfo.pace(1.0);
        let mut run = lfo.start(rate);
        let mut heard = Vec::new();
        for _ in 0..64 {
            lfo.advance(&mut run, 1.0, rate);
            heard.push(run.out);
        }
        // A level of 1 is 0.5 over the centre, so the swing is ±0.5.
        let peak = heard.iter().fold(0f32, |max, v| max.max(*v));
        let low = heard.iter().fold(0f32, |min, v| min.min(*v));
        assert!((peak - 0.5).abs() < 0.035 && (low + 0.5).abs() < 0.035);
        // Up for a quarter, down for a half, up for a quarter.
        assert!(heard[..14].windows(2).all(|w| w[1] > w[0]));
        assert!(heard[17..46].windows(2).all(|w| w[1] < w[0]));
        assert!(heard[49..].windows(2).all(|w| w[1] > w[0]));
        assert!(heard[63].abs() < 0.035, "{}", heard[63]);
    }

    /// The LFO at the top of its triangle, with its level at `level`: a
    /// quarter of a cycle on from its start once it has moved a 1024th.
    fn top_of(level: f64) -> f32 {
        let tail = [26_259, 0, 0, 0, (1 << 30) - (1 << 22)];
        let points = [point(0.0, level, 0)];
        let bytes = stream(2, 3, &points, [1, -1, -1, -1, -1], &tail);
        let lfo = Lfo::new(&read(&bytes, Class::Lfo).unwrap());
        let rate = CYCLE / 1024.0 / lfo.pace(1.0);
        let mut run = lfo.start(rate);
        lfo.advance(&mut run, 1.0, rate);
        run.out
    }

    #[test]
    fn an_lfo_s_level_counts_from_0_5() {
        // FL's articulator: an adding one does `v += l * amount`, a
        // multiplying one `v *= l * amount * 2 + 1`. At amount 1 and a
        // level of 1, the first swings by ±0.5 and the second between 0
        // and 2, never below 0.
        let full = top_of(1.0);
        assert!((full - 0.5).abs() < 1e-3, "{full}");
        let amount = 1.0;
        let (high, low) = (full * amount * 2.0 + 1.0, -full * amount * 2.0 + 1.0);
        assert!(
            (high - 2.0).abs() < 2e-3 && low.abs() < 2e-3,
            "{high} {low}"
        );
        // A level of 0.5 stills it, and one of 0 turns it upside down.
        assert!(top_of(0.5).abs() < 1e-6);
        assert!((top_of(0.0) + 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_run_started_a_tick_early_is_on_its_first_point_after_one_tick() {
        let points = [point(0.0, 0.0, 0), point(1.0, 1.0, 0)];
        let track = Track::new(&envelope(&points, [-1, -1, -1]), 0.0);
        let mut run = track.start_before(0.25);
        let mut heard = Vec::new();
        for _ in 0..3 {
            track.advance(&mut run, 1.0, 0.25);
            heard.push(run.out);
        }
        assert_eq!(heard, [0.0, 0.25, 0.5]);

        // And an LFO is a tick behind its start, where its level waits.
        let mut lfo = lfo([26_259, 0, 0, 0], 1, 0);
        let rate = CYCLE / 64.0 / lfo.pace(1.0);
        let mut run = lfo.start_before(rate);
        lfo.advance(&mut run, 1.0, rate);
        assert!((run.out + 1.0 / 32.0).abs() < 1e-3, "{}", run.out);
        lfo.advance(&mut run, 1.0, rate);
        assert!(run.out.abs() < 1e-3, "{}", run.out);
        // The knobs can be moved under it.
        lfo.set_knobs([21_502, 0, 0, 0]);
        assert!((CYCLE / lfo.pace(1.0) - 32.0).abs() < 0.5);
    }

    #[test]
    fn the_phase_moves_the_start_and_the_tension_bends_the_triangle() {
        // A quarter of a cycle on: the top of the triangle.
        let top = lfo([26_259, 0, 0, 0], 1, 1 << 30);
        let rate = CYCLE / 1024.0 / top.pace(1.0);
        let mut run = top.start(rate);
        top.advance(&mut run, 1.0, rate);
        assert!(run.out > 0.49, "{}", run.out);

        // A tension bends the triangle and keeps its ends.
        let bent = Oscillator::new([64, 0, 0], false);
        let plain = Oscillator::new([0, 0, 0], false);
        assert_eq!(plain.at(0), 1.0);
        assert!((plain.at(1 << 29) - 0.5).abs() < 1e-6);
        assert!(plain.at(1 << 30).abs() < 1e-6);
        assert!(plain.at(1 << 31) < -0.999);
        assert!(bent.at(1 << 29) > plain.at(1 << 29));
        assert!((bent.at(0) - 1.0).abs() < 1e-6);
        // Each half bends the same way round 0.
        assert!((bent.at(1 << 29) + bent.at(3 << 29)).abs() < 1e-6);

        // A pulse width of -128 is a saw that falls for the whole cycle.
        let saw = Oscillator::new([0, 0, -128], false);
        assert!(saw.at(1 << 30) > saw.at(1 << 31));
        assert!(saw.at(1 << 31) > saw.at(3 << 30));
        // With the flag the swing bends as one curve, so it is no longer
        // the same on either side of 0.
        let whole = Oscillator::new([64, 0, 0], true);
        assert!((whole.at(0) - 1.0).abs() < 1e-6);
        assert!((whole.at(1 << 29) + whole.at(3 << 29)).abs() > 0.1);
    }
}
