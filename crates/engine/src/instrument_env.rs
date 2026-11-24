//! A channel's instrument envelopes and LFOs on the voices FL's host plays
//! itself, a sampler's and an audio clip's: the five 218 `EnvelopeLFO`
//! records, which move the voice's pan, volume, mod X, mod Y and pitch.
//! `docs/rec-channel.md`, "Instrument envelopes and LFOs", has where it
//! was read in `FLEngine_x64.dll`:
//!
//! - `FUN_009ce9e0` works a record's times out in ticks for the song's
//!   PPQ and tempo, `FUN_009cedb0` starts a voice's envelope, `FUN_009ce490`
//!   steps it once a tick, and `FUN_009cecc0` lets it go at the note off.
//!   They are the same unit as 3x Osc's copy, which
//!   [`plugins::three_osc::Block`] and [`plugins::three_osc::Envelope`]
//!   build, and those are what this runs.
//! - `FUN_00d24500` steps the five once a tick and makes what they move:
//!   the pan `lfo * lfo amount * 2`, the volume `env * (lfo * lfo amount +
//!   1)` held at 0 and up (the envelope's amount is not read), mod X and Y
//!   `env * amount + lfo * lfo amount`, and the pitch in cents `env *
//!   amount + lfo * lfo amount` with the amounts as the record's integers.
//! - The voice's render (`FUN_00d24ae0`) puts the pan and the volume on
//!   its gains, `ComputeLRVol(FinalLevels.Pan + pan, FinalLevels.Vol *
//!   volume)`, moving them from where they were over the tick by no more
//!   than `0.001 * 44100 / rate` a sample (`FUN_006a3810`); the speed
//!   reads `FinalLevels.Pitch + pitch` once a tick (`FUN_00d26890`); the
//!   filter takes `FinalLevels.FCut + mod X` and `FRes + mod Y`
//!   (`FUN_00d24650`).
//! - At the note off (`FUN_00d241e0`) the volume, mod X, mod Y and pitch
//!   envelopes release. A voice whose volume envelope is on then sounds
//!   until it is over (stage 9), and is cut at the end of that tick; one
//!   whose volume envelope is off is cut at the note off, as before.
//!
//! The ticks are the song's, worked out from the tempo the note starts at:
//! a clip on the tempo moves them where the note is long. Building one
//! allocates; a voice that carries one allocates nothing as it plays.

use crate::Curves;
use model::{Channel, ChannelKind};
use plugins::three_osc::{Block, Envelope, HostTables, OVER, PITCH, RISE, VOLUME, ramp};
use std::sync::Arc;

/// The pan's record, the first of the five.
const PAN: usize = 0;
/// The mods' records, after the volume's.
const MODS: [usize; 2] = [2, 3];

/// The 17 words of a 218 record as FL's host keeps them at `smp+0x198 +
/// 0xa0 e`: the flags byte and its three bytes as one word, then the
/// fields in order. A payload under 68 bytes has its last fields 0 but
/// for the tensions its flags ask for (`flp::event::layout::EnvelopeLfo`).
fn words(record: &flp::event::layout::EnvelopeLfo) -> [i32; 17] {
    let r = record;
    let flags = i32::from_le_bytes([r.flags, r.unknown_1[0], r.unknown_1[1], r.unknown_1[2]]);
    let mut tensions = [r.unknown_56, r.unknown_60, r.unknown_64].map(i32::from_le_bytes);
    if r.len < 68 {
        if r.flags & 8 != 0 {
            tensions[0] = -101;
        }
        if r.flags & 0x40 != 0 {
            tensions[1] = -101;
        }
    }
    [
        flags,
        r.enabled,
        r.predelay,
        r.attack,
        r.hold,
        r.decay,
        r.sustain,
        r.release,
        r.amount,
        r.lfo_predelay,
        r.lfo_attack,
        r.lfo_amount,
        r.lfo_speed,
        r.lfo_shape,
        tensions[0],
        tensions[1],
        tensions[2],
    ]
}

/// Whether FL's host plays the voices of this channel itself, and so runs
/// its envelopes: a sampler and an audio clip channel. A plugin channel's
/// `chan+0x164` is 2 unless the plugin asks for the sampler
/// (`FUN_012bd640`, `FPF_UseSampler`), and at 2 the host lets go of none of
/// the envelopes (`FUN_00d241e0`); a generator that renders its own voices
/// is not run through the host's voice render either.
pub(crate) fn hosts_voices(channel: &Channel) -> bool {
    matches!(
        channel.kind,
        Some(ChannelKind::Sampler | ChannelKind::AudioClip)
    )
}

/// The channel's side of `FinalLevels.FCut` and `FRes` on a voice FL's
/// host plays, `(219 @12 + 229 @12) / 256` and the same of @16
/// (`FUN_00fea140`), and its filter type, 219 @20.
pub(crate) fn channel_filter(channel: &Channel) -> (i32, (f32, f32)) {
    let [x, y, x_offset, y_offset] = crate::automation::mods(channel);
    let per = 1.0f32 / 256.0;
    let kind = channel
        .levels
        .map_or(0, |l| i32::from_le_bytes(l.unknown_20));
    (
        kind,
        ((x + x_offset) as f32 * per, (y + y_offset) as f32 * per),
    )
}

/// What a voice on a channel FL's host plays itself is, for its
/// envelopes and its filter.
pub(crate) struct Played<'a> {
    pub channel: &'a Channel,
    /// The song's tick the voice starts on, and the tempo there.
    pub tick: u64,
    pub tempo: f64,
    pub ppq: u16,
    pub rate: u32,
    /// The voice's whole level and pan, as [`crate::voice::gains`] puts
    /// them together: its own, the channel's volume and pan.
    pub level: f32,
    pub pan: f32,
    /// The note's own mod X and mod Y with what tracking adds; 0 for a
    /// clip.
    pub mods: (f32, f32),
}

/// The voice, set to play its channel's envelopes and LFOs, and to run
/// its filter where tracking moves the note's mods. A voice of a channel
/// the host does not play, or whose records move nothing, is as it was.
pub(crate) fn articulate(
    voice: crate::voice::Voice,
    at: &Played,
    curves: &dyn Curves,
) -> crate::voice::Voice {
    let channel = at.channel;
    if !hosts_voices(channel) {
        return voice;
    }
    let (kind, (cut, res)) = channel_filter(channel);
    let base = (at.mods.0 + cut, at.mods.1 + res);
    let voice = match Shape::of(channel, at.tempo, at.ppq, at.rate) {
        Some(shape) => {
            let art = Articulation::new(shape, at.tick, (at.level, at.pan), curves);
            voice.articulated(art, at.rate, Some((kind, base)))
        }
        None => voice,
    };
    if tracks_filter(channel) {
        return voice.filtered(kind, at.rate, base);
    }
    voice
}

/// Whether the channel's velocity or key tracking moves a note's mod X or
/// mod Y, so that its sampler voices run the filter (`FUN_00d265e0` adds
/// it to `FinalLevels.FCut` and `FRes`).
pub(crate) fn tracks_filter(channel: &Channel) -> bool {
    channel.kind == Some(ChannelKind::Sampler) && channel.tracks_mods()
}

/// The plugins whose instrument tab is the channel's envelopes: FL's form
/// shows 3x Osc's tab with the channel's controls (tags 0x1000 + w,
/// `docs/plugins/3x-osc.md`, section 2), and its parameters from 23 are
/// the five records' 17 words.
fn tab_of(channel: &Channel) -> bool {
    channel.kind == Some(ChannelKind::Plugin)
        && channel
            .plugin_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(plugins::three_osc::NAME))
}

/// The first of 3x Osc's parameters that are its instrument tab.
const TAB_PARAM: u16 = 23;

/// The plugin parameter that channel control `control`, one of the
/// envelopes' (0x1000..0x14FF, `0x1000 + 0x100 e + f`), drives on a
/// channel whose plugin keeps the envelopes as its own (3x Osc).
pub(crate) fn tab_param(channel: &Channel, control: u16) -> Option<u16> {
    let (e, f) = (control.checked_sub(0x1000)? >> 8, control & 0xff);
    (tab_of(channel) && e < 5 && f < 17).then_some(TAB_PARAM + 17 * e + f)
}

/// Hand a plugin that keeps the channel's envelopes as its own tab the
/// channel's 218 records, when its state has no tab of its own: 3x Osc
/// below version 14 (`FUN_0072c250` loads the defaults), which FL 12 and
/// older wrote, when the envelopes lived in the channel alone. That FL
/// plays these from the channel is inferred: such projects automate the
/// tab's controls on the channel (0x1408 in `Treben - Gimbal and Sinan -
/// RawFL`), and a version 14 state carries the same words as the
/// channel's records in 24 of the corpus's 26.
pub(crate) fn hand_tab(channel: &Channel, generator: &mut dyn plugins::Generator) {
    let old = channel
        .plugin_state
        .as_deref()
        .is_none_or(|s| s.len() < 460);
    if !tab_of(channel) || !old || channel.envelopes.len() < 5 {
        return;
    }
    for (e, record) in channel.envelopes.iter().take(5).enumerate() {
        for (f, word) in words(record).into_iter().enumerate() {
            let param = usize::from(TAB_PARAM) + 17 * e + f;
            generator.set_param(param, word as f32);
        }
    }
}

/// What a channel's records move, worked out for a tempo, an output rate
/// and the song's PPQ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shape {
    blocks: [Block; 5],
    /// The volume envelope is on, or a volume or pan LFO: the gains move.
    gains: bool,
    /// The pan LFO is on.
    pans: bool,
    /// The pitch envelope or LFO moves the pitch.
    pitches: bool,
    /// An envelope or LFO moves mod X or mod Y.
    mods: bool,
    /// Output samples to a tick.
    per_tick: f64,
    /// The most a gain moves a sample, `0.001 * 44100 / rate`.
    limit: f32,
}

impl Shape {
    /// The records of `channel` at `tempo` BPM, `ppq` and `rate`. `None`
    /// when they move nothing.
    pub fn of(channel: &Channel, tempo: f64, ppq: u16, rate: u32) -> Option<Self> {
        if channel.envelopes.len() < 5 || tempo.is_nan() || tempo <= 0.0 || ppq == 0 {
            return None;
        }
        // FL keeps the PPQ over 4 as a whole number (`FUN_009ce3f0`).
        let ppq_ticks = f64::from(ppq / 4 * 4);
        let blocks: [Block; 5] =
            std::array::from_fn(|e| Block::of(&words(&channel.envelopes[e]), tempo, ppq_ticks));
        let moves = |e: usize, cents: bool| {
            let b = &blocks[e];
            let (amount, lfo) = if cents {
                (b.cents, b.lfo_cents)
            } else {
                (b.amount, b.lfo_amount)
            };
            (b.enabled && amount != 0.0) || lfo != 0.0
        };
        let volume = &blocks[VOLUME];
        let pans = blocks[PAN].lfo_amount != 0.0;
        let shape = Shape {
            gains: volume.enabled || volume.lfo_amount != 0.0 || pans,
            pans,
            pitches: moves(PITCH, true),
            mods: MODS.iter().any(|&e| moves(e, false)),
            per_tick: f64::from(rate) * 60.0 / (tempo * f64::from(ppq)),
            limit: (44_100.0 / f64::from(rate.max(1)) * 0.001) as f32,
            blocks,
        };
        // The LFOs read FL's tables, which are made the first time.
        HostTables::get();
        (shape.gains || shape.pitches || shape.mods).then_some(shape)
    }

    /// Whether the voice's speed may move.
    pub fn pitches(&self) -> bool {
        self.pitches
    }

    /// Whether mod X or mod Y moves, so that the voice runs its filter.
    pub fn mods(&self) -> bool {
        self.mods
    }

    /// The lowest the pitch envelope and LFO take a voice, in cents.
    pub fn lowest_cents(&self) -> f32 {
        if !self.pitches {
            return 0.0;
        }
        let b = &self.blocks[PITCH];
        let env = if b.enabled { b.cents.min(0.0) } else { 0.0 };
        env - b.lfo_cents.abs()
    }

    /// The output samples a voice may sound past its note off: its
    /// volume's whole release, from the top, and a tick either side. 0
    /// when the volume envelope is off, as the voice is cut at the note
    /// off.
    pub fn tail(&self) -> usize {
        let volume = &self.blocks[VOLUME];
        if !volume.enabled {
            return 0;
        }
        let ticks = (-1.0 / f64::from(volume.release)).ceil() + 3.0;
        (ticks * self.per_tick).ceil() as usize
    }
}

/// Left and right gain for a pan, as [`Curves::pan_law`] gives it.
type PanLaw = fn(f32) -> (f32, f32);

/// The envelopes of one voice. A clone starts where the original is.
#[derive(Debug, Clone)]
pub(crate) struct Articulation {
    shape: Arc<Shape>,
    /// The song's tick the voice started on, and the output sample of it.
    first_tick: f64,
    first_sample: f64,
    envelopes: [Envelope; 5],
    /// The ticks stepped so far, and the voice's sample the next is on.
    stepped: u64,
    next: usize,
    released: bool,
    /// For a voice whose pan the LFO moves: its level and its pan, as
    /// [`crate::voice::gains`] puts them together, and the pan law.
    panned: Option<(f32, f32, PanLaw)>,
    /// While the gains move: where they were at the voice's sample
    /// `tick_at`, where the tick started, and what a sample adds to them.
    from: [f32; 2],
    gain_step: [f32; 2],
    tick_at: usize,
    /// The pitch the envelope adds, in cents, and the mods.
    pub cents: f32,
    pub mods: (f32, f32),
}

/// What a tick of the envelopes did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ticked {
    /// The voice is over at its sample `.0`, the next tick.
    pub ends: Option<usize>,
}

impl Articulation {
    /// The envelopes of a voice that starts on song tick `tick`, with the
    /// level and pan [`crate::voice::gains`] made its gains from.
    pub fn new(shape: Shape, tick: u64, (level, pan): (f32, f32), curves: &dyn Curves) -> Self {
        let first_tick = tick as f64;
        let envelopes = std::array::from_fn(|e| Envelope::new(&shape.blocks[e]));
        Articulation {
            panned: shape.pans.then(|| (level, pan, curves.pan_law())),
            first_sample: (first_tick * shape.per_tick).round(),
            shape: Arc::new(shape),
            first_tick,
            envelopes,
            stepped: 0,
            next: 0,
            released: false,
            from: [0.0; 2],
            gain_step: [0.0; 2],
            tick_at: 0,
            cents: 0.0,
            mods: (0.0, 0.0),
        }
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    /// The voice's sample the next tick is on.
    pub fn next(&self) -> usize {
        self.next
    }

    /// The voice's sample tick `k` of it falls on.
    fn boundary(&self, k: u64) -> usize {
        let at = ((self.first_tick + k as f64) * self.shape.per_tick).round();
        (at - self.first_sample).max(0.0) as usize
    }

    /// Step the envelopes at the tick on the voice's sample `at`, letting
    /// them go first when the note is off by then (`held`, the note's
    /// length, `None` for one that plays its sample out). `gains` are the
    /// voice's from its channel and its note, which the volume and the
    /// pan go on.
    pub fn tick(&mut self, at: usize, held: Option<usize>, gains: (f32, f32)) -> Ticked {
        let shape = Arc::clone(&self.shape);
        let blocks = &shape.blocks;
        if !self.released && held.is_some_and(|held| held <= at) {
            self.released = true;
            // The pan's has no envelope that moves anything.
            let rest = self.envelopes.iter_mut().zip(blocks).skip(VOLUME);
            for (envelope, block) in rest {
                envelope.release(block);
            }
        }
        let first = self.stepped == 0;
        let starts_full = self.envelopes[VOLUME].stage > RISE;
        let tables = HostTables::get();
        // FL's shared LFO phase moves a step a tick from the song's start
        // (`FUN_009cef00`, `FUN_009ceec0`).
        let song_tick = (self.first_tick as u64).wrapping_add(self.stepped);
        for (envelope, block) in self.envelopes.iter_mut().zip(blocks) {
            let global = (song_tick as u32).wrapping_mul(block.lfo_step);
            envelope.step(block, global, tables);
        }
        self.stepped += 1;
        self.next = self.boundary(self.stepped);
        let [pan, volume, mod_x, mod_y, pitch] = &self.envelopes;
        let moved = |e: &Envelope, b: &Block| e.level * b.amount + e.lfo * b.lfo_amount;
        self.mods = (moved(mod_x, &blocks[2]), moved(mod_y, &blocks[3]));
        let p = &blocks[PITCH];
        self.cents = pitch.level * p.cents + pitch.lfo * p.lfo_cents;
        if shape.gains {
            let level = (volume.level * (volume.lfo * blocks[VOLUME].lfo_amount + 1.0)).max(0.0);
            let target = match self.panned {
                Some((own, centre, law)) => {
                    let swing = pan.lfo * blocks[PAN].lfo_amount * 2.0;
                    let (l, r) = law((centre + swing).clamp(-1.0, 1.0));
                    [l * own * level, r * own * level]
                }
                None => [gains.0 * level, gains.1 * level],
            };
            let from = match (first, starts_full) {
                (true, true) => target,
                (true, false) => [0.0; 2],
                (false, _) => self.gain_at(at),
            };
            // FL ramps over a tick's length, rounded, and ends where the
            // ramp is when the tick is over.
            let len = self.shape.per_tick.round().max(1.0) as usize;
            for side in 0..2 {
                self.gain_step[side] = ramp(target[side], from[side], shape.limit, len).0;
            }
            self.from = from;
        }
        self.tick_at = at;
        let over = blocks[VOLUME].enabled && self.released && volume.stage == OVER;
        Ticked {
            ends: over.then_some(self.next.max(at + 1)),
        }
    }

    /// The gains at the voice's sample `at`, inside the tick: a product
    /// and not a running sum, so they do not depend on where blocks fall,
    /// and FL works the tick's end out the same way (`0xd24d1f`).
    pub fn gain_at(&self, at: usize) -> [f32; 2] {
        let along = at.saturating_sub(self.tick_at) as f32;
        [0, 1].map(|side| self.from[side] + self.gain_step[side] * along)
    }

    /// Whether the gains move.
    pub fn gains(&self) -> bool {
        self.shape.gains
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FlExact;
    use flp::event::layout::EnvelopeLfo;

    /// A record as a new sampler has it, with `edit` done to it.
    fn record(edit: impl Fn(&mut EnvelopeLfo)) -> EnvelopeLfo {
        let mut r = EnvelopeLfo {
            flags: 0,
            unknown_1: [0; 3],
            enabled: 0,
            predelay: 100,
            attack: 20_000,
            hold: 20_000,
            decay: 30_000,
            sustain: 50,
            release: 20_000,
            amount: 0,
            lfo_predelay: 100,
            lfo_attack: 20_000,
            lfo_amount: 0,
            lfo_speed: 32_950,
            lfo_shape: 0,
            unknown_56: [0; 4],
            unknown_60: [0; 4],
            unknown_64: [0; 4],
            len: 68,
        };
        edit(&mut r);
        r
    }

    fn channel(volume: impl Fn(&mut EnvelopeLfo), pitch: impl Fn(&mut EnvelopeLfo)) -> Channel {
        let mut envelopes = vec![record(|_| {}); 5];
        envelopes[VOLUME] = record(|r| {
            r.flags = 4;
            volume(r);
        });
        envelopes[PITCH] = record(pitch);
        Channel {
            kind: Some(ChannelKind::Sampler),
            envelopes,
            ..Channel::default()
        }
    }

    /// 120 BPM at PPQ 96 and 9600 Hz: 50 samples a tick, and a time knob
    /// whose curve comes to 1 is 24 * 0.512 = 12.288 ticks.
    fn shape(channel: &Channel) -> Shape {
        Shape::of(channel, 120.0, 96, 9_600).unwrap()
    }

    #[test]
    fn records_that_move_nothing_make_no_shape() {
        let quiet = channel(|_| {}, |_| {});
        assert!(Shape::of(&quiet, 120.0, 96, 9_600).is_none());
        // A pitch envelope at amount 0 moves nothing; its LFO does.
        let pitch = channel(|_| {}, |r| r.enabled = 1);
        assert!(Shape::of(&pitch, 120.0, 96, 9_600).is_none());
        let lfo = channel(|_| {}, |r| r.lfo_amount = 100);
        assert!(shape(&lfo).pitches());
        // A volume envelope moves the volume whatever its amount.
        let volume = channel(|r| r.enabled = 1, |_| {});
        let s = shape(&volume);
        assert!(s.gains && !s.pitches && !s.mods);
    }

    /// Step a voice of `channel` over `ticks` ticks, its note off at
    /// sample `held`, and give the volume level each tick left it at.
    fn levels(channel: &Channel, held: Option<usize>, ticks: usize) -> Vec<f32> {
        let s = shape(channel);
        let mut art = Articulation::new(s, 0, (1.0, 0.0), &FlExact);
        (0..ticks)
            .map(|_| {
                let at = art.next();
                art.tick(at, held, (1.0, 1.0));
                art.envelopes[VOLUME].level
            })
            .collect()
    }

    #[test]
    fn an_attack_reaches_the_top_on_its_tick() {
        // attack 65536 * ln(1 + 3/12.288) / ln 1001: a curve of 3/12.288,
        // so 3 ticks, and a step of 1/4.
        let knob = (65_536.0 * (1.0 + 3.0 / 12.288f64).ln() / 1001f64.ln()).round() as i32;
        let c = channel(
            |r| {
                r.enabled = 1;
                r.attack = knob;
                r.hold = 100;
                r.decay = 100;
                r.sustain = 128;
            },
            |_| {},
        );
        let got = levels(&c, None, 6);
        assert_eq!(&got[..4], &[0.25, 0.5, 0.75, 1.0]);
        // Then it holds the sustain, which is the top.
        assert_eq!(&got[4..], &[1.0, 1.0]);
    }

    #[test]
    fn the_release_starts_at_the_note_off_and_ends_the_voice() {
        let c = channel(
            |r| {
                r.enabled = 1;
                r.attack = 100;
                r.hold = 100;
                r.decay = 100;
                r.sustain = 128;
                r.release = 100;
            },
            |_| {},
        );
        let s = shape(&c);
        let mut art = Articulation::new(s, 0, (1.0, 0.0), &FlExact);
        // Full from the start, since the attack takes no time.
        let first = art.tick(0, Some(150), (1.0, 1.0));
        assert_eq!(first.ends, None);
        assert_eq!(art.gain_at(0), [1.0, 1.0]);
        assert_eq!(art.next(), 50);
        assert_eq!(art.tick(50, Some(150), (1.0, 1.0)).ends, None);
        assert_eq!(art.tick(100, Some(150), (1.0, 1.0)).ends, None);
        // A release of no time drops to 0 at the tick of the note off,
        // and the voice ends with that tick.
        let off = art.tick(150, Some(150), (1.0, 1.0));
        assert_eq!(art.envelopes[VOLUME].level, 0.0);
        assert_eq!(off.ends, Some(200));
        // The gains head down by no more than 0.001 * 44100 / 9600 a
        // sample.
        assert!((art.gain_step[0] + 0.004_593_75).abs() < 1e-7);
    }

    #[test]
    fn a_volume_envelope_at_sustain_0_goes_quiet_and_waits_for_the_note_off() {
        let c = channel(
            |r| {
                r.enabled = 1;
                r.attack = 100;
                r.hold = 100;
                r.decay = 100;
                r.sustain = 0;
            },
            |_| {},
        );
        let s = shape(&c);
        let mut art = Articulation::new(s, 0, (1.0, 0.0), &FlExact);
        while art.next() < 1_000 {
            let at = art.next();
            assert_eq!(art.tick(at, Some(1_000), (1.0, 1.0)).ends, None, "{at}");
            assert_eq!(art.envelopes[VOLUME].level, 0.0);
        }
        assert_eq!(art.tick(1_000, Some(1_000), (1.0, 1.0)).ends, Some(1_050));
    }

    #[test]
    fn the_pitch_envelope_moves_by_its_amount_in_cents() {
        let c = channel(
            |_| {},
            |r| {
                r.enabled = 1;
                r.amount = -1_200;
                r.attack = 100;
                r.hold = 100;
                r.decay = 100;
                r.sustain = 64;
            },
        );
        let s = shape(&c);
        assert_eq!(s.lowest_cents(), -1_200.0);
        let mut art = Articulation::new(s, 0, (1.0, 0.0), &FlExact);
        art.tick(0, None, (1.0, 1.0));
        // At once to the top, and on the first tick down to the sustain,
        // half way, as the decay takes no time.
        assert_eq!(art.cents, -600.0);
        art.tick(50, None, (1.0, 1.0));
        assert_eq!(art.cents, -600.0);
    }

    #[test]
    fn an_old_3x_osc_takes_the_channel_s_envelopes_as_its_tab() {
        let mut c = channel(|r| r.enabled = 1, |r| r.amount = 68);
        c.kind = Some(ChannelKind::Plugin);
        c.plugin_name = Some("3x Osc".to_owned());
        let hosted = |c: &Channel| crate::generator::hosted(c, None, None).unwrap();
        // The pitch record's amount is word 8 of the fifth block.
        assert_eq!(tab_param(&c, 0x1408), Some(23 + 17 * 4 + 8));
        assert_eq!(tab_param(&c, 0x1511), None);
        let osc = hosted(&c);
        assert_eq!(osc.param(23 + 17 + 1), 1.0);
        assert_eq!(osc.param(23 + 17 * 4 + 8), 68.0);
        // A state of version 14 keeps its own tab.
        let mut state = vec![0u8; 460];
        state[0] = 14;
        c.plugin_state = Some(state);
        let osc = hosted(&c);
        assert_eq!(osc.param(23 + 17 * 4 + 8), 0.0);
        // A sampler has no tab.
        c.kind = Some(ChannelKind::Sampler);
        assert_eq!(tab_param(&c, 0x1408), None);
    }

    #[test]
    fn ticks_fall_on_the_song_s_ticks() {
        let c = channel(|r| r.enabled = 1, |_| {});
        // 44.1 kHz at 140 BPM and PPQ 96: 196.875 samples a tick.
        let s = Shape::of(&c, 140.0, 96, 44_100).unwrap();
        let mut art = Articulation::new(s, 3, (1.0, 0.0), &FlExact);
        let mut at = Vec::new();
        for _ in 0..4 {
            art.tick(art.next(), None, (1.0, 1.0));
            at.push(art.next());
        }
        // round(196.875 * (3 + k)) - round(196.875 * 3)
        assert_eq!(at, [197, 393, 590, 787]);
    }
}
