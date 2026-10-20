//! Automation clips: the curve a clip draws, and the controls the clips of
//! a song drive.
//!
//! `docs/automation-234.md` is the source for the curve. A clip's points
//! are [`flp::event::layout::Point`]s. Each one ends a segment and says how
//! the segment gets from the point before to itself: one of 13 modes and a
//! tension. [`Curve`] is those segments as a function of beats.
//!
//! A clip plays where the playlist has an item for its channel, from the
//! beat the item's trim starts at. What it drives comes from
//! [`model::Project::automation_bindings`]. The engine plays the bindings
//! to an insert's volume, pan and send levels, to the volume and pan of a
//! sampler or audio clip channel, and to the tempo. Of an effect slot whose
//! plugin the engine hosts it plays the bindings to the slot's switch, to
//! its mix level and to the parameters the plugin has. Of a plugin channel
//! whose generator the engine hosts it plays the bindings to the channel's
//! volume and pan and to the parameters the generator has. A control that
//! a clip has driven stays where the clip left it, and before any clip has
//! started it is what the project says.
//!
//! A plugin's parameter `n` is control `0x8000 + n` of its slot, or of its
//! channel for a generator. The clip's
//! 0 to 1 covers the part of the parameter's range that the channel's 219
//! gives, in a straight line, as for any other control. The range is the
//! one the plugin keeps the parameter in.
//!
//! The engine reads the controls on every 512th sample of the song and on
//! the sample a note or an audio clip starts on, and they stay as read
//! until the next of those. The tempo is read on every 512th sample only.
//! The engine's clock is built with it, and the hosted plugins are told it
//! where it changes.
//!
//! Only the envelope of a clip is played. Three corpus clips have their
//! LFO switched on as well, and `docs/automation-234.md` has no formula for
//! it.
//!
//! # Controllers
//!
//! A 227 link whose source is an output of a plugin in an effect slot is
//! driven by that plugin: Fruity Peak Controller's Peak + LFO, Peak and LFO
//! are controls `0x8000`, `0x8001` and `0x8002` of its slot
//! (`docs/plugins/fruity-peak-controller.md`, section 4). The plugin works
//! its outputs out once a tick of the song, and where one has moved, every
//! link from it sets its control: the output's 0 to 1 goes through the
//! link's formula when it has one, and then covers the whole of the
//! control's range in a straight line. That is FL's REC dispatcher taking a
//! value in 2^30ths, as it does for a clip. The Peak Controller counts in
//! 2^30ths already, so the 16384 that FL's host multiplies an older
//! controller's value by is not on it.
//!
//! The controls are the ones a clip can drive, but for the tempo, which
//! the clock is built from before the song plays. A control that a
//! controller has set stays the controller's: a clip on the same control
//! no longer moves it, where FL would give it to whichever wrote last. No
//! project FL ships has both on one control. A link with bit 1 of its flags
//! set goes through a smoothed setter in FL that the doc has not read, and
//! is set at once here.

use crate::formula::Formula;
use crate::generator;
use crate::mix::{self, BLOCK, Mixer};
use crate::sequence::Song;
use crate::voice::{self, Voice};
use crate::{Curves, clock::Clock};
use flp::event::layout::Point;
use model::rec::{ControllerOutput, control};
use model::{Binding, ChannelKind, LinkKind, Project, RecTarget};
use std::collections::BTreeSet;
use std::f64::consts::{FRAC_PI_2, TAU};

/// The points an automation clip draws, as a value for every beat.
///
/// The value is in the clip's own 0..=1. A segment runs from the point
/// before up to, and not including, the point that ends it, which is where
/// the next one takes over. So a hold segment jumps on the beat of its end
/// point.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Curve {
    segments: Vec<Segment>,
    /// The last point's value, and 0 for a curve without points.
    last: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    /// Beats from the start of the clip.
    start: f64,
    end: f64,
    /// The values of the point before and of the point that ends it.
    from: f64,
    to: f64,
    shape: Shape,
}

impl Curve {
    /// The curve through these points. A point's `x` is the beats since
    /// the point before, and its mode and tension shape the segment that
    /// ends at it.
    ///
    /// FL's loader reads a negative `x` as 0 and a `y` that is not a number
    /// as the `y` before it, and so does this.
    pub fn new(points: &[Point]) -> Self {
        let mut clean: Vec<Point> = Vec::with_capacity(points.len());
        for point in points {
            let before = clean.last().map_or(0.0, |p| p.y);
            clean.push(Point {
                x: if point.x > 0.0 { point.x } else { 0.0 },
                y: if point.y.is_finite() { point.y } else { before },
                ..*point
            });
        }
        let mut at = clean.first().map_or(0.0, |p| p.x);
        let mut segments = Vec::with_capacity(clean.len().saturating_sub(1));
        for (i, point) in clean.iter().enumerate().skip(1) {
            segments.push(Segment {
                start: at,
                end: at + point.x,
                from: clean[i - 1].y,
                to: point.y,
                shape: Shape::new(&clean, i),
            });
            at += point.x;
        }
        Curve {
            segments,
            last: clean.last().map_or(0.0, |p| p.y),
        }
    }

    /// The curve's value `beats` into the clip: the first point's value
    /// before the first point, and the last point's from the last point
    /// on. This allocates nothing.
    pub fn value_at(&self, beats: f64) -> f64 {
        let over = self.segments.partition_point(|s| s.end <= beats);
        let Some(segment) = self.segments.get(over) else {
            return self.last;
        };
        // A segment that ends after `beats` and has started is longer than
        // nothing.
        let t = (beats - segment.start) / (segment.end - segment.start);
        if t > 0.0 {
            segment.from + segment.shape.at(t) * (segment.to - segment.from)
        } else {
            segment.from
        }
    }
}

/// ln 1000001, the exponent FL's curves are built on.
const L: f64 = 13.815_511_557_963_774;
/// 2^-24, FL's "too small to divide by".
const EPSILON: f64 = 5.960_464_477_539_063e-8;

/// How a segment gets from 0 at its start to 1 at its end, with what FL
/// works out per point after loading (`FUN_00b9ba60`) worked out already.
/// The names are FL's.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// A straight line. FL draws one for a tension of exactly 0 in every
    /// mode that only bends, and for a smooth segment between two equal
    /// values.
    Line,
    /// Stays at the start. Mode 2, and any mode past 12.
    Hold,
    /// Modes 0 and 1. `k` is the tension's size times [`L`].
    SingleCurve {
        k: f64,
        up: bool,
    },
    DoubleCurve {
        k: f64,
        up: bool,
    },
    /// Mode 3: `n` treads, and `side` is 1 for a tension from 0 up and -1
    /// below.
    Stairs {
        n: f64,
        side: f64,
    },
    /// Mode 4.
    SmoothStairs {
        n: f64,
        rounded: bool,
    },
    /// Mode 5.
    Pulse {
        n: f64,
        side: f64,
    },
    /// Mode 6: `periods` of a sine for a tension below 0 and of a triangle
    /// from 0 up.
    Wave {
        periods: f64,
        sine: bool,
    },
    /// Modes 7 and 8.
    SingleCurve2 {
        tension: f64,
    },
    DoubleCurve2 {
        tension: f64,
    },
    /// Mode 9.
    HalfSine {
        tension: f64,
    },
    /// Mode 10: a cubic with the slope `into` at its start and `out` at
    /// its end, mixed with the straight line by `amount`.
    Smooth {
        into: f64,
        out: f64,
        amount: f64,
    },
    /// Modes 11 and 12.
    SingleCurve3 {
        a: f64,
        b: f64,
        c: f64,
        up: bool,
    },
    DoubleCurve3 {
        a: f64,
        b: f64,
        c: f64,
        up: bool,
    },
}

/// `(e^(ku) - 1) / (e^k - 1)`, the bend of modes 0 and 1.
fn bend(k: f64, u: f64) -> f64 {
    (k * u).exp_m1() / k.exp_m1()
}

/// The bend of modes 0 and 1 for either sign of the tension.
fn single_curve(k: f64, up: bool, t: f64) -> f64 {
    if up {
        1.0 - bend(k, 1.0 - t)
    } else {
        bend(k, t)
    }
}

/// Mode 7.
fn single_curve_2(tension: f64, t: f64) -> f64 {
    let u = if tension > 0.0 { 1.0 - t } else { t };
    t + tension * (u - u.powi(6))
}

/// Mode 11.
fn single_curve_3(a: f64, b: f64, c: f64, up: bool, t: f64) -> f64 {
    b / (1.0 - a * if up { 1.0 - t } else { t }) - c
}

/// A double curve: `half` over the first half of the segment and its
/// mirror image over the second.
fn doubled(t: f64, half: impl Fn(f64) -> f64) -> f64 {
    if t < 0.5 {
        0.5 * half(2.0 * t)
    } else {
        1.0 - 0.5 * half(2.0 * (1.0 - t))
    }
}

/// FL's step count for modes 3 to 6 (`FUN_00b9d450`): 1000 at no tension,
/// down to 0 at full tension either way.
fn steps(tension: f64) -> f64 {
    let scaled = (L * (1.0 - 0.6 * tension.abs())).exp();
    ((scaled - 1.0) / 1000.0).round_ties_even()
}

/// The slope a smooth segment has where it meets a neighbour: the weighted
/// harmonic mean of the two segments' slopes, which keeps the curve from
/// overshooting. The segment itself has slope 1 and length 1, and `slope`
/// and `len` are the neighbour's on that scale. Flat where the two slopes
/// do not point the same way.
fn tangent(slope: f64, len: f64) -> f64 {
    if slope > 0.0 {
        let (theirs, ours) = (2.0 * len + 1.0, len + 2.0);
        slope * (theirs + ours) / (theirs * slope + ours)
    } else {
        0.0
    }
}

impl Shape {
    /// The shape of the segment that ends at `points[i]`, for `i` from 1.
    fn new(points: &[Point], i: usize) -> Self {
        let point = &points[i];
        let tension = f64::from(point.tension);
        let (size, up) = (tension.abs(), tension > 0.0);
        let side = if tension >= 0.0 { 1.0 } else { -1.0 };
        if tension == 0.0 && matches!(point.mode, 0 | 1 | 7 | 8 | 9 | 11 | 12) {
            return Shape::Line;
        }
        match point.mode {
            0 => Shape::SingleCurve { k: size * L, up },
            1 => Shape::DoubleCurve { k: size * L, up },
            3 => Shape::Stairs {
                n: steps(tension) + if tension >= 0.0 { 2.0 } else { 1.0 },
                side,
            },
            4 => Shape::SmoothStairs {
                n: steps(tension) + 1.0,
                rounded: tension < 0.0,
            },
            5 => Shape::Pulse {
                n: 2.0 * (steps(tension) + 1.0),
                side,
            },
            6 if up && steps(tension) == 0.0 => Shape::Line,
            6 => Shape::Wave {
                periods: steps(tension) + 0.5,
                sine: tension < 0.0,
            },
            7 => Shape::SingleCurve2 { tension },
            8 => Shape::DoubleCurve2 { tension },
            9 => Shape::HalfSine { tension },
            10 => Self::smooth(points, i),
            11 | 12 => {
                let reach = (0.9 * size).max(1.525_878_906_25e-5);
                let a = 1.0 - (1.0 - reach).powi(3);
                let b = (1.0 - a) / a;
                let (b, c) = if up { (-b, -b - 1.0) } else { (b, b) };
                if point.mode == 11 {
                    Shape::SingleCurve3 { a, b, c, up }
                } else {
                    Shape::DoubleCurve3 { a, b, c, up }
                }
            }
            _ => Shape::Hold,
        }
    }

    /// Mode 10, from the loader's precompute at 0xb9be64. The slopes at
    /// the two ends come from the neighbouring segments when those are
    /// smooth as well, and are flat when they are not.
    fn smooth(points: &[Point], i: usize) -> Self {
        let (from, to) = (points[i - 1].y, points[i].y);
        let rise = to - from;
        if rise.abs() < EPSILON {
            return Shape::Line;
        }
        let len = points[i].x.max(EPSILON);
        // A neighbour's length and its rise, both in units of this segment.
        let (before_len, before_rise) = if points[i - 1].mode == 10 {
            let start = i.checked_sub(2).map_or(from, |j| points[j].y);
            (points[i - 1].x / len, (from - start) / rise)
        } else {
            (1.0, 0.0)
        };
        let (after_len, after_rise) = match points.get(i + 1) {
            Some(next) if next.mode == 10 => (next.x / len, (next.y - to) / rise),
            _ => (1.0, 0.0),
        };
        let (before_len, after_len) = (before_len.max(EPSILON), after_len.max(EPSILON));
        Shape::Smooth {
            into: tangent(before_rise / before_len, before_len),
            out: tangent(after_rise / after_len, after_len),
            amount: 1.0 - f64::from(points[i].tension).abs(),
        }
    }

    /// How far along the segment's rise it is at `t`, for `t` above 0 and
    /// below 1.
    fn at(self, t: f64) -> f64 {
        match self {
            Shape::Line => t,
            Shape::Hold => 0.0,
            Shape::SingleCurve { k, up } => single_curve(k, up, t),
            Shape::DoubleCurve { k, up } => doubled(t, |u| single_curve(k, up, u)),
            Shape::Stairs { n, side } => (t * n - 0.5 * side).round_ties_even() / (n - side),
            Shape::SmoothStairs { n, rounded } => {
                let s = t * n - 0.5;
                let tread = (s - 0.5).round_ties_even();
                let u = 2.0 * (s - tread) - 1.0;
                let w = if rounded {
                    u.signum() * (1.0 - (1.0 - u.abs()).powi(3))
                } else {
                    u.powi(3)
                };
                (tread + 0.5 * w + 1.0) / n
            }
            Shape::Pulse { n, side } => (t * n - 0.5 * side).round_ties_even().rem_euclid(2.0),
            Shape::Wave {
                periods,
                sine: true,
            } => ((TAU * (t * periods + 0.75)).sin() + 1.0) / 2.0,
            Shape::Wave { periods, .. } => {
                let s = t * periods - 0.5;
                (2.0 * (s - (s - 0.5).round_ties_even()) - 1.0).abs()
            }
            Shape::SingleCurve2 { tension } => single_curve_2(tension, t),
            Shape::DoubleCurve2 { tension } => doubled(t, |u| single_curve_2(tension, u)),
            Shape::HalfSine { tension } => {
                let quarter = if tension > 0.0 {
                    (t * FRAC_PI_2).sin()
                } else {
                    1.0 - (t * FRAC_PI_2).cos()
                };
                (1.0 - tension.abs()) * t + tension.abs() * quarter
            }
            Shape::Smooth { into, out, amount } => {
                let cubic = 3.0 * t * t - 2.0 * t.powi(3)
                    + out * t * t * (t - 1.0)
                    + into * t * (t - 1.0) * (t - 1.0);
                t + amount * (cubic - t)
            }
            Shape::SingleCurve3 { a, b, c, up } => single_curve_3(a, b, c, up, t),
            Shape::DoubleCurve3 { a, b, c, up } => doubled(t, |u| single_curve_3(a, b, c, up, u)),
        }
    }
}

/// A control of the song that a clip can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    /// Of the insert with this loader number.
    InsertVolume(u16),
    InsertPan(u16),
    /// The level of the send from one insert to another.
    Send {
        from: u16,
        to: u16,
    },
    /// Of the channel whose 64 has this index.
    ChannelVolume(u16),
    ChannelPan(u16),
    Tempo,
    /// Whether this slot of this insert is switched on.
    SlotEnabled {
        insert: u16,
        slot: u8,
    },
    /// The slot's mix level.
    SlotMix {
        insert: u16,
        slot: u8,
    },
    /// Parameter `index` of the slot's plugin.
    Param {
        insert: u16,
        slot: u8,
        index: u16,
    },
    /// Parameter `index` of the generator of the channel whose 64 has this
    /// index.
    ChannelParam {
        channel: u16,
        index: u16,
    },
}

/// The control number of the first parameter of a slot's plugin, and of a
/// channel's. The others follow.
const PARAM: u16 = 0x8000;

/// One item that shows a clip on the playlist.
#[derive(Debug, Clone, Copy)]
struct Shown {
    /// Ticks from the start of the song.
    start: f64,
    /// Ticks.
    length: f64,
    /// Beats into the clip where the item starts.
    offset: f64,
}

/// An automation clip that drives a control, and where it plays.
struct Clip {
    curve: Curve,
    /// In the order they start.
    items: Vec<Shown>,
}

impl Clip {
    /// The value the clip last gave by `tick`, and the tick it gave it at:
    /// `tick` itself inside an item, and the item's end after it. `None`
    /// before the first item. Where two items overlap, the one that
    /// started later counts.
    fn at(&self, tick: f64, ppq: f64) -> Option<(f64, f64)> {
        let started = self.items.partition_point(|item| item.start <= tick);
        let item = self.items.get(started.checked_sub(1)?)?;
        let into = (tick - item.start).min(item.length);
        let value = self.curve.value_at(item.offset + into / ppq);
        Some((item.start + into, value))
    }
}

/// A clip that drives a lane, and the raw values of the control at the
/// clip's 0 and its 1.
struct Source {
    clip: usize,
    min: f64,
    max: f64,
}

/// One automated control and the clips that drive it.
struct Lane {
    control: Control,
    /// The control's raw value before any clip has started.
    initial: f64,
    /// The lowest and the highest raw value the control takes.
    range: (f64, f64),
    sources: Vec<Source>,
}

impl Lane {
    /// The control's raw value at `tick`: what the clip that wrote last
    /// gave it, kept inside the control's range.
    fn raw_at(&self, clips: &[Clip], tick: f64, ppq: f64) -> f64 {
        let mut last: Option<(f64, f64)> = None;
        for source in &self.sources {
            let Some((written, value)) = clips[source.clip].at(tick, ppq) else {
                continue;
            };
            if last.is_none_or(|(at, _)| written >= at) {
                last = Some((written, source.min + value * (source.max - source.min)));
            }
        }
        last.map_or(self.initial, |(_, raw)| {
            raw.clamp(self.range.0, self.range.1)
        })
    }
}

/// A link from an output of a controller plugin to a control.
struct Link {
    source: ControllerOutput,
    control: Control,
    /// The lowest and the highest raw value the control takes: where the
    /// output's 0 and its 1 land.
    range: (f64, f64),
    formula: Option<Formula>,
    /// The raw value the controller last gave the control. `None` until
    /// it has given one, and the control is the project's until then.
    raw: Option<f64>,
    /// The controller has moved since the control was last set.
    moved: bool,
}

impl Link {
    /// The control's raw value for an output from 0 to 1.
    fn raw_at(&self, output: f64) -> f64 {
        let value = match &self.formula {
            Some(formula) => formula.at(output),
            None => output.clamp(0.0, 1.0),
        };
        self.range.0 + value * (self.range.1 - self.range.0)
    }
}

/// Put a control of the mixer at a raw value, or note it in `levels` for a
/// channel's volume and pan. The tempo is not set this way: the clock has
/// it.
fn set(control: Control, raw: f64, levels: &mut [Levels], mixer: &mut Mixer, curves: &dyn Curves) {
    let raw = raw.round();
    match control {
        Control::InsertVolume(insert) => {
            mixer.set_volume(insert, curves.insert_gain(raw as i32));
        }
        Control::InsertPan(insert) => {
            let raw = raw as i32;
            mixer.set_pan(insert, curves.insert_pan(raw), curves.insert_cross(raw));
        }
        Control::Send { from, to } => {
            mixer.set_send(from, to, curves.send_gain(raw as i32));
        }
        Control::ChannelVolume(channel) | Control::ChannelPan(channel) => {
            let levels = levels.iter_mut().find(|l| l.channel == channel);
            match (levels, control) {
                (Some(levels), Control::ChannelVolume(_)) => levels.volume = raw as u32,
                (Some(levels), _) => levels.pan = raw as u32,
                (None, _) => {}
            }
        }
        Control::Tempo => {}
        Control::SlotEnabled { insert, slot } => {
            mixer.set_slot_enabled(insert, slot, raw > 0.0);
        }
        Control::SlotMix { insert, slot } => {
            mixer.set_slot_mix(insert, slot, curves.slot_mix(raw as i32));
        }
        Control::Param {
            insert,
            slot,
            index,
        } => mixer.set_param(insert, slot, usize::from(index), raw as f32),
        Control::ChannelParam { channel, index } => {
            mixer.set_channel_param(channel, usize::from(index), raw as f32);
        }
    }
}

/// The volume and pan of a channel that has one of them automated, as the
/// channel's 219 stores them.
struct Levels {
    channel: u16,
    volume: u32,
    pan: u32,
    /// The channel's [`voice::adjust`], which no clip drives.
    adjust: i32,
}

/// An automation channel that is on the playlist.
struct Placed {
    /// The tick its first item starts at.
    first: u64,
    /// Whether it drives a control the engine plays.
    plays: bool,
    /// Its links to controls of an effect slot that the engine plays.
    slots: usize,
    /// Its links to controls the engine does not play, or 1 when it has no
    /// link at all.
    unsupported: usize,
}

/// What [`Automation::counts`] counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// The clips that drive a control the engine plays.
    pub clips: usize,
    /// The links from those clips to a control of an effect slot: its
    /// switch, its mix level or a parameter of its plugin.
    pub slot_targets: usize,
    /// What the engine does not play: one for each link to such a control
    /// and one for each clip without a link.
    pub unsupported: usize,
}

/// The automation of one song: every clip that drives a control the engine
/// plays, what each of those controls is at a tick, and every link from a
/// controller plugin to such a control. Building it allocates.
/// [`Automation::apply`], [`Automation::follow`], [`Automation::drive`] and
/// [`Automation::carry`] do not.
pub(crate) struct Automation {
    ppq: f64,
    clips: Vec<Clip>,
    lanes: Vec<Lane>,
    levels: Vec<Levels>,
    placed: Vec<Placed>,
    /// The links from a controller plugin to a control the engine plays.
    links: Vec<Link>,
    /// The 227 links whose source is an output of a plugin the engine
    /// hosts, whatever they drive.
    pub controller_links: usize,
    /// Those among them the engine does not play: the ones to a control it
    /// does not play or to the tempo, and the ones with a formula it does
    /// not read.
    pub unsupported_controller_targets: usize,
    /// A link has an output that has not been carried to its control.
    moved: bool,
    /// The output sample the controls were last read at.
    read_at: Option<usize>,
}

impl Automation {
    /// `tempo` is the project's, in BPM. An automation channel that is
    /// switched off drives nothing and is not counted.
    pub fn new(project: &Project, song: &Song, tempo: f64) -> Self {
        let mut automation = Automation {
            ppq: f64::from(project.header.ppq),
            clips: Vec::new(),
            lanes: Vec::new(),
            levels: Vec::new(),
            placed: Vec::new(),
            links: Vec::new(),
            controller_links: 0,
            unsupported_controller_targets: 0,
            moved: false,
            read_at: None,
        };
        let bindings = project.automation_bindings();
        let channels: BTreeSet<u16> = song.automation.iter().map(|item| item.channel).collect();
        for index in channels {
            let Some(channel) = project.channel(index) else {
                continue;
            };
            if channel.enabled == Some(false) {
                continue;
            }
            let mine = song.automation.iter().filter(|item| item.channel == index);
            let mut placed = Placed {
                first: mine.clone().next().map_or(0, |item| item.tick),
                plays: false,
                slots: 0,
                unsupported: 0,
            };
            let items: Vec<Shown> = mine
                .map(|item| Shown {
                    start: item.tick as f64,
                    length: item.length as f64,
                    offset: f64::from(item.offset),
                })
                .collect();
            let bound = bindings.iter().filter(|b| b.channel == index);
            // A clip whose 234 did not decode has no curve to play.
            let curve = channel.automation.as_ref().map(|d| Curve::new(&d.envelope));
            for binding in bound.clone() {
                // A control of a slot that holds no plugin changes nothing:
                // the slot passes its input on whatever it is set to.
                if let RecTarget::Insert {
                    index,
                    slot: Some(slot),
                    ..
                } = binding.target
                    && empty_slot(project, index, slot)
                {
                    continue;
                }
                let lane = automation.lane(project, binding, tempo);
                match (lane, curve.is_some()) {
                    (Some((lane, min, max)), true) => {
                        let clip = automation.clips.len();
                        automation.lanes[lane]
                            .sources
                            .push(Source { clip, min, max });
                        placed.plays = true;
                        let slot =
                            matches!(binding.target, RecTarget::Insert { slot: Some(_), .. });
                        placed.slots += usize::from(slot);
                    }
                    _ => placed.unsupported += 1,
                }
            }
            placed.unsupported += usize::from(bound.count() == 0);
            if let (Some(curve), true) = (curve, placed.plays) {
                automation.clips.push(Clip { curve, items });
            }
            automation.placed.push(placed);
        }
        automation.link(project, tempo);
        automation
    }

    /// Find the links a controller plugin drives. The source has to be an
    /// output the plugin in that slot has, and the plugin one the mixer
    /// hosts.
    fn link(&mut self, project: &Project, tempo: f64) {
        let remote = project.controller_links.iter();
        for link in remote.filter(|link| link.kind == LinkKind::Remote) {
            let Some(ids) = link.link else {
                continue;
            };
            let Some(source) = project.rec_target(ids.source).controller_output() else {
                continue;
            };
            let outputs = mix::hosted(project, source.insert, source.slot)
                .map_or(0, |plugin| plugin.controller_outputs().len());
            if usize::from(source.output) >= outputs {
                continue;
            }
            self.controller_links += 1;
            // A formula of nothing but spaces is no formula.
            let text = link.formula.as_deref().map(str::trim);
            let formula = match text.filter(|text| !text.is_empty()) {
                Some(text) => Formula::compile(text).map(Some),
                None => Some(None),
            };
            let driven = formula.and_then(|formula| {
                let target = project.rec_target(ids.target);
                let (control, _, range) = self.control(project, target, tempo)?;
                (control != Control::Tempo).then_some((control, range, formula))
            });
            let Some((control, (low, high), formula)) = driven else {
                self.unsupported_controller_targets += 1;
                continue;
            };
            self.links.push(Link {
                source,
                control,
                range: (f64::from(low), f64::from(high)),
                formula,
                raw: None,
                moved: false,
            });
        }
    }

    /// The links from a controller plugin that the engine plays.
    pub fn controller_targets(&self) -> usize {
        self.links.len()
    }

    /// The inserts with a controller plugin that drives one of those
    /// links. A mixer hosts them whether sound reaches them or not.
    pub fn controller_inserts(&self) -> BTreeSet<u16> {
        let inserts = self.links.iter().map(|link| link.source.insert);
        inserts.collect()
    }

    /// Take the outputs of the controller in a slot of an insert, from 0
    /// to 1, as its tick left them. [`Automation::carry`] sets the
    /// controls they drive.
    pub fn drive(&mut self, insert: u16, slot: u8, outputs: &[f32]) {
        for link in &mut self.links {
            if (link.source.insert, link.source.slot) != (insert, slot) {
                continue;
            }
            let Some(output) = outputs.get(usize::from(link.source.output)) else {
                continue;
            };
            let raw = link.raw_at(f64::from(*output));
            if link.raw != Some(raw) {
                link.raw = Some(raw);
                link.moved = true;
                self.moved = true;
            }
        }
    }

    /// Set every control whose controller has moved since the last call,
    /// from the next block on.
    pub fn carry(&mut self, mixer: &mut Mixer, curves: &dyn Curves) {
        if !std::mem::take(&mut self.moved) {
            return;
        }
        for link in self.links.iter_mut().filter(|link| link.moved) {
            link.moved = false;
            if let Some(raw) = link.raw {
                set(link.control, raw, &mut self.levels, mixer, curves);
            }
        }
        self.gains(mixer, curves);
    }

    /// Put the volume and the pan of every automated channel with a
    /// generator on what the generator plays. A voice gets its own in
    /// [`Automation::follow`].
    fn gains(&self, mixer: &mut Mixer, curves: &dyn Curves) {
        for levels in &self.levels {
            let level = curves.volume_adjust_gain(levels.adjust);
            let gains = voice::gains(levels.volume, levels.pan, level, 0.0, curves);
            mixer.set_channel_gains(levels.channel, gains);
        }
    }

    /// The control a REC ID names, what the project has it at, and its
    /// lowest and highest raw value. `None` for a control the engine does
    /// not play. A channel's volume or pan gets its place in `levels`
    /// here.
    fn control(
        &mut self,
        project: &Project,
        target: RecTarget,
        tempo: f64,
    ) -> Option<(Control, i32, (i32, i32))> {
        Some(match target {
            RecTarget::Insert {
                index: insert,
                slot: Some(slot),
                control,
            } => Self::slot_control(project, insert, slot, control)?,
            RecTarget::Insert {
                index,
                slot: None,
                control,
            } => {
                let strip = project.insert_mix(index);
                let (found, initial) = match control {
                    control::VOLUME => (Control::InsertVolume(index), strip.volume_raw()),
                    control::PAN => (Control::InsertPan(index), strip.pan_raw()),
                    send => {
                        let to = send.checked_sub(control::SEND)?;
                        let send = Control::Send { from: index, to };
                        (send, strip.send_level_raw(to)?)
                    }
                };
                (found, initial, target.raw_range()?)
            }
            RecTarget::Channel { index, control } if control >= PARAM => {
                let mut plugin = generator::hosted(project.channel(index)?, None, None)?;
                let param = control - PARAM;
                let initial = plugin.param(usize::from(param)) as i32;
                let range = generator::param_range(plugin.as_mut(), usize::from(param))?;
                let channel = index;
                let index = param;
                (Control::ChannelParam { channel, index }, initial, range)
            }
            RecTarget::Channel { index, control } => {
                let range = target.raw_range()?;
                let channel = project.channel(index)?;
                let played = [ChannelKind::Sampler, ChannelKind::AudioClip];
                let hosted = || generator::hosted(channel, None, None).is_some();
                if !channel.kind.is_some_and(|kind| played.contains(&kind)) && !hosted() {
                    return None;
                }
                let (volume, pan) = voice::levels(channel);
                if !self.levels.iter().any(|l| l.channel == index) {
                    self.levels.push(Levels {
                        channel: index,
                        volume,
                        pan,
                        adjust: voice::adjust(project, channel),
                    });
                }
                match control {
                    control::CHANNEL_VOLUME => {
                        (Control::ChannelVolume(index), volume as i32, range)
                    }
                    _ => (Control::ChannelPan(index), pan as i32, range),
                }
            }
            RecTarget::Global { .. } => {
                let range = target.raw_range()?;
                (Control::Tempo, (tempo * 1000.0).round() as i32, range)
            }
            RecTarget::Other(_) => return None,
        })
    }

    /// The place in `lanes` of the control a binding drives, with the
    /// control's raw value at the clip's 0 and 1. Makes the lane when the
    /// control has none. `None` for a control the engine does not play.
    fn lane(
        &mut self,
        project: &Project,
        binding: &Binding,
        tempo: f64,
    ) -> Option<(usize, f64, f64)> {
        let (found, initial, (low, high)) = self.control(project, binding.target, tempo)?;
        // The model has the two ends for the controls it knows the range
        // of. A slot's are worked out here the same way.
        let (min, max) = match binding.min_raw.zip(binding.max_raw) {
            Some(ends) => ends,
            None => {
                let (min, max) = project.channel(binding.channel)?.automation_range();
                (raw_at(low, high, min), raw_at(low, high, max))
            }
        };
        let lane = self.lanes.iter().position(|lane| lane.control == found);
        let lane = lane.unwrap_or_else(|| {
            self.lanes.push(Lane {
                control: found,
                initial: f64::from(initial),
                range: (f64::from(low), f64::from(high)),
                sources: Vec::new(),
            });
            self.lanes.len() - 1
        });
        Some((lane, min, max))
    }

    /// A control of an effect slot, what the project has it at, and its
    /// lowest and highest raw value. `None` for a slot whose plugin the
    /// engine does not host, for a parameter the plugin does not have, and
    /// for a control that is none of a slot's.
    fn slot_control(
        project: &Project,
        insert: u16,
        slot: u8,
        control: u16,
    ) -> Option<(Control, i32, (i32, i32))> {
        let mut plugin = mix::hosted(project, insert, slot)?;
        let strip = project.insert_mix(insert);
        Some(match control {
            control::SLOT_ENABLED => {
                let enabled = Control::SlotEnabled { insert, slot };
                (enabled, i32::from(strip.slot_enabled(slot)?), (0, 1))
            }
            control::SLOT_MIX => {
                let mix = Control::SlotMix { insert, slot };
                (mix, strip.slot_mix_raw(slot)?, (0, 12_800))
            }
            param => {
                let index = param.checked_sub(PARAM)?;
                let initial = plugin.param(usize::from(index)) as i32;
                let range = mix::param_range(plugin.as_mut(), usize::from(index))?;
                let param = Control::Param {
                    insert,
                    slot,
                    index,
                };
                (param, initial, range)
            }
        })
    }

    /// The tempo at `tick` in BPM, when a clip drives the tempo.
    pub fn tempo_at(&self, tick: f64) -> Option<f64> {
        let lane = self.lanes.iter().find(|l| l.control == Control::Tempo)?;
        Some(lane.raw_at(&self.clips, tick, self.ppq) / 1000.0)
    }

    /// The slowest tempo a clip can take the song to, in BPM, when a clip
    /// drives the tempo: the bottom of the tempo's range.
    pub fn slowest_tempo(&self) -> Option<f64> {
        let lane = self.lanes.iter().find(|l| l.control == Control::Tempo)?;
        Some(lane.range.0 / 1000.0)
    }

    /// The sends whose level a clip or a controller drives, as sender and
    /// destination.
    pub fn sends(&self) -> BTreeSet<(u16, u16)> {
        let lanes = self.lanes.iter().map(|lane| lane.control);
        let links = self.links.iter().map(|link| link.control);
        let sends = lanes.chain(links).filter_map(|control| match control {
            Control::Send { from, to } => Some((from, to)),
            _ => None,
        });
        sends.collect()
    }

    /// Whether a clip drives the volume or the pan of this channel.
    pub fn follows(&self, channel: u16) -> bool {
        self.levels.iter().any(|l| l.channel == channel)
    }

    /// The counts over the clips that start before output sample `limit`.
    pub fn counts(&self, clock: &Clock, limit: usize) -> Counts {
        let inside = |placed: &&Placed| clock.sample(placed.first) < limit;
        let placed = self.placed.iter().filter(inside);
        placed.fold(Counts::default(), |counts, placed| Counts {
            clips: counts.clips + usize::from(placed.plays),
            slot_targets: counts.slot_targets + placed.slots,
            unsupported: counts.unsupported + placed.unsupported,
        })
    }

    /// Set every automated control of the mixer to what it is at output
    /// sample `sample`, and note the levels of the automated channels,
    /// which the generators among them get here. A second call for the
    /// same sample does nothing. A control that a controller plugin has set
    /// goes back to what the controller gave it.
    ///
    /// The mixer's plugins get the tempo of the clock's cell that `sample`
    /// is in, which is the tempo the clock was built with there.
    pub fn apply(&mut self, sample: usize, clock: &Clock, mixer: &mut Mixer, curves: &dyn Curves) {
        if self.read_at == Some(sample) {
            return;
        }
        self.read_at = Some(sample);
        let tick = clock.tick_at(sample);
        for lane in &self.lanes {
            let raw = lane.raw_at(&self.clips, tick, self.ppq);
            match lane.control {
                // The clock has the tempo in it already. The plugins that
                // keep time with it have yet to hear.
                Control::Tempo => {
                    let cell = clock.tick_at(sample - sample % BLOCK);
                    mixer.set_tempo(lane.raw_at(&self.clips, cell, self.ppq) / 1000.0);
                }
                control => set(control, raw, &mut self.levels, mixer, curves),
            }
        }
        // A control a controller has set is the controller's.
        for link in &self.links {
            if let Some(raw) = link.raw {
                set(link.control, raw, &mut self.levels, mixer, curves);
            }
        }
        self.gains(mixer, curves);
    }

    /// Give every voice on an automated channel the gains that go with
    /// the channel's volume and pan as [`Automation::apply`] left them.
    pub fn follow(&self, voices: &mut [Voice], curves: &dyn Curves) {
        if self.levels.is_empty() {
            return;
        }
        for voice in voices {
            let Some(own) = voice.follows() else {
                continue;
            };
            if let Some(levels) = self.levels.iter().find(|l| l.channel == own.channel) {
                let gains = voice::gains(levels.volume, levels.pan, own.level, own.pan, curves);
                voice.set_gains(gains);
            }
        }
    }
}

/// The raw value of a control whose range is `low` to `high` at `part` of
/// an automation clip's 12800 steps. A plugin that leaves a parameter
/// unbounded has the whole i32 for its range, wider than an i32 holds, so
/// the width is taken in f64.
fn raw_at(low: i32, high: i32, part: i32) -> f64 {
    f64::from(low) + f64::from(part) / 12_800.0 * (f64::from(high) - f64::from(low))
}

/// Whether a slot of an insert holds no plugin.
fn empty_slot(project: &Project, insert: u16, slot: u8) -> bool {
    let slots = project
        .insert_mix(insert)
        .insert()
        .map(|insert| &insert.slots);
    let found = slots.and_then(|slots| slots.iter().find(|s| s.index == i32::from(slot)));
    found
        .and_then(|slot| slot.plugin_name.as_deref())
        .is_none_or(str::is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_as_wide_as_an_i32_does_not_overflow() {
        let (low, high) = (i32::MIN, i32::MAX);
        assert_eq!(raw_at(low, high, 0), f64::from(low));
        assert_eq!(raw_at(low, high, 12_800), f64::from(high));
        assert_eq!(raw_at(low, high, 6_400), -0.5);
        assert_eq!(raw_at(0, 100, 3_200), 25.0);
    }

    fn point(x: f64, y: f64, mode: u8, tension: f32) -> Point {
        Point {
            mode,
            tension,
            ..Point::new(x, y)
        }
    }

    /// One segment of this mode and tension, from 0 at beat 0 to 1 at beat
    /// 1, read at `t`.
    fn shape(mode: u8, tension: f32, t: f64) -> f64 {
        Curve::new(&[Point::new(0.0, 0.0), point(1.0, 1.0, mode, tension)]).value_at(t)
    }

    fn close(got: f64, want: f64) -> bool {
        (got - want).abs() < 1e-9
    }

    #[test]
    fn every_mode_starts_at_the_point_before_and_ends_at_its_own() {
        for mode in 0..=13 {
            for tension in [-1.0, -0.5, 0.0, 0.3, 1.0] {
                let at = format!("mode {mode} tension {tension}");
                assert_eq!(shape(mode, tension, 0.0), 0.0, "{at}");
                assert_eq!(shape(mode, tension, 1.0), 1.0, "{at}");
                assert_eq!(shape(mode, tension, 7.0), 1.0, "{at}");
                assert_eq!(shape(mode, tension, -1.0), 0.0, "{at}");
                // And it stays a number in between.
                for step in 1..200 {
                    let value = shape(mode, tension, f64::from(step) / 200.0);
                    assert!((-0.01..=1.01).contains(&value), "{at}: {value} at {step}");
                }
            }
        }
    }

    #[test]
    fn every_mode_half_way_at_no_tension_and_at_half_tension() {
        // Per mode: the value at t = 0.5 for tension 0, 0.5 and -0.5, worked
        // out by hand from the formulas in docs/automation-234.md.
        let g = |k: f64| (0.5 * k).exp_m1() / k.exp_m1();
        let bent = g(0.5 * L);
        // Half tension leaves 16 steps: 1000001^0.7 is 15849. Stairs that
        // go up have 18 treads over 17 rises, and 8.5 rounds to 8. Below 0
        // they are round(17t + 0.5) / 18.
        let stairs = (8.0 / 17.0, 9.0 / 18.0);
        let seventh = 0.5 * (0.5 - 0.5f64.powi(6));
        let a = 1.0 - 0.55f64.powi(3);
        let third = (1.0 - a) / a / (1.0 - 0.5 * a) - (1.0 - a) / a;
        let want = [
            (0, 0.5, 1.0 - bent, bent),
            (1, 0.5, 0.5, 0.5),
            (2, 0.0, 0.0, 0.0),
            (3, 500.0 / 1001.0, stairs.0, stairs.1),
            (4, 0.5, 0.5, 0.5),
            (5, 0.0, 0.0, 0.0),
            // 16.5 periods put both the triangle and the sine half-way up.
            (6, 0.5, 0.5, 0.5),
            (7, 0.5, 0.5 + seventh, 0.5 - seventh),
            (8, 0.5, 0.5, 0.5),
            (
                9,
                0.5,
                0.25 + 0.5 * 0.5f64.sqrt(),
                0.75 - 0.5 * 0.5f64.sqrt(),
            ),
            (10, 0.5, 0.5, 0.5),
            (11, 0.5, 1.0 - third, third),
            (12, 0.5, 0.5, 0.5),
        ];
        for (mode, none, up, down) in want {
            let got = [0.0, 0.5, -0.5].map(|tension| shape(mode, tension, 0.5));
            for (got, want) in got.into_iter().zip([none, up, down]) {
                assert!(close(got, want), "mode {mode}: {got}, wanted {want}");
            }
        }
    }

    #[test]
    fn a_line_and_every_bending_mode_without_tension_are_straight() {
        for mode in [0, 1, 7, 8, 9, 11, 12] {
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert_eq!(shape(mode, 0.0, t), t, "mode {mode} at {t}");
            }
        }
    }

    #[test]
    fn hold_stays_at_the_point_before_until_the_end() {
        for tension in [-1.0, 0.0, 1.0] {
            assert_eq!(shape(2, tension, 0.0), 0.0);
            assert_eq!(shape(2, tension, 0.5), 0.0);
            assert_eq!(shape(2, tension, 0.999_999), 0.0);
            assert_eq!(shape(2, tension, 1.0), 1.0);
        }
        // A mode FL has no curve for holds as well.
        assert_eq!(shape(13, 0.0, 0.5), 0.0);
        assert_eq!(shape(200, 0.0, 0.5), 0.0);
    }

    #[test]
    fn a_single_curve_bends_up_for_a_tension_above_0_and_down_below() {
        // g(u) = (e^(ku) - 1) / (e^k - 1) with k = ln 1000001 at full
        // tension, so g(0.5) is close to 1000 / 1000000.
        let half = (1_000_001f64.sqrt() - 1.0) / 1e6;
        assert!(close(shape(0, -1.0, 0.5), half));
        assert!(close(shape(0, 1.0, 0.5), 1.0 - half));
        assert!(shape(0, -0.2, 0.5) < 0.5 && shape(0, 0.2, 0.5) > 0.5);
        // The two signs mirror each other.
        for t in [0.1, 0.5, 0.8] {
            assert!(close(shape(0, 0.4, t), 1.0 - shape(0, -0.4, 1.0 - t)));
        }
        // A tension close to 0 is close to the line.
        assert!((shape(0, 1e-6, 0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn the_double_curves_are_two_halves_that_meet_in_the_middle() {
        for (double, single) in [(1, 0), (8, 7), (12, 11)] {
            for tension in [-1.0, -0.3, 0.6, 1.0] {
                let at = format!("mode {double} tension {tension}");
                assert!(close(shape(double, tension, 0.5), 0.5), "{at}");
                let half = shape(single, tension, 0.5);
                assert!(close(shape(double, tension, 0.25), 0.5 * half), "{at}");
                assert!(
                    close(shape(double, tension, 0.75), 1.0 - 0.5 * half),
                    "{at}"
                );
            }
        }
    }

    #[test]
    fn the_stairs_and_the_pulse_step() {
        // Full tension leaves 0 steps. Stairs that go up get one more:
        // round(2t - 0.5) / 1, a single step half-way.
        assert_eq!(shape(3, 1.0, 0.4), 0.0);
        assert_eq!(shape(3, 1.0, 0.6), 1.0);
        // Below 0 it is round(t + 0.5) / 2: half-way up from the start on.
        assert_eq!(shape(3, -1.0, 0.01), 0.5);
        assert_eq!(shape(3, -1.0, 0.99), 0.5);
        // No tension is 1000 steps, which is the line to a thousandth.
        for t in [0.123, 0.5, 0.877] {
            assert!((shape(3, 0.0, t) - t).abs() < 0.001);
            assert!((shape(4, 0.0, t) - t).abs() < 0.001);
            assert!((shape(3, -1e-6, t) - t).abs() < 0.001);
        }
        // Smooth stairs with one step: flat at both ends, and through the
        // middle on a cube (tension from 0 up) or on its mirror image.
        assert!(close(shape(4, 1.0, 0.5), 0.5));
        assert!(close(shape(4, 1.0, 0.25), 0.5 * 0.125));
        assert!(close(shape(4, 1.0, 0.75), 1.0 - 0.5 * 0.125));
        assert!(close(shape(4, -1.0, 0.25), 0.5 * 0.875));
        assert!(close(shape(4, -1.0, 0.75), 1.0 - 0.5 * 0.875));
        // A pulse is 0 or 1 and nothing else. At full tension it has two
        // halves: low then high from 0 up, high then low below.
        let levels = |tension| [0.1, 0.3, 0.6, 0.9].map(|t| shape(5, tension, t));
        assert_eq!(levels(1.0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(levels(-1.0), [1.0, 1.0, 0.0, 0.0]);
        // 1000 steps are 1001 pulses, each a little under 0.001 long.
        let levels = [0.0004, 0.0006, 0.0012, 0.0016].map(|t| shape(5, 0.0, t));
        assert_eq!(levels, [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn a_wave_swings_between_the_two_points() {
        // Full negative tension is half a period of a sine, from its
        // trough to its crest.
        assert!(close(shape(6, -1.0, 0.5), 0.5));
        assert!(close(shape(6, -1.0, 0.25), (1.0 - 0.5f64.sqrt()) / 2.0));
        // Full positive tension has no steps, and FL draws a line.
        assert_eq!(shape(6, 1.0, 0.3), 0.3);
        // No tension is 1000.5 periods: up and down again within a
        // thousandth of the segment.
        assert!(shape(6, 0.0, 0.0005) > 0.9);
        assert!(shape(6, 0.0, 0.001) < 0.1);
        assert!(shape(6, -0.0001, 0.0005) > 0.9);
    }

    #[test]
    fn the_second_and_third_single_curves_and_the_half_sine() {
        // Mode 7 adds tension * (u - u^6) to the line.
        let bulge = 0.5 - 0.5f64.powi(6);
        assert!(close(shape(7, 1.0, 0.5), 0.5 + bulge));
        assert!(close(shape(7, -1.0, 0.5), 0.5 - bulge));
        assert!(close(shape(7, 0.5, 0.5), 0.5 + 0.5 * bulge));
        // Mode 9 mixes the line with a quarter of a sine.
        assert!(close(shape(9, 1.0, 0.5), 0.5f64.sqrt()));
        assert!(close(shape(9, -1.0, 0.5), 1.0 - 0.5f64.sqrt()));
        assert!(close(shape(9, 0.5, 0.5), 0.25 + 0.5 * 0.5f64.sqrt()));
        // Mode 11 at full tension: A = 1 - 0.1^3, so the curve is at
        // 0.001 / 0.5005 - 0.001 / 0.999 half-way.
        let a = 0.999;
        let low = (1.0 - a) / a / (1.0 - a * 0.5) - (1.0 - a) / a;
        assert!(close(shape(11, -1.0, 0.5), low));
        assert!(close(shape(11, 1.0, 0.5), 1.0 - low));
        assert!(shape(11, -0.2, 0.5) < 0.5 && shape(11, 0.2, 0.5) > 0.5);
    }

    #[test]
    fn a_smooth_segment_alone_eases_in_and_out() {
        // Flat at both ends: 3t^2 - 2t^3.
        for t in [0.1f64, 0.25, 0.5, 0.9] {
            assert!(close(shape(10, 0.0, t), 3.0 * t * t - 2.0 * t.powi(3)));
        }
        // Tension takes it towards the line, either sign alike.
        let eased = 3.0 * 0.0625 - 2.0 * 0.015_625;
        for tension in [0.5, -0.5] {
            assert!(close(shape(10, tension, 0.25), 0.25 + 0.5 * (eased - 0.25)));
        }
        assert_eq!(shape(10, 1.0, 0.25), 0.25);
    }

    #[test]
    fn smooth_neighbours_pass_through_a_point_at_one_slope() {
        // Three points on a line, all smooth: the middle keeps the line's
        // slope, and the ends are flat.
        let points = [
            point(0.0, 0.0, 10, 0.0),
            point(1.0, 0.25, 10, 0.0),
            point(1.0, 0.5, 10, 0.0),
            point(1.0, 0.75, 10, 0.0),
        ];
        let curve = Curve::new(&points);
        let slope = |at: f64| (curve.value_at(at + 1e-6) - curve.value_at(at - 1e-6)) / 2e-6;
        assert!((slope(1.0) - 0.25).abs() < 1e-5);
        assert!((slope(2.0) - 0.25).abs() < 1e-5);
        assert!(slope(1e-5).abs() < 1e-4 && slope(3.0 - 1e-5).abs() < 1e-4);
        // The middle segment has slope 1 at both ends, so it is the line.
        assert!(close(curve.value_at(1.5), 0.375));

        // At a peak the slopes point opposite ways, and the curve is flat.
        let peak = [
            point(0.0, 0.0, 10, 0.0),
            point(1.0, 1.0, 10, 0.0),
            point(1.0, 0.0, 10, 0.0),
        ];
        let curve = Curve::new(&peak);
        assert!(close(curve.value_at(0.5), 0.5));
        assert!(curve.value_at(0.99) < 1.0 && curve.value_at(0.99) > 0.999);

        // A neighbour of another mode leaves the end flat too.
        let mixed = [
            point(0.0, 0.0, 0, 0.0),
            point(1.0, 0.5, 0, 0.0),
            point(1.0, 1.0, 10, 0.0),
        ];
        assert!(close(
            Curve::new(&mixed).value_at(1.25),
            0.5 + 0.5 * 0.15625
        ));
        // And between two equal values there is nothing to shape.
        let level = [point(0.0, 0.5, 10, 0.0), point(1.0, 0.5, 10, 0.0)];
        assert_eq!(Curve::new(&level).value_at(0.3), 0.5);
    }

    #[test]
    fn a_curve_is_its_segments_end_to_end() {
        let curve = Curve::new(&[
            Point::new(0.0, 0.25),
            Point::new(2.0, 1.0),
            point(1.0, 0.5, 2, 0.0),
            // No beats long: a jump.
            Point::new(0.0, 0.0),
            Point::new(4.0, 1.0),
        ]);
        let values: Vec<f64> = [-1.0, 0.0, 1.0, 2.0, 2.5, 3.0, 5.0, 7.0, 99.0]
            .map(|beats| curve.value_at(beats))
            .to_vec();
        assert_eq!(values, [0.25, 0.25, 0.625, 1.0, 1.0, 0.0, 0.5, 1.0, 1.0]);
        assert_eq!(curve.value_at(f64::NAN), 0.25);
        assert_eq!(curve.value_at(f64::INFINITY), 1.0);

        assert_eq!(Curve::new(&[]).value_at(1.0), 0.0);
        assert_eq!(Curve::new(&[Point::new(3.0, 0.7)]).value_at(1.0), 0.7);
        // A first point that is not at 0 moves the whole curve.
        let late = Curve::new(&[Point::new(1.0, 0.0), Point::new(2.0, 1.0)]);
        assert_eq!(late.value_at(0.5), 0.0);
        assert_eq!(late.value_at(2.0), 0.5);
    }

    #[test]
    fn a_broken_point_reads_as_fl_s_loader_reads_it() {
        let curve = Curve::new(&[
            Point::new(0.0, f64::NAN),
            Point::new(1.0, 0.5),
            Point::new(-3.0, f64::NAN),
            Point::new(f64::NAN, f64::INFINITY),
            Point::new(1.0, 1.0),
        ]);
        // 0, a line to 0.5, two points on the spot that keep 0.5, a line
        // to 1.
        assert_eq!(curve.value_at(0.0), 0.0);
        assert_eq!(curve.value_at(0.5), 0.25);
        assert_eq!(curve.value_at(1.0), 0.5);
        assert_eq!(curve.value_at(1.5), 0.75);
    }

    fn clip(items: &[(f64, f64, f64)]) -> Clip {
        Clip {
            curve: Curve::new(&[Point::new(0.0, 0.0), Point::new(4.0, 1.0)]),
            items: items
                .iter()
                .map(|(start, length, offset)| Shown {
                    start: *start,
                    length: *length,
                    offset: *offset,
                })
                .collect(),
        }
    }

    #[test]
    fn a_clip_plays_where_its_items_are_and_holds_after_them() {
        // Four beats from tick 96, then the clip's second half from 960.
        let clip = clip(&[(96.0, 384.0, 0.0), (960.0, 192.0, 2.0)]);
        assert_eq!(clip.at(0.0, 96.0), None);
        assert_eq!(clip.at(95.9, 96.0), None);
        assert_eq!(clip.at(96.0, 96.0), Some((96.0, 0.0)));
        assert_eq!(clip.at(288.0, 96.0), Some((288.0, 0.5)));
        // Past the item the value is the one at its end.
        assert_eq!(clip.at(480.0, 96.0), Some((480.0, 1.0)));
        assert_eq!(clip.at(700.0, 96.0), Some((480.0, 1.0)));
        assert_eq!(clip.at(960.0, 96.0), Some((960.0, 0.5)));
        assert_eq!(clip.at(1056.0, 96.0), Some((1056.0, 0.75)));
        assert_eq!(clip.at(5000.0, 96.0), Some((1152.0, 1.0)));
    }

    #[test]
    fn the_clip_that_wrote_last_has_the_control() {
        let clips = [
            clip(&[(0.0, 384.0, 0.0)]),
            clip(&[(192.0, 96.0, 0.0), (768.0, 384.0, 0.0)]),
        ];
        let lane = Lane {
            control: Control::InsertVolume(1),
            initial: 12_800.0,
            range: (0.0, 16_000.0),
            sources: vec![
                Source {
                    clip: 0,
                    min: 0.0,
                    max: 16_000.0,
                },
                Source {
                    clip: 1,
                    min: 1000.0,
                    max: 2000.0,
                },
            ],
        };
        let raw = |tick| lane.raw_at(&clips, tick, 96.0);
        assert_eq!(raw(96.0), 4000.0);
        // Both play: the one listed later wins.
        assert_eq!(raw(192.0), 1000.0);
        assert_eq!(raw(240.0), 1125.0);
        // The second has stopped and the first goes on.
        assert_eq!(raw(300.0), 12_500.0);
        // The first ended last, at its 16000.
        assert_eq!(raw(500.0), 16_000.0);
        assert_eq!(raw(768.0), 1000.0);

        // Without a clip so far, the control is what the project says, and
        // a value past the control's range stops at its end.
        let late = [clip(&[(96.0, 384.0, 0.0)])];
        let lane = Lane {
            sources: vec![Source {
                clip: 0,
                min: -4000.0,
                max: 40_000.0,
            }],
            ..lane
        };
        assert_eq!(lane.raw_at(&late, 0.0, 96.0), 12_800.0);
        assert_eq!(lane.raw_at(&late, 96.0, 96.0), 0.0);
        assert_eq!(lane.raw_at(&late, 480.0, 96.0), 16_000.0);
    }
}
