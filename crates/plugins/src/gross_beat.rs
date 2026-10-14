//! Gross Beat: a time envelope that reads the last two bars back at a
//! place it draws, and a volume envelope over what that reads, both a bar
//! long and in step with the song, as `docs/plugins/gross-beat.md` has it.
//!
//! The state is a version and nine parameters as little-endian i32, a few
//! option bytes, then 36 time slots and 36 volume slots. A slot is a name,
//! three i32 and three flags, and its envelope as a `TMMap` stream (see
//! [`crate::menvelope`]). FL reads versions up to 9, written as
//! `0x00090003`, and leaves a newer state alone; so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Time slot | 0 to 35 | 0 |
//! | 1 Volume slot | 0 to 35 | 0 |
//! | 2 Time mix | 0 to 256 | 256 |
//! | 3 Volume mix | 0 to 256 | 256 |
//! | 4 Volume attack | 0 to 10000 | 1765 |
//! | 5 Volume release | 0 to 10000 | 5000 |
//! | 6 Volume tension | -256 to 256 | 0 |
//! | 7 Hold | 0, 1 | 0 |
//! | 8 Scratch | 0 to 65536 | 65536 |
//!
//! An envelope's `x` counts beats and a slot's envelope runs round and
//! round, a beat every `60 / tempo` seconds whatever the song does, once
//! the plugin has put it in place. It does that when the song starts or
//! moves and when a slot is chosen: at the song's place inside the slot's
//! position sync length, or at 0 with that sync off. A slot chosen by its
//! parameter waits for the next multiple of its trigger sync length, and
//! the place counts in ticks at the start of a block, rounded, as FL's
//! `GetMixingTime` hands them out.
//!
//! The time envelope's value `y` reads the input `(1 - y)` times two bars
//! back, through a windowed sinc of 8 or 16 taps a side that narrows when
//! the read runs faster than the input. Everything is 15 frames late,
//! which the plugin tells its host, and the dry signal is held back by as
//! much. The volume envelope goes through the tension curve, the volume
//! mix and two one-pole smoothers with the attack and release times, and
//! scales what the time envelope read.
//!
//! What this leaves out or takes its own way, all in the doc:
//!
//! - FL starts the plugin when the song starts and tells it when the song
//!   moves. Here [`Effect::tick`] says both: the first tick starts it, and
//!   a tick that does not follow the one before moves it. A loop of the
//!   song counts as a move. [`Effect::reset`] stops it, and a stopped
//!   plugin passes its input through on time, as FL's does.
//! - Slots chosen from MIDI notes or the keyboard, the key-held slots'
//!   return on a key's release and the GUI's slot buttons: nothing here
//!   sends them. A slot changes when its parameter does.
//! - The sinc's arithmetic is FL's, in f32 and in its order, with an exact
//!   reciprocal where FL's SSE code takes `rcpps` and one Newton step.
//! - The tension curve takes `exp` in full where FL calls IPP's 11-bit
//!   `ippsExp_32f_A11`; [`Effect::fidelity`] says so when the tension is
//!   not 0.

use crate::menvelope::{Class, MEnvelope, Reader};
use crate::{Effect, Fidelity, StateError, knob_value};
use dsp::curve::{Curve, Point};
use std::f64::consts::{FRAC_PI_2, PI};

/// The plugin's event 201 name.
pub const NAME: &str = "Gross Beat";

const PARAMS: usize = 9;
const TIME_SLOT: usize = 0;
const VOLUME_SLOT: usize = 1;
const TIME_MIX: usize = 2;
const VOLUME_MIX: usize = 3;
const ATTACK: usize = 4;
const RELEASE: usize = 5;
const TENSION: usize = 6;
const HOLD: usize = 7;
const SCRATCH: usize = 8;

/// The range of each parameter's control.
const RANGES: [(i32, i32); PARAMS] = [
    (0, 35),
    (0, 35),
    (0, 256),
    (0, 256),
    (0, 10_000),
    (0, 10_000),
    (-256, 256),
    (0, 1),
    (0, 65_536),
];
const DEFAULTS: [i32; PARAMS] = [0, 0, 256, 256, 1765, 5000, 0, 0, 65_536];

/// How many slots each of the two envelopes has.
const SLOTS: usize = 36;
/// The newest version FL reads, once a version over 0xffff is shifted
/// down 16 bits.
const NEWEST: i32 = 0x9_0003;

/// How many frames late everything comes out: FL's `FHD_SetLatency`.
const LATENCY: usize = 15;

/// The largest block worked through in one go. A longer one is cut up.
const CHUNK: usize = 512;

/// The slowest tempo FL has, in BPM, which sizes the ring.
const SLOWEST: usize = 10;

/// A trigger or a hold that is not due, and one that is due at once.
const NEVER: i32 = i32::MAX;
const AT_ONCE: i32 = i32::MIN + 1;
/// The sync length a hold repeats: 4 beats.
const HOLD_LENGTH: usize = 5;

/// FL's "too small to tell from 0", 2^-24.
const TINY: f32 = 5.960_464_5e-8;

/// What a jump in the time envelope is faded over, by the click reduction
/// setting: the read speed's smoothing and the correction's decay, in ms.
const SPEED_MS: [f64; 3] = [1.0, 1.0, 5.0];
const DECAY_MS: [f64; 3] = [25.0, 25.0, 35.0];

/// The DC blocker's coefficient at 44.1 kHz and its damping.
const DC_RATE: f64 = 2.2835e-5;
const DC_DAMP: f32 = 0.96;

/// An envelope as a slot runs it.
#[derive(Debug, Clone, PartialEq)]
struct Envelope {
    /// Each point's `x`, how far it is after the one before, in beats.
    lengths: Vec<f64>,
    /// Each point's value, as the f32 FL keeps.
    values: Vec<f64>,
    curve: Curve,
    /// The two points a new slot has: FL leaves such an envelope out.
    flat: bool,
    /// It has a length to run round. Without one it stays on its first
    /// value.
    runs: bool,
}

/// Where an envelope's run is: in the segment that ends at `point`,
/// `phase` of the way through it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Run {
    point: usize,
    phase: f64,
    /// The segment's speed: 1 over its length.
    rate: f64,
    from: f64,
    rise: f64,
}

impl Envelope {
    fn new(points: &[Point]) -> Self {
        let mut values: Vec<f64> = Vec::with_capacity(points.len());
        for point in points {
            let before = values.last().copied().unwrap_or(1.0);
            let y = if point.y.is_finite() { point.y } else { before };
            values.push(f64::from(y as f32));
        }
        let lengths: Vec<f64> = points.iter().map(|p| p.x.max(0.0)).collect();
        let default = |p: &Point, x: f64| p.x == x && p.y == 1.0 && p.tension == 0.0 && p.mode == 2;
        let flat = points.len() == 2 && default(&points[0], 0.0) && default(&points[1], 4.0);
        let runs = points.len() >= 2 && lengths.iter().sum::<f64>() > 0.0;
        Envelope {
            lengths,
            values,
            curve: Curve::new(points),
            flat,
            runs,
        }
    }

    /// The envelope a new slot has: 1 all the way along its 4 beats.
    fn default_slot() -> Self {
        let point = |x: f64| Point {
            x,
            y: 1.0,
            tension: 0.0,
            mode: 2,
        };
        Envelope::new(&[point(0.0), point(4.0)])
    }

    /// A run at the first point (`FUN_00787050`).
    fn start(&self) -> Run {
        let first = self.lengths.first().copied().unwrap_or(0.0);
        let (phase, rate) = if first == 0.0 {
            (1.0, 1.0)
        } else {
            (0.0, 1.0 / first)
        };
        Run {
            point: 0,
            phase,
            rate,
            from: self.values.first().copied().unwrap_or(1.0),
            rise: 0.0,
        }
    }

    /// Move the run on by `by` beats and give its value there
    /// (`FUN_00788d60`). A run that passes the last point goes round to
    /// the first.
    fn step(&self, run: &mut Run, by: f64) -> f64 {
        if !self.runs {
            return run.from;
        }
        let last = self.lengths.len() - 1;
        run.phase += by * run.rate;
        // An envelope this short against the step has no shape to draw.
        let mut turns = 0;
        while run.phase >= 1.0 {
            run.phase = if run.rate == 0.0 {
                by
            } else {
                (run.phase - 1.0) / run.rate
            };
            run.from = self.values[run.point];
            run.point = if run.point < last { run.point + 1 } else { 0 };
            run.rise = self.values[run.point] - run.from;
            let length = self.lengths[run.point];
            if length <= 0.0 {
                run.rate = 1.0;
                run.phase += 1.0;
            } else {
                run.rate = 1.0 / length;
                run.phase *= run.rate;
            }
            turns += 1;
            if turns > 1 << 16 {
                run.phase = 0.0;
                break;
            }
        }
        run.from + self.curve.progress(run.point, run.phase) * run.rise
    }
}

/// One slot of either envelope.
#[derive(Debug, Clone, PartialEq)]
struct Slot {
    /// The sync lengths, as indexes into [`GrossBeat::sync`]: how a choice
    /// of the slot waits, and where its envelope starts.
    trigger: usize,
    position: usize,
    /// 0 off, 1 hold the last bar at once, 2 hold the next bar.
    hold: i32,
    /// A time slot's volume slot is the one with its number.
    link: bool,
    /// Back to the slot before on a key's release, which nothing here
    /// sends.
    key_held: bool,
    /// Back to slot 0 a bar after it is chosen.
    one_shot: bool,
    envelope: Envelope,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            trigger: 5,
            position: 5,
            hold: 0,
            link: false,
            key_held: false,
            one_shot: false,
            envelope: Envelope::default_slot(),
        }
    }
}

/// The time envelope or the volume envelope, with the slot it plays.
#[derive(Debug, Clone, PartialEq)]
struct Lane {
    slots: Vec<Slot>,
    /// The slot playing, the one the parameter asks for, and the one a
    /// trigger waits to play at tick `at`.
    current: usize,
    wanted: usize,
    pending: usize,
    at: i32,
    run: Run,
}

impl Default for Lane {
    fn default() -> Self {
        let slots = vec![Slot::default(); SLOTS];
        let run = slots[0].envelope.start();
        Lane {
            slots,
            current: 0,
            wanted: 0,
            pending: 0,
            at: NEVER,
            run,
        }
    }
}

impl Lane {
    fn slot(&self) -> &Slot {
        &self.slots[self.current]
    }
}

/// FL's one-pole smoother with a time to rise and a time to fall
/// (`FUN_00608c30`, run by `FUN_00608dd0`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Smoother {
    up: f64,
    down: f64,
    state: f32,
    same: bool,
}

impl Smoother {
    /// Times in frames: the state gets within 1 % in that many.
    fn set(&mut self, up: f64, down: f64) {
        let coefficient = |time: f64| {
            if time > 0.0 {
                (0.01f64.ln() / time.max(1.0)).exp()
            } else {
                0.0
            }
        };
        self.up = coefficient(up);
        self.down = coefficient(down);
        self.same = self.up == self.down;
    }

    fn run(&mut self, block: &mut [f32]) {
        let (up, down) = (self.up as f32, self.down as f32);
        let mut state = self.state;
        if self.same {
            for x in block {
                state = (state - *x) * up + *x;
                *x = state;
            }
        } else {
            for x in block {
                let k = if state <= *x { up } else { down };
                state = *x + k * (state - *x);
                *x = state;
            }
        }
        self.state = if state.abs() > TINY { state } else { 0.0 };
    }
}

/// The volume tension curve (`FUN_00611cd0`, `FUN_00611d60`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Bend {
    tension: f32,
    inverse: f32,
    log: f32,
}

impl Bend {
    fn new(knob: i32) -> Self {
        let size = (knob as f32 * (1.0 / 256.0)).abs();
        let curve = ((f64::from(size) * 1001f64.ln()).exp() - 1.0) as f32;
        let tension = curve * knob.signum() as f32;
        if tension == 0.0 {
            return Bend::default();
        }
        let magnitude = f64::from(tension.abs());
        Bend {
            tension,
            inverse: (1.0 / magnitude) as f32,
            log: (magnitude + 1.0).ln() as f32,
        }
    }

    fn apply(&self, block: &mut [f32]) {
        if self.tension == 0.0 {
            return;
        }
        let up = self.tension.is_sign_positive();
        for x in block {
            let mut v = if up { 1.0 - *x } else { *x };
            v *= self.log;
            v = f64::from(v).exp() as f32;
            v += -1.0;
            v *= self.inverse;
            *x = if up { 1.0 - v } else { v };
        }
    }
}

/// The plugin's per-block scratch, [`CHUNK`] long.
#[derive(Debug, Clone, PartialEq)]
struct Scratch {
    places: Vec<f64>,
    levels: Vec<f64>,
    speed: Vec<f32>,
    jumps: Vec<f32>,
    cut: Vec<f64>,
    cos2: Vec<f64>,
    wet: Vec<[f32; 2]>,
    old: Vec<[f32; 2]>,
    gain: Vec<f32>,
    push: Vec<f32>,
}

impl Default for Scratch {
    fn default() -> Self {
        Scratch {
            places: vec![0.0; CHUNK],
            levels: vec![0.0; CHUNK],
            speed: vec![0.0; CHUNK],
            jumps: vec![0.0; CHUNK],
            cut: vec![0.0; CHUNK],
            cos2: vec![0.0; CHUNK],
            wet: vec![[0.0; 2]; CHUNK],
            old: vec![[0.0; 2]; CHUNK],
            gain: vec![0.0; CHUNK],
            push: vec![0.0; CHUNK],
        }
    }
}

/// How a reader's sinc is set: its taps a side, and for a read faster
/// than the input `pi / speed` and twice its cosine.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sinc {
    taps: usize,
    window: f64,
    window2: f64,
}

impl Sinc {
    fn new(taps: usize) -> Self {
        let window = PI / taps as f64;
        Sinc {
            taps,
            window,
            window2: window.cos() * 2.0,
        }
    }
}

/// The frame `at + frac` of `ring` through FL's windowed sinc
/// (`FUN_00614d50`): `taps` frames on each side of `at`, each weighted
/// `sin(x cut) / (x pi)` under the window `0.36 + c + 0.64 c^2`, with
/// `c = cos(x pi / taps)`, and the sum halved.
fn sinc_at(ring: &[[f32; 2]], at: usize, frac: f64, taps: Sinc, cut: f64, cos2: f64) -> [f32; 2] {
    let len = ring.len();
    let frame = |back: usize, ahead: usize| ring[(at + len * 2 + ahead - back) % len];
    let f = frac.max(1e-25);
    let a0 = f * -taps.window + FRAC_PI_2;
    let a1 = a0 - taps.window;
    let b0 = f * cut;
    let b1 = b0 + cut;
    let (w0, w1, s0, s1) = (a0.sin(), a1.sin(), b0.sin(), b1.sin());
    let w = w0 * taps.window2 - w1;
    let s = s0 * cos2 - s1;
    // The four lanes: frame at + 1, at, at + 2 and at - 1, which is x of
    // f - 1, f, f - 2 and f + 1.
    let mut win = [
        w as f32,
        w0 as f32,
        (w * taps.window2 - w0) as f32,
        w1 as f32,
    ];
    let mut sin = [s as f32, s0 as f32, (s * cos2 - s0) as f32, s1 as f32];
    let fp = f * PI;
    let mut angle = [
        (fp - PI) as f32,
        fp as f32,
        ((fp - PI) - PI) as f32,
        (fp + PI) as f32,
    ];
    let turn = (2.0 * PI) as f32;
    let (cs, cw) = (cos2 as f32, taps.window2 as f32);
    let shape = |c: f32| 0.64 * c * c + c + 0.36;
    let mut ahead = [0.0f32; 2];
    let mut back = [0.0f32; 2];
    for k in 0..taps.taps / 2 {
        let r = angle.map(|a| 1.0 / a);
        let next = [
            sin[2] * cs - sin[0],
            sin[3] * cs - sin[1],
            win[2] * cw - win[0],
            win[3] * cw - win[1],
        ];
        angle[0] -= turn;
        angle[1] += turn;
        angle[2] -= turn;
        angle[3] += turn;
        let a = sin[0] * r[0] * shape(win[0]);
        let b = sin[1] * r[1] * shape(win[1]);
        let c = sin[2] * r[2] * shape(win[2]);
        let d = sin[3] * r[3] * shape(win[3]);
        let (one, two) = (frame(0, 1 + 2 * k), frame(0, 2 + 2 * k));
        let (here, before) = (frame(2 * k, 0), frame(2 * k + 1, 0));
        for side in 0..2 {
            ahead[side] = ahead[side] + a * one[side] + c * two[side];
            back[side] = back[side] + b * here[side] + d * before[side];
        }
        let win2 = [next[2] * cw - win[2], next[3] * cw - win[3]];
        sin = [
            next[0],
            next[1],
            next[0] * cs - sin[2],
            next[1] * cs - sin[3],
        ];
        win = [next[2], next[3], win2[0], win2[1]];
    }
    [(back[0] + ahead[0]) * 0.5, (back[1] + ahead[1]) * 0.5]
}

/// Gross Beat.
#[derive(Debug, Clone, PartialEq)]
pub struct GrossBeat {
    params: [i32; PARAMS],
    /// The options: remove DC, attack compensation, HQ resampling, click
    /// reduction (0 off, 1 low, 2 high), and slots from MIDI notes.
    dc: bool,
    compensate: bool,
    hq: bool,
    declick: usize,
    midi: bool,
    lanes: [Lane; 2],
    rate: f32,
    tempo: f64,
    ppq: u16,
    /// A bar of 4 beats and two of them, in frames, and a frame in beats.
    bar: f64,
    span: f64,
    beat_step: f64,
    /// How far ahead the attack compensation starts both envelopes, in
    /// frames.
    lead: f64,
    smoothers: [Smoother; 2],
    scratchers: [Smoother; 3],
    speed: Smoother,
    bend: Bend,
    scratch: f32,
    dc_k: f32,
    dc_state: [f64; 4],
    /// The read speed a jump starts at, the correction's decay, and the
    /// frames a change of tempo fades over.
    jump: f32,
    decay: f32,
    fade: usize,
    // The transport.
    playing: bool,
    first: bool,
    tick: Option<u64>,
    elapsed: usize,
    // The ring of past input, where the next frame goes into it, and the
    // frame the reader is at for the first frame of the next block.
    ring: Vec<[f32; 2]>,
    write: usize,
    base: usize,
    delay: [[f32; 2]; LATENCY],
    delay_at: usize,
    reader: Sinc,
    cut: f64,
    cos2: f64,
    last_place: f64,
    // A change of tempo fades from the reader before it.
    fading: usize,
    fade_span: f64,
    fade_base: usize,
    // Hold.
    holds: i32,
    hold_at: i32,
    holding: bool,
    hold_saved: usize,
    hold_base: usize,
    // Click reduction.
    correction: [f32; 2],
    last_wet: [f32; 2],
    last_out: [f32; 2],
    work: Scratch,
}

impl Default for GrossBeat {
    fn default() -> Self {
        let mut plugin = GrossBeat {
            params: DEFAULTS,
            dc: false,
            compensate: false,
            hq: true,
            declick: 1,
            midi: false,
            lanes: [Lane::default(), Lane::default()],
            rate: 44_100.0,
            tempo: 120.0,
            ppq: 96,
            bar: 0.0,
            span: 0.0,
            beat_step: 0.0,
            lead: 0.0,
            smoothers: [Smoother::default(); 2],
            scratchers: [Smoother::default(); 3],
            speed: Smoother::default(),
            bend: Bend::default(),
            scratch: 0.0,
            dc_k: 0.0,
            dc_state: [0.0; 4],
            jump: 0.0,
            decay: 0.0,
            fade: 1,
            playing: false,
            first: true,
            tick: None,
            elapsed: 0,
            ring: Vec::new(),
            write: 0,
            base: 0,
            delay: [[0.0; 2]; LATENCY],
            delay_at: 0,
            reader: Sinc::new(16),
            cut: PI,
            cos2: -2.0,
            last_place: 0.0,
            fading: 0,
            fade_span: 0.0,
            fade_base: 0,
            holds: 0,
            hold_at: NEVER,
            holding: false,
            hold_saved: 0,
            hold_base: 0,
            correction: [0.0; 2],
            last_wet: [0.0; 2],
            last_out: [0.0; 2],
            work: Scratch::default(),
        };
        plugin.at_rate();
        plugin.retime();
        plugin.knobs();
        plugin
    }
}

impl GrossBeat {
    /// A sync length in ticks: off, a quarter, half, one, two or four
    /// beats (`FUN_00844a90`).
    fn sync(&self, index: usize) -> i32 {
        let ppq = i32::from(self.ppq.max(4));
        [0, ppq >> 2, ppq / 2, ppq, ppq * 2, ppq * 4][index.min(5)]
    }

    /// The song's place at the start of the next block in ticks, rounded:
    /// FL's `GetMixingTime` in ticks.
    fn now(&self) -> i32 {
        let Some(tick) = self.tick else {
            return 0;
        };
        let per_tick = f64::from(self.rate) * 60.0 / (self.tempo * f64::from(self.ppq.max(1)));
        let at = tick as f64 + self.elapsed as f64 / per_tick;
        at.round_ties_even().min(f64::from(i32::MAX - 1)) as i32
    }

    /// A frame of the attack or release knob (`FUN_00841de0`,
    /// `FUN_00841ec0`): `501^(v / 10000) - 1` ms.
    fn knob_frames(&self, knob: i32) -> f64 {
        let x = f64::from(knob as f32 * 1e-4_f32);
        let ms = ((x * 501f64.ln()).exp() - 1.0) as f32;
        f64::from(ms) * 0.001 * f64::from(self.rate)
    }

    /// What the attack and release knobs set (`FUN_00841fb0`).
    fn knobs(&mut self) {
        let root = 1.0 / 2f64.sqrt();
        let attack = self.knob_frames(self.params[ATTACK]);
        let release = self.knob_frames(self.params[RELEASE]) * 2.0;
        self.lead = if self.compensate { attack } else { 0.0 };
        for smoother in &mut self.smoothers {
            smoother.set(attack * root, release * root);
        }
        self.bend = Bend::new(self.params[TENSION]);
        self.scratch = self.params[SCRATCH] as f32 * (1.0 / 65_536.0) - 1.0;
    }

    /// What the rate sets (`FUN_00841ef0`, `FUN_00841920`, and the DC
    /// blocker in the dispatcher).
    fn at_rate(&mut self) {
        let rate = f64::from(self.rate);
        let root = 1.0 / 3f64.sqrt();
        for smoother in &mut self.scratchers {
            smoother.set(rate * 0.3 * root, rate * 0.2 * root);
        }
        let declick = self.declick.min(2);
        self.jump = (rate * 0.002) as f32;
        self.decay = (0.01f64.ln() / (DECAY_MS[declick] * 0.001 * rate)).exp() as f32;
        let speed = SPEED_MS[declick] * 0.001 * rate;
        self.speed.set(speed, speed);
        self.speed.same = true;
        self.fade = ((rate * 0.001).round_ties_even() as usize).max(1);
        self.dc_k = (44_100.0 / rate * DC_RATE) as f32;
        self.size_ring();
    }

    /// The ring FL sizes once (`FUN_00841a20`): three bars at 10 BPM, the
    /// slowest tempo FL has, plus 16 frames, a block and 64 guard frames.
    fn room(&self) -> usize {
        let rate = self.rate as usize;
        rate * 240 / SLOWEST * 3 + 16 + CHUNK + 64
    }

    /// Size the ring for the rate. Only the rate sizes it, so a tempo
    /// change never asks for memory. A ring of a new length starts empty.
    fn size_ring(&mut self) {
        let room = self.room();
        if self.ring.len() != room {
            self.ring = vec![[0.0; 2]; room];
            self.write = 0;
            self.base = 0;
            self.fade_base = 0;
            self.fading = 0;
            self.hold_saved = 0;
            self.hold_base = 0;
        }
    }

    /// What the tempo, the rate and the PPQ set (`FUN_00841ac0`): the
    /// length of a bar, the envelopes' step, and where the reader is
    /// against the ring's newest frame. A change of more than 8 frames in
    /// two bars while the song plays fades from the reader before it over
    /// a millisecond.
    fn retime(&mut self) {
        let old = self.span;
        let whole = (self.rate as i32).saturating_mul(240) as f32;
        self.bar = f64::from(whole / self.tempo as f32);
        self.span = self.bar * 2.0;
        self.beat_step = 4.0 / self.bar;
        // Below 10 BPM the reader stops at the ring's oldest frames.
        let len = self.ring.len() as i64;
        let behind = ((self.span + 0.5).round_ties_even() as i64 + LATENCY as i64)
            .clamp(LATENCY as i64, len - LATENCY as i64);
        let base = (self.write as i64 - behind).rem_euclid(len) as usize;
        if self.playing && old > 0.0 && (self.span - old).abs() > 8.0 {
            self.fade_base = self.base;
            self.fade_span = old;
            self.fading = self.fade;
        }
        self.base = base;
        self.hold_saved = base;
    }

    /// Choose `slot` for lane `lane` now (`FUN_00844060`): the run starts
    /// over at the song's place in the slot's position sync length, and
    /// the hold and the link follow.
    fn choose(&mut self, lane: usize, slot: usize, linked: bool) {
        let now = self.now();
        let was_linked = self.lanes[lane].slot().link;
        let mut slot = slot.min(SLOTS - 1);
        if lane == 1 && self.lanes[0].slot().link {
            slot = self.lanes[0].current;
            self.lanes[1].at = NEVER;
        }
        self.lanes[lane].current = slot;
        let chosen = &self.lanes[lane].slots[slot];
        let (hold, position, one_shot, link) =
            (chosen.hold, chosen.position, chosen.one_shot, chosen.link);
        let mut run = chosen.envelope.start();
        let bit = 2 << lane;
        if hold < 1 {
            let before = self.holds;
            self.holds &= !bit;
            if before > 0 && self.holds == 0 {
                self.hold_at = AT_ONCE;
            }
        } else {
            self.holds |= bit;
            if self.hold_at == NEVER {
                self.hold_at = if hold == 1 {
                    AT_ONCE
                } else {
                    now.saturating_add(self.sync(HOLD_LENGTH))
                };
            }
        }
        let mut place = 0.0;
        if position > 0 {
            let length = f64::from(self.sync(position));
            let mut at = f64::from(now);
            if self.holds > 0 && self.hold_at != NEVER {
                at = if self.hold_at == AT_ONCE {
                    0.0
                } else {
                    at + f64::from(i32::from(self.ppq) << 8) - f64::from(self.hold_at)
                };
            }
            if at > 0.0 {
                let whole = (at / length).trunc();
                place = (at - whole * length) / f64::from(self.ppq.max(1));
            }
        }
        let envelope = &self.lanes[lane].slots[slot].envelope;
        envelope.step(&mut run, place + self.lead * self.beat_step);
        self.lanes[lane].run = run;
        if one_shot {
            self.lanes[lane].pending = 0;
            self.lanes[lane].at = now.saturating_add(self.sync(5));
        }
        if linked && (link || was_linked) {
            let wanted = self.lanes[1].wanted;
            self.choose(1, wanted, false);
        }
    }

    /// The hold knob (`ProcessParam` 7): it counts only where it turns the
    /// hold on or off, and a hold it starts waits for the next bar.
    fn hold_knob(&mut self, value: i32) {
        if (self.holds > 0) != (value > 0) {
            self.holds = (self.holds & !1) | value;
            if self.hold_at == NEVER {
                let length = self.sync(HOLD_LENGTH);
                let now = self.now();
                self.hold_at = (now.saturating_add(length - 1) / length).saturating_mul(length);
            }
        }
    }

    /// The parameter asks lane `lane` for another slot (`FUN_00843fe0`):
    /// at once, or at the next multiple of the slot's trigger sync.
    fn ask(&mut self, lane: usize, slot: usize) {
        if lane == 1 && self.lanes[0].slot().link {
            return;
        }
        let length = self.sync(self.lanes[lane].slots[slot].trigger);
        if length == 0 {
            self.lanes[lane].at = NEVER;
            self.choose(lane, slot, true);
        } else {
            let now = self.now();
            let at = (now.saturating_add(length - 1) / length).saturating_mul(length);
            self.lanes[lane].at = at;
            self.lanes[lane].pending = self.lanes[lane].wanted;
        }
    }

    /// The song started or moved (`FUN_00844320`).
    fn moved(&mut self) {
        if self.holding {
            self.base = self.hold_saved;
        }
        self.holding = false;
        self.hold_at = NEVER;
        if self.holds > 0 {
            self.hold_at = self.now().saturating_add(self.sync(HOLD_LENGTH));
        }
        self.lanes[0].at = NEVER;
        self.lanes[1].at = NEVER;
        let (time, volume) = (self.lanes[0].wanted, self.lanes[1].wanted);
        self.choose(0, time, true);
        self.choose(1, volume, true);
    }

    /// Forget the past (`FUN_00843450`).
    fn flush(&mut self) {
        self.dc_state = [0.0; 4];
        self.ring.fill([0.0; 2]);
        self.delay = [[0.0; 2]; LATENCY];
        self.first = true;
        self.last_place = 0.0;
        self.speed.state = 1.0;
        self.last_wet = [0.0; 2];
        self.correction = [0.0; 2];
        self.last_out = [0.0; 2];
        self.fading = 0;
    }

    /// The song stopped (`FUN_00841cf0` with 0). A hold that was playing
    /// stays marked, so that the next start puts the reader back.
    fn stop(&mut self) {
        self.playing = false;
        self.fading = 0;
        self.holds &= 1;
        self.hold_at = NEVER;
    }

    /// The taps the reader has when it is not HQ: 8, or 16 with HQ on.
    fn quality(&self) -> usize {
        if self.hq { 16 } else { 8 }
    }

    /// Read `n` frames at `places` frames on from `base` into `out`.
    fn read(
        &self,
        base: usize,
        n: usize,
        places: &[f64],
        sinc: Sinc,
        wide: bool,
        out: &mut [[f32; 2]],
    ) {
        let len = self.ring.len() as i64;
        for i in 0..n {
            let place = places[i];
            let whole = place.trunc();
            let at = (base as i64 + i as i64 + whole as i64).rem_euclid(len) as usize;
            let (cut, cos2) = if wide {
                (self.work.cut[i], self.work.cos2[i])
            } else {
                (self.cut, self.cos2)
            };
            out[i] = sinc_at(&self.ring, at, place - whole, sinc, cut, cos2);
        }
    }

    /// One block of at most [`CHUNK`] frames (`Eff_Render`, `FUN_008420f0`).
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len()).min(CHUNK);
        if n == 0 {
            return;
        }
        if !self.playing {
            if self.last_out != [0.0; 2] {
                let span = (n - 1).max(1) as f64;
                for i in 0..n {
                    let w = ((((i as f64 * PI) / span).cos() + 1.0) * 0.5) as f32;
                    let keep = 1.0 - f64::from(w);
                    left[i] = (f64::from(left[i]) * keep + f64::from(self.last_out[0] * w)) as f32;
                    right[i] =
                        (f64::from(right[i]) * keep + f64::from(self.last_out[1] * w)) as f32;
                }
                self.last_out = [0.0; 2];
            }
            return;
        }
        let now = self.now();
        for lane in 0..2 {
            if self.lanes[lane].at <= now {
                self.lanes[lane].at = NEVER;
                let pending = self.lanes[lane].pending;
                self.choose(lane, pending, true);
            }
        }
        let len = self.ring.len();
        // Hold: from the bar before the hold, over and over.
        let mut jumped = false;
        if self.hold_at <= now {
            if self.holds < 1 {
                if self.holding {
                    self.holding = false;
                    self.base = self.hold_saved;
                    self.hold_at = NEVER;
                    jumped = self.declick != 0;
                }
            } else {
                if !self.holding {
                    self.hold_saved = self.base;
                    let length = self.sync(HOLD_LENGTH);
                    let back = (self.bar * f64::from(length)
                        / f64::from(i32::from(self.ppq.max(1)) << 2))
                    .round_ties_even() as i64;
                    self.hold_base = (self.base as i64 - back).rem_euclid(len as i64) as usize;
                    self.holding = true;
                }
                self.base = self.hold_base;
                self.hold_at = now.saturating_add(self.sync(HOLD_LENGTH));
                jumped = self.declick != 0;
            }
        }
        // The input goes into the ring, and is held back 15 frames.
        if !self.holding {
            for i in 0..n {
                self.ring[self.write] = [left[i], right[i]];
                self.write = (self.write + 1) % len;
            }
        }
        for i in 0..n {
            let out = self.delay[self.delay_at];
            self.delay[self.delay_at] = [left[i], right[i]];
            self.delay_at = (self.delay_at + 1) % LATENCY;
            left[i] = out[0];
            right[i] = out[1];
        }
        let scratching = self.scratch < 0.0
            || self
                .scratchers
                .iter()
                .any(|s| f64::from(s.state - self.scratch).abs() > f64::from(TINY));
        let time = self.lanes[0].slot();
        let on = (!time.envelope.flat && self.params[TIME_MIX] != 0)
            || self.holding
            || scratching
            || self.correction[0] != 0.0
            || self.correction[1] != 0.0
            || jumped;
        if on {
            self.time(n, left, right, scratching, jumped);
        } else {
            let lane = &mut self.lanes[0];
            let by = n as f64 * self.beat_step;
            lane.slots[lane.current].envelope.step(&mut lane.run, by);
            self.last_place = self.span;
            self.base = (self.base + n) % len;
            self.fading = 0;
            for i in 0..n {
                self.work.wet[i] = [left[i], right[i]];
            }
        }
        // The volume envelope.
        let lane = &mut self.lanes[1];
        let envelope = &lane.slots[lane.current].envelope;
        let mix = self.params[VOLUME_MIX];
        let gain = &mut self.work.gain[..n];
        if !envelope.flat && mix != 0 {
            for g in gain.iter_mut() {
                *g = envelope.step(&mut lane.run, self.beat_step) as f32;
            }
            self.bend.apply(gain);
            if mix != 256 {
                let m = mix as f32 * (1.0 / 256.0);
                for g in gain.iter_mut() {
                    *g = *g * m + (1.0 - m);
                }
            }
        } else {
            envelope.step(&mut lane.run, n as f64 * self.beat_step);
            gain.fill(1.0);
        }
        if self.first {
            for smoother in &mut self.smoothers {
                smoother.state = gain[0];
            }
        }
        for smoother in &mut self.smoothers {
            smoother.run(gain);
        }
        for i in 0..n {
            let [l, r] = self.work.wet[i];
            left[i] = l * gain[i];
            right[i] = r * gain[i];
        }
        if self.dc {
            self.block_dc(&mut left[..n], &mut right[..n]);
        }
        self.last_out = [left[n - 1], right[n - 1]];
        self.first = false;
    }

    /// The time envelope's half of a block: `left` and `right` hold the
    /// dry signal, and `work.wet` gets what the envelope reads.
    fn time(&mut self, n: usize, left: &[f32], right: &[f32], scratching: bool, jumped: bool) {
        let len = self.ring.len();
        let hq = self.quality() >= 9;
        let lane = &mut self.lanes[0];
        let envelope = &lane.slots[lane.current].envelope;
        let places = &mut self.work.places[..n];
        for place in places.iter_mut() {
            *place = envelope.step(&mut lane.run, self.beat_step);
        }
        if scratching {
            let push = &mut self.work.push[..n];
            push.fill(self.scratch);
            if self.first {
                for s in &mut self.scratchers {
                    s.state = push[0];
                }
            }
            for s in &mut self.scratchers {
                s.run(push);
            }
            for (place, push) in places.iter_mut().zip(push.iter()) {
                *place += f64::from(*push);
                if *place < 0.0 {
                    *place = 0.0;
                }
            }
        }
        if self.fading > 0 {
            self.work.levels[..n].copy_from_slice(places);
        }
        for place in places.iter_mut() {
            *place *= self.span;
        }
        if self.first {
            self.last_place = places[0];
        }
        let jumps = &mut self.work.jumps[..n];
        let mut wide = false;
        let taps;
        if !hq {
            if self.declick != 0 {
                for (jump, place) in jumps.iter_mut().zip(places.iter()) {
                    *jump = ((*place - self.last_place) as f32 + 1.0).abs();
                    self.last_place = *place;
                }
            }
            self.last_place = places[n - 1];
            taps = 8;
        } else {
            let speed = &mut self.work.speed[..n];
            for (s, place) in speed.iter_mut().zip(places.iter()) {
                *s = ((*place - self.last_place) as f32 + 1.0).abs();
                self.last_place = *place;
            }
            if self.declick != 0 {
                jumps.copy_from_slice(speed);
            }
            self.speed.run(speed);
            let low = speed.iter().copied().fold(f32::INFINITY, f32::min);
            let high = speed.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            if high > 1.0 + TINY {
                wide = true;
                taps = 16;
                let cuts = self.work.cut.iter_mut().zip(self.work.cos2.iter_mut());
                for (s, (cut, cos2)) in speed.iter().zip(cuts) {
                    *cut = PI / f64::from(s.max(1.0));
                    *cos2 = cut.cos() * 2.0;
                }
            } else {
                self.cut = PI;
                self.cos2 = -2.0;
                taps = if low >= 1.0 - TINY { 8 } else { 16 };
            }
        }
        if self.reader.taps != taps {
            self.reader = Sinc::new(taps);
        }
        let mut wet = std::mem::take(&mut self.work.wet);
        self.read(
            self.base,
            n,
            &self.work.places[..n],
            self.reader,
            wide,
            &mut wet,
        );
        self.base = (self.base + n) % len;
        if self.fading > 0 {
            let mut old = std::mem::take(&mut self.work.old);
            let places = &mut self.work.places[..n];
            for (place, level) in places.iter_mut().zip(self.work.levels[..n].iter()) {
                *place = *level * self.fade_span;
            }
            self.read(
                self.fade_base,
                n,
                &self.work.places[..n],
                Sinc::new(16),
                wide,
                &mut old,
            );
            let span = self.fading.min(n) as f64;
            for (i, (new, old)) in wet[..n].iter_mut().zip(old.iter()).enumerate() {
                let t = (i as f64 / span).clamp(0.0, 1.0);
                for (new, old) in new.iter_mut().zip(old) {
                    *new = (f64::from(*new) * t + f64::from(*old) * (1.0 - t)) as f32;
                }
            }
            self.work.old = old;
            self.fading = 0;
        }
        if self.declick != 0 {
            if jumped {
                self.work.jumps[0] = self.jump * 2.0;
            }
            for (frame, jump) in wet[..n].iter_mut().zip(self.work.jumps.iter()) {
                if *jump > self.jump {
                    for ((correction, last), value) in
                        self.correction.iter_mut().zip(self.last_wet).zip(*frame)
                    {
                        *correction = (*correction + last) - value;
                    }
                }
                self.last_wet = *frame;
                for (value, correction) in frame.iter_mut().zip(&mut self.correction) {
                    *value += *correction;
                    *correction *= self.decay;
                }
            }
            for c in &mut self.correction {
                if c.abs() < TINY {
                    *c = 0.0;
                }
            }
        }
        let mix = self.params[TIME_MIX];
        if mix != 256 {
            let m = mix as f32 * (1.0 / 256.0);
            let rest = 1.0 - m;
            for i in 0..n {
                wet[i][0] = wet[i][0] * m + left[i] * rest;
                wet[i][1] = wet[i][1] * m + right[i] * rest;
            }
        }
        self.work.wet = wet;
    }

    /// FL's DC blocker (`FUN_0062bf00`), a side at a time.
    fn block_dc(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (a, b) = (f64::from(self.dc_k), f64::from(DC_DAMP));
        for (side, block) in [left, right].into_iter().enumerate() {
            let (mut level, mut speed) = (self.dc_state[side * 2], self.dc_state[side * 2 + 1]);
            for x in block.iter_mut() {
                let v = f64::from(*x);
                speed += (v - level) * a;
                level += speed;
                speed *= b;
                *x = (v - level) as f32;
            }
            self.dc_state[side * 2] = level;
            self.dc_state[side * 2 + 1] = speed;
        }
    }

    /// Read one slot of a state.
    fn read_slot(reader: &mut Reader<'_>) -> Result<Slot, StateError> {
        let mut len = usize::from(reader.u8()?);
        if len == 0xff {
            len = reader.u32()? as usize;
        }
        reader.bytes(len)?;
        let trigger = reader.i32()?;
        let position = reader.i32()?;
        let hold = reader.i32()?;
        let flags = reader.bytes(3)?;
        for (param, value) in [
            (PARAMS, trigger),
            (PARAMS + 1, position),
            (PARAMS + 2, hold),
        ] {
            let top = if param == PARAMS + 2 { 2 } else { 5 };
            if !(0..=top).contains(&value) {
                return Err(StateError::Range { param, value });
            }
        }
        let stream = MEnvelope::read(reader, Class::Map)?;
        Ok(Slot {
            trigger: trigger as usize,
            position: position as usize,
            hold,
            link: flags[0] != 0,
            key_held: flags[1] != 0,
            one_shot: flags[2] != 0,
            envelope: Envelope::new(&stream.points),
        })
    }
}

impl Effect for GrossBeat {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let mut reader = Reader::new(bytes);
        let raw = reader.i32()?;
        if raw > NEWEST {
            return Err(StateError::Version { found: raw });
        }
        let version = if raw > 0xffff { raw >> 16 } else { raw };
        let mut params = self.params;
        params[SCRATCH] = 0x1_0000;
        let count = if version < 2 { 8 } else { 9 };
        for (param, value) in params.iter_mut().enumerate().take(count) {
            let read = reader.i32()?;
            let (min, max) = RANGES[param];
            if read < min || read > max {
                return Err(StateError::Range { param, value: read });
            }
            *value = read;
        }
        let options = if version >= 5 {
            9
        } else if version >= 3 {
            8
        } else {
            7
        };
        let options = reader.bytes(options)?;
        let mut slots = Vec::with_capacity(2 * SLOTS);
        for _ in 0..2 * SLOTS {
            slots.push(Self::read_slot(&mut reader)?);
        }
        let midi = if version > 8 { reader.u8()? != 0 } else { true };
        reader.end()?;
        self.params = params;
        self.dc = options[4] != 0;
        self.compensate = options[5] != 0;
        self.hq = options[6] != 0;
        self.declick = options.get(8).map_or(1, |d| usize::from(*d).min(2));
        self.midi = midi;
        let volume = slots.split_off(SLOTS);
        self.lanes[0].slots = slots;
        self.lanes[1].slots = volume;
        for (lane, param) in [(0, TIME_SLOT), (1, VOLUME_SLOT)] {
            self.lanes[lane].wanted = self.params[param] as usize;
            self.lanes[lane].current = self.lanes[lane].current.min(SLOTS - 1);
        }
        self.reader = Sinc::new(self.quality());
        self.cut = PI;
        self.cos2 = -2.0;
        self.at_rate();
        self.knobs();
        self.hold_knob(self.params[HOLD]);
        if self.playing {
            self.moved();
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 && hz.is_finite() {
            self.rate = hz;
            self.at_rate();
            self.knobs();
            self.retime();
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm.is_finite() && bpm != self.tempo {
            self.tempo = bpm;
            self.retime();
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        if ppq > 0 && ppq != self.ppq {
            self.ppq = ppq;
            self.retime();
        }
    }

    fn tick(&mut self, ppq_tick: u64, _tempo: f64) {
        let follows = self
            .tick
            .is_some_and(|tick| ppq_tick == tick || ppq_tick == tick + 1);
        self.tick = Some(ppq_tick);
        self.elapsed = 0;
        if !self.playing {
            self.playing = true;
            self.flush();
            self.moved();
        } else if !follows {
            self.moved();
        }
    }

    fn follows_song(&self) -> bool {
        true
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let mut at = 0;
        while at < len {
            let end = (at + CHUNK).min(len);
            self.render(&mut left[at..end], &mut right[at..end]);
            self.elapsed += end - at;
            at = end;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(range) = RANGES.get(index) else {
            return;
        };
        let value = knob_value(value, *range);
        let before = self.params[index];
        self.params[index] = value;
        match index {
            TIME_SLOT | VOLUME_SLOT => {
                let lane = index;
                if self.lanes[lane].wanted != value as usize {
                    self.lanes[lane].wanted = value as usize;
                    if self.playing {
                        self.ask(lane, value as usize);
                    }
                }
            }
            HOLD => self.hold_knob(value),
            ATTACK | RELEASE | TENSION | SCRATCH if before != value => self.knobs(),
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.stop();
        self.flush();
        self.tick = None;
        self.elapsed = 0;
    }

    fn fidelity(&self) -> Fidelity {
        if self.params[TENSION] != 0 && self.params[VOLUME_MIX] != 0 {
            Fidelity::Approximate(
                "Gross Beat's tension curve takes exp in full where FL's is 11-bit",
            )
        } else {
            Fidelity::Exact
        }
    }

    fn latency(&self) -> usize {
        LATENCY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// A rate and a tempo that make a bar 65536 frames and a frame 2^-14
    /// of a beat, so that an envelope's steps add up without rounding.
    const RATE: f32 = 32_768.0;
    const TEMPO: f64 = 120.0;
    const PPQ: u16 = 96;
    const BAR: usize = 65_536;

    /// An envelope stream of version 3 through these points: `x` after
    /// the point before, `y` and the mode.
    fn envelope(points: &[(f64, f64, u8)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for word in [3u32, 3, points.len() as u32] {
            bytes.extend(word.to_le_bytes());
        }
        for (x, y, mode) in points {
            bytes.extend(x.to_le_bytes());
            bytes.extend(y.to_le_bytes());
            bytes.extend(0f32.to_le_bytes());
            bytes.extend([*mode, 0, 0, 0]);
        }
        bytes.extend(0u32.to_le_bytes());
        for word in [-1i32, -1, -1, -1] {
            bytes.extend(word.to_le_bytes());
        }
        bytes
    }

    /// A slot with no name, its trigger and position sync, and this
    /// envelope.
    fn slot(trigger: i32, position: i32, envelope: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0];
        for word in [trigger, position, 0] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend([0, 0, 0]);
        bytes.extend(envelope);
        bytes
    }

    fn flat() -> Vec<u8> {
        envelope(&[(0.0, 1.0, 2), (4.0, 1.0, 2)])
    }

    /// A version 8 state: these parameters, the options HQ on and click
    /// reduction low, and every slot flat but the ones given.
    fn state(
        params: [i32; PARAMS],
        time: &[(usize, Vec<u8>)],
        volume: &[(usize, Vec<u8>)],
    ) -> Vec<u8> {
        let mut bytes = 0x8_0003i32.to_le_bytes().to_vec();
        bytes.extend(testing::state(&params));
        bytes.extend([0, 0, 0, 0, 0, 0, 1, 1, 1]);
        for given in [time, volume] {
            for index in 0..SLOTS {
                match given.iter().find(|(at, _)| *at == index) {
                    Some((_, slot)) => bytes.extend(slot),
                    None => bytes.extend(slot(5, 5, &flat())),
                }
            }
        }
        bytes
    }

    /// The parameters with the smoothing off.
    fn sharp() -> [i32; PARAMS] {
        let mut params = DEFAULTS;
        params[ATTACK] = 0;
        params[RELEASE] = 0;
        params
    }

    fn plugin(bytes: &[u8]) -> GrossBeat {
        let mut plugin = GrossBeat::default();
        plugin.set_sample_rate(RATE);
        plugin.set_tempo(TEMPO);
        plugin.set_ppq(PPQ);
        plugin.set_state(bytes).unwrap();
        plugin
    }

    /// The first frame of tick `tick`, as the engine's clock has it.
    fn frame(tick: u64) -> usize {
        (tick as f64 * f64::from(RATE) * 60.0 / (TEMPO * f64::from(PPQ))).round() as usize
    }

    /// Play `input` from tick `from` on, a tick at a time, with `before`
    /// called ahead of each tick with the tick's number.
    fn play(
        plugin: &mut GrossBeat,
        from: u64,
        input: &[f32],
        mut before: impl FnMut(&mut GrossBeat, u64),
    ) -> Vec<f32> {
        let (mut left, mut right) = (input.to_vec(), input.to_vec());
        let start = frame(from);
        let mut tick = from;
        while frame(tick) - start < input.len() {
            let (a, b) = (
                frame(tick) - start,
                (frame(tick + 1) - start).min(input.len()),
            );
            before(plugin, tick);
            plugin.tick(tick, TEMPO);
            plugin.process(&mut left[a..b], &mut right[a..b]);
            tick += 1;
        }
        assert_eq!(left, right);
        left
    }

    #[test]
    fn a_bar_is_65536_frames_and_a_frame_a_power_of_two_of_a_beat() {
        let plugin = plugin(&state(sharp(), &[], &[]));
        assert_eq!(plugin.bar, BAR as f64);
        assert_eq!(plugin.beat_step, 1.0 / 16_384.0);
        assert_eq!(plugin.latency(), 15);
        assert_eq!((frame(1), frame(96)), (171, 16_384));
    }

    #[test]
    fn a_volume_step_at_two_beats_lands_on_its_frame() {
        // 1 up to beat 2, then 0 to the end of the bar.
        let gate = envelope(&[(0.0, 1.0, 2), (2.0, 0.0, 2), (2.0, 0.0, 2)]);
        let mut plugin = plugin(&state(sharp(), &[], &[(0, slot(5, 5, &gate))]));
        let out = play(&mut plugin, 0, &vec![1.0; 2 * BAR], |_, _| {});
        // The dry signal is 15 frames late, and the envelope is read a
        // step on from where it is, so beat 2 is frame 32767.
        assert!(out[..15].iter().all(|v| *v == 0.0));
        assert!(out[15..32_767].iter().all(|v| *v == 1.0));
        assert!(out[32_767..65_535].iter().all(|v| *v == 0.0));
        // And round again from the start of the next bar.
        assert!(out[65_535..98_303].iter().all(|v| *v == 1.0));
        assert_eq!(out[98_303], 0.0);
    }

    #[test]
    fn the_smoothing_takes_the_attack_and_release_times() {
        let gate = envelope(&[(0.0, 1.0, 2), (2.0, 0.0, 2), (2.0, 0.0, 2)]);
        let mut params = sharp();
        params[RELEASE] = 5000;
        let mut plugin = plugin(&state(params, &[], &[(0, slot(5, 5, &gate))]));
        let out = play(&mut plugin, 0, &vec![1.0; BAR], |_, _| {});
        // Two one-poles of (501^0.5 - 1) ms times 2 over root 2 each.
        let ms = f64::from(((0.5 * 501f64.ln()).exp() - 1.0) as f32);
        let time = ms * 0.001 * f64::from(RATE) * 2.0 / 2f64.sqrt();
        let k = (0.01f64.ln() / time).exp();
        let mut stages = [1.0f64; 2];
        for (n, got) in out[32_767..40_000].iter().enumerate() {
            for stage in 0..2 {
                let input = if stage == 0 { 0.0 } else { stages[0] };
                stages[stage] = input + k * (stages[stage] - input);
            }
            assert!((f64::from(*got) - stages[1]).abs() < 1e-4, "{n}");
        }
    }

    #[test]
    fn a_flat_time_envelope_halfway_down_reads_a_bar_back() {
        let back = envelope(&[(0.0, 0.5, 2), (4.0, 0.5, 2)]);
        let mut plugin = plugin(&state(sharp(), &[(0, slot(5, 5, &back))], &[]));
        let mut input = vec![0.0; 2 * BAR];
        input[1000] = 1.0;
        let out = play(&mut plugin, 0, &input, |_, _| {});
        // Two bars are 131072 frames, which is even, so the reader starts
        // 131072 + 15 frames back and the envelope brings it a bar on.
        let at = 1000 + BAR + 15;
        assert!((out[at] - 1.0).abs() < 1e-4, "{}", out[at]);
        let rest = out.iter().enumerate().filter(|(n, _)| *n != at);
        assert!(rest.map(|(_, v)| v.abs()).fold(0.0, f32::max) < 1e-4);
    }

    #[test]
    fn the_sinc_passes_a_slow_sine_between_its_frames() {
        let turns = 0.01;
        let ring: Vec<[f32; 2]> = testing::sine(turns, 4096)
            .into_iter()
            .map(|v| [v, -v])
            .collect();
        for taps in [8, 16] {
            for frac in [0.0, 0.25, 0.5, 0.9] {
                let got = sinc_at(&ring, 2000, frac, Sinc::new(taps), PI, -2.0);
                let want = (std::f64::consts::TAU * turns * (2000.0 + frac)).sin();
                assert!((f64::from(got[0]) - want).abs() < 2e-3, "{taps} {frac}");
                assert!((f64::from(got[1]) + want).abs() < 2e-3, "{taps} {frac}");
            }
        }
        // Read twice as fast, it halves the band: a tone near the top is
        // gone and a slow one stays.
        let fast = testing::sine(0.45, 4096);
        let fast: Vec<[f32; 2]> = fast.into_iter().map(|v| [v, v]).collect();
        let cut = PI / 2.0;
        let worst = (1000..1100)
            .map(|at| sinc_at(&fast, at, 0.3, Sinc::new(16), cut, cut.cos() * 2.0)[0].abs())
            .fold(0.0, f32::max);
        assert!(worst < 0.05, "{worst}");
    }

    #[test]
    fn a_seek_puts_the_envelope_where_the_song_is_in_its_bar() {
        let gate = envelope(&[(0.0, 1.0, 2), (2.0, 0.0, 2), (2.0, 0.0, 2)]);
        for (position, after) in [(5, 0.0), (0, 1.0)] {
            let bytes = state(sharp(), &[], &[(0, slot(5, position, &gate))]);
            let mut plugin = plugin(&bytes);
            play(&mut plugin, 0, &vec![1.0; 1000], |_, _| {});
            // On to beat 2 of a later bar: in 4-beat sync the envelope is
            // past its step at once, and with the sync off it starts over.
            let out = play(&mut plugin, 96 * 6, &vec![1.0; 500], |_, _| {});
            assert!(out[20..].iter().all(|v| *v == after), "{position}");
        }
    }

    #[test]
    fn a_slot_waits_for_its_trigger_sync() {
        let silent = envelope(&[(0.0, 0.0, 2), (4.0, 0.0, 2)]);
        // Slot 1 is silent and waits for the next beat.
        let mut plugin = plugin(&state(sharp(), &[], &[(1, slot(3, 5, &silent))]));
        let out = play(&mut plugin, 0, &vec![1.0; 20_000], |plugin, tick| {
            if tick == 10 {
                plugin.set_param(VOLUME_SLOT, 1.0);
            }
        });
        assert_eq!(plugin.param(VOLUME_SLOT), 1.0);
        assert!(out[15..frame(96)].iter().all(|v| *v == 1.0));
        assert!(out[frame(96)..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_hold_of_the_last_bar_loops_the_bar_before_it() {
        // Time slot 1 holds the last bar and is chosen at once, from the
        // start of bar 2. Click reduction is off.
        let hold = {
            let mut bytes = slot(0, 5, &flat());
            bytes[9..13].copy_from_slice(&1i32.to_le_bytes());
            bytes
        };
        let mut bytes = state(sharp(), &[(1, hold)], &[]);
        bytes[4 + 4 * PARAMS + 8] = 0;
        let mut plugin = plugin(&bytes);
        let input: Vec<f32> = (0..4 * BAR)
            .map(|n| ((n * 7919) % 1000) as f32 / 1000.0 - 0.5)
            .collect();
        let out = play(&mut plugin, 0, &input, |plugin, tick| {
            if tick == 4 * u64::from(PPQ) {
                plugin.set_param(TIME_SLOT, 1.0);
            }
        });
        let start = frame(4 * u64::from(PPQ));
        assert_eq!(start, BAR);
        // Up to the hold the dry signal 15 frames late, then the bar
        // before the hold over and over.
        assert!((15..start).all(|f| out[f] == input[f - 15]));
        let held = |f: usize| {
            let from = (f - start) % BAR + start;
            from.checked_sub(BAR + 15).map_or(0.0, |at| input[at])
        };
        let worst = (start..4 * BAR)
            .map(|f| (out[f] - held(f)).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn a_stopped_plugin_passes_its_input_through_on_time() {
        let mut plugin = plugin(&state(sharp(), &[], &[]));
        let (mut left, mut right) = (vec![0.5; 64], vec![-0.5; 64]);
        plugin.process(&mut left, &mut right);
        assert_eq!((left, right), (vec![0.5; 64], vec![-0.5; 64]));
        assert!(plugin.follows_song());
    }

    #[test]
    fn the_rate_sizes_the_ring_once_and_a_tempo_change_keeps_it() {
        let mut plugin = GrossBeat::default();
        plugin.set_sample_rate(48_000.0);
        // Three bars at 10 BPM, 16 frames, a block and 64 guard frames.
        let room = 48_000 * 72 + 16 + CHUNK + 64;
        assert_eq!(plugin.ring.len(), room);
        let ring = plugin.ring.as_ptr();
        for bpm in [10.0, 300.0, 5.0, 60.0] {
            plugin.set_tempo(bpm);
            assert_eq!((plugin.ring.as_ptr(), plugin.ring.len()), (ring, room));
            assert!(plugin.base < room);
        }
        plugin.set_sample_rate(96_000.0);
        assert_eq!(plugin.ring.len(), 96_000 * 72 + 16 + CHUNK + 64);
    }

    #[test]
    fn a_state_too_new_or_out_of_range_is_turned_down() {
        let mut bytes = state(sharp(), &[], &[]);
        bytes[..4].copy_from_slice(&0x9_0004i32.to_le_bytes());
        let mut plugin = GrossBeat::default();
        assert_eq!(
            plugin.set_state(&bytes),
            Err(StateError::Version { found: 0x9_0004 })
        );
        let mut params = sharp();
        params[TIME_SLOT] = 36;
        let bytes = state(params, &[], &[]);
        assert_eq!(
            plugin.set_state(&bytes),
            Err(StateError::Range {
                param: 0,
                value: 36
            })
        );
        let bytes = state(sharp(), &[], &[]);
        assert_eq!(
            plugin.set_state(&bytes[..bytes.len() - 1]).map_err(|_| ()),
            Err(())
        );
        assert_eq!(plugin.set_state(&bytes), Ok(()));
        assert_eq!(plugin.param(ATTACK), 0.0);
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states of Gross Beat skipped");
            return;
        };
        let mut saved = testing::projects(&root);
        saved.extend(testing::presets(NAME).unwrap_or_default());
        let saved: Vec<_> = saved
            .into_iter()
            .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert_eq!(saved.len(), 35 + 9);
        let tone = testing::sine(0.013, 3 * BAR / 4);
        for saved in &saved {
            let mut plugin = GrossBeat::default();
            plugin.set_sample_rate(44_100.0);
            plugin.set_tempo(128.0);
            plugin.set_state(&saved.state).unwrap();
            if saved.file == "Default" {
                assert_eq!(plugin.params, DEFAULTS);
            }
            let out = play(&mut plugin, 0, &tone, |_, _| {});
            let peak = out.iter().map(|v| v.abs()).fold(0.0, f32::max);
            assert!(
                out.iter().all(|v| v.is_finite()) && peak < 2.0,
                "{}: {peak}",
                saved.file
            );
        }
    }
}
