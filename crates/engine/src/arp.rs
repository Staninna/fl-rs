//! A channel's arpeggiator, as `FLEngine_x64.dll` plays it. The map, with
//! the addresses of the functions it comes from, is `docs/arpeggiator.md`.
//!
//! FL's host arpeggiates a channel's notes itself, so a sampler and a
//! plugin channel hear the same thing: notes. The engine works them out
//! from the song before anything plays, in ticks, and plays them as it
//! plays any other note.
//!
//! The notes held on a channel are kept in a list for each colour, from the
//! lowest key up. The first note held on an empty list is the master: it
//! makes no sound itself and starts a note of its own on every step of the
//! arpeggio, from a pattern built out of the list (`FUN_00feac50`): the
//! held notes over as many octaves as the range, each round played as many
//! times as the repeat, up, down, up and down, or up and down with both
//! ends twice. The notes that come while it is held join the list and
//! make no sound. The master goes on while any note of its list is held, its
//! own included or not, and stops as the last is let go of. A step is a
//! time on a log scale, in 48ths of a step of the song (a quarter of a
//! beat, `FUN_00fea220`), and a note lasts the gate's 48ths of it.
//!
//! When the list changes, the next step picks up in the new pattern where
//! the last left off: at the note nearest the one it played, or the one
//! nearest the note that was to come (`FUN_00d26b70`). Random picks every
//! step at random from the pattern, with FL's Mersenne twister.
//!
//! With slide on, the master sounds itself and moves its pitch to each
//! step's note, over the part of the step past the gate. With the time at
//! its top, 1447, it sounds itself once, at the first note of the
//! pattern.
//!
//! Each note of an arpeggio takes the key, the velocity, the pan and the
//! two mods of the held note it plays, and the master's release and colour.
//! With a chord of 0 every note runs an arpeggio of its own over its own
//! octaves.
//!
//! A channel whose arpeggiator asks for a chord out of FL's chord table,
//! which the engine does not have, or for no repeats, plays its notes as
//! they are, and [`Arpeggiated::unbuilt`] names it.

use crate::automation::Automation;
use crate::sequence::{Played, Song};
use model::{Channel, Note, Project};
use std::collections::BTreeMap;

/// A tick that never comes: FL's `0x7fffffff`.
const NEVER: i64 = i64::MAX;

/// Where `rec+0x200` and `rec+0x204`, the pitch a voice last played and the
/// one to come, start: `0x4f000000`, 2^31 as an f32.
const UNSET: f32 = 2_147_483_648.0;

/// The arpeggiator's time knob at its top: no steps, one note.
pub(crate) const TIME_HOLD: i32 = 1447;

/// A channel's arpeggiator as 215 has it, at `chan+0x2ac` and on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settings {
    /// 0 off, 1 up, 2 down, 3 up and down, 4 up and down with both ends
    /// twice, 5 random. Any other plays up.
    pub direction: i32,
    /// Octaves. Below 1 counts as 1, and 0 adds an octave to each note of
    /// a round over the one before, up to 5.
    pub range: i32,
    /// Below 0 the held notes, 0 the note alone, `n` entry `n - 1` of
    /// FL's chord table.
    pub chord: i32,
    /// The time knob, 450..=1447 ([`step_48ths`]).
    pub time: i32,
    /// The part of a step a note lasts, in 48ths.
    pub gate: i32,
    pub slide: bool,
    /// How many times each round of the notes plays.
    pub repeat: i32,
}

/// A new channel's, as `FUN_00fe90d0` sets them. 215 overwrites the part
/// it has bytes for.
impl Default for Settings {
    fn default() -> Self {
        Settings {
            direction: 0,
            range: 1,
            chord: -1,
            time: 1024,
            gate: 48,
            slide: false,
            repeat: 1,
        }
    }
}

impl Settings {
    /// The channel's, as FL's loader leaves them.
    pub fn of(project: &Project, channel: &Channel) -> Settings {
        let mut settings = Settings::default();
        if let Some(p) = channel.parameters {
            // The loader copies as many bytes as the event has over the
            // defaults.
            let has = |end: usize| p.len >= end;
            if has(44) {
                settings.direction = p.arp_direction;
            }
            if has(48) {
                settings.range = p.arp_range;
            }
            if has(52) {
                settings.chord = p.arp_chord;
            }
            if has(56) {
                settings.time = p.arp_time;
            }
            if has(60) {
                settings.gate = p.arp_gate;
            }
            if has(61) {
                settings.slide = p.arp_slide != 0;
            }
            if has(96) {
                settings.repeat = p.arp_repeat;
            }
        }
        let writer = (project.writer_version.as_deref()).and_then(flp::event::version_number);
        if writer.is_some_and(|writer| writer < 3_005_002) {
            settings.time = older_time(settings.time).clamp(450, TIME_HOLD);
        }
        settings
    }

    pub fn on(&self) -> bool {
        self.direction != 0
    }

    /// The part of FL's arpeggiator these ask for that the engine does
    /// not build.
    pub fn unbuilt(&self) -> Option<&'static str> {
        if self.chord > 0 {
            Some("a chord out of FL's chord table")
        } else if self.repeat < 1 {
            Some("no repeats")
        } else {
            None
        }
    }
}

/// A time knob in 48ths of a step: `FUN_0117bbc0`, with its f32 casts.
pub(crate) fn step_48ths(time: i32) -> i32 {
    let x = (f64::from(time) * (1.0 / 1024.0)) as f32;
    let grown = (f64::from(x) * 27f64.ln()).exp();
    let scaled = ((grown - 1.0) as f32) * 48.0;
    (f64::from(scaled) * 0.038_461_538_461_538_464).round_ties_even() as i32
}

/// A step in ticks of the song at this PPQ: `FUN_00fea220` caps the knob
/// at 1446 and `FUN_01312200` takes 48ths of `ppq / 4` ticks.
pub(crate) fn step_ticks(time: i32, ppq: u16) -> i64 {
    let per_step = i32::from(ppq) >> 2;
    i64::from(step_48ths(time.min(TIME_HOLD - 1)) * per_step / 48)
}

/// The ticks a note lasts at this step and gate: `FUN_00d26eb0`.
pub(crate) fn gate_ticks(step: i64, gate: i32) -> i64 {
    (step * i64::from(gate) / 48).max(1)
}

/// A time knob of a writer below 3.5.2 on today's scale: `FUN_0117bc50`.
fn older_time(time: i32) -> i32 {
    let x = (f64::from(time.wrapping_mul(26)) / 48.0) as f32 + 1.0;
    let part = (f64::from(x).ln() / 27f64.ln()) as f32 * 1024.0;
    f64::from(part).round_ties_even() as i32
}

/// FL's Mersenne twister, MT19937 (`FUN_00981250` seeds it,
/// `FUN_00981360` draws).
pub(crate) struct Twister {
    state: [u32; 624],
    index: usize,
}

impl Twister {
    /// The seed FL gives the global generator when a project is cleared
    /// (`FUN_011d3790`).
    pub const SEED: u32 = 0x012b_d6aa;

    pub fn new(seed: u32) -> Self {
        let mut state = [0; 624];
        state[0] = seed;
        for i in 1..624 {
            let prev = state[i - 1];
            state[i] = (prev ^ (prev >> 30))
                .wrapping_mul(0x6c07_8965)
                .wrapping_add(i as u32);
        }
        Twister { state, index: 624 }
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7fff_ffff);
                let mag = if y & 1 == 0 { 0 } else { 0x9908_b0df };
                self.state[i] = self.state[(i + 397) % 624] ^ (y >> 1) ^ mag;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
}

/// The notes of a song with its arpeggios played.
pub(crate) struct Arpeggiated {
    pub song: Song,
    /// The channels with an arpeggiator the engine does not build, by
    /// index, and what it lacks. Their notes play as they are.
    pub unbuilt: BTreeMap<u16, &'static str>,
}

/// The song with the notes of every channel whose arpeggiator is on played
/// through it. `None` when no channel with a note has one on.
pub(crate) fn arpeggiate(
    project: &Project,
    song: &Song,
    automation: &Automation,
) -> Option<Arpeggiated> {
    let mut channels = BTreeMap::new();
    let mut unbuilt = BTreeMap::new();
    for played in &song.notes {
        let index = played.note.channel;
        if channels.contains_key(&index) || unbuilt.contains_key(&index) {
            continue;
        }
        let Some(channel) = project.channel(index) else {
            continue;
        };
        let settings = Settings::of(project, channel);
        if !settings.on() {
            continue;
        }
        match settings.unbuilt() {
            Some(what) => {
                unbuilt.insert(index, what);
            }
            None => {
                channels.insert(index, settings);
            }
        }
    }
    if channels.is_empty() && unbuilt.is_empty() {
        return None;
    }
    let ppq = project.header.ppq;
    let mut run = Run::new(ppq, |channel, gate, tick| {
        automation.arp_at(channel, gate, tick as f64)
    });
    // A slide note starts no voice, and bends the arpeggio's notes as it
    // would any others.
    let mut notes = Vec::with_capacity(song.notes.len());
    for played in &song.notes {
        match channels.get(&played.note.channel) {
            Some(settings) if !played.note.slide() => run.add(*settings, played),
            _ => notes.push((*played, None)),
        }
    }
    let (made, glides) = run.finish();
    // The notes kept come first where two start together, as they came,
    // then the arpeggios' in the order FL starts them.
    notes.extend(made);
    notes.sort_by_key(|(played, _)| played.tick);
    let mut song = Song {
        notes: Vec::with_capacity(notes.len()),
        clips: song.clips.clone(),
        automation: song.automation.clone(),
        end: song.end,
        glides: BTreeMap::new(),
    };
    let mut glides: Vec<Option<Vec<(u64, f32)>>> = glides.into_iter().map(Some).collect();
    for (at, (played, glide)) in notes.into_iter().enumerate() {
        song.notes.push(played);
        if let Some(points) = glide.and_then(|g| glides[g].take()) {
            song.glides.insert(at, points);
        }
    }
    Some(Arpeggiated { song, unbuilt })
}

/// One held note of an arpeggio channel, FL's voice record from `rec+0x1c8`
/// on.
#[derive(Debug, Clone)]
struct Voice {
    played: Played,
    group: usize,
    /// `InitLevels.Pitch`, `key * 100`, which orders the list.
    init: f32,
    /// `rec+0x38`: the note's fine pitch in cents.
    fine: i32,
    /// The tick it is let go of on.
    end: i64,
    released: bool,
    /// `rec+0x1c8`: 1 a master, 2 a note of a master's list, -1 a note with
    /// an arpeggio of its own or one let go of.
    mode: i32,
    /// `rec+0x1cc`: 0 no steps, 1 a note on every step, 2 a glide, 3 one
    /// note.
    state: i32,
    /// `rec+0x1d0`, the place in the pattern.
    index: i32,
    /// `rec+0x1d4`, `+0x1d8`, `+0x1dc`: the ticks of the gate, the next
    /// step and the next glide.
    gate: i64,
    step: i64,
    glide_at: i64,
    /// `rec+0x1e0`: cents on the voice's pitch, its fine pitch until a
    /// step moves it.
    offset: f32,
    /// `rec+0x1e4` and `+0x1e8`: where a glide has got to and what it adds
    /// a tick.
    glide: f32,
    rate: f32,
    /// `rec+0x1ec..0x1fc` and `rec+0x1f4` on its own: what the step plays.
    /// `pitch` is FL's `rec+0x1f4`, in cents from the voice's own pitch
    /// for the held notes and from its key for a chord.
    entry: Entry,
    pitch: f32,
    /// `rec+0x200` and `+0x204`.
    last: f32,
    next: f32,
    /// `rec+0x210`: find the place in a changed pattern.
    reindex: bool,
    /// `rec+0x208`: the note of the step that sounds, in the output.
    child: Option<usize>,
    /// The voice's own note in the output, when it sounds.
    sounds: Option<usize>,
    /// Where a glide has moved the voice's own note: from each tick on,
    /// this many cents off the pitch it started at.
    moved: Vec<(u64, f32)>,
}

/// A note of a pattern: FL's 0x14-byte record of the five levels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    /// In cents, as FL adds it up in f32: from the voice's key for a
    /// chord, and from key 0 for the held notes.
    pitch: f32,
    /// The same in keys.
    key: i32,
    /// The voice whose velocity, pan and mods it takes.
    source: usize,
}

/// The notes held on one colour of one channel: the record at
/// `chan + 0x398 + 0x34 * colour`.
struct Group {
    channel: u16,
    color: u8,
    settings: Settings,
    /// `+0x14`: by `InitLevels.Pitch`, lowest first.
    held: Vec<usize>,
    /// `+0x1c`.
    master: Option<usize>,
    /// `+0x24`.
    pattern: Vec<Entry>,
    /// `+0x2c`: the chord the pattern was built for, 0 before the first.
    chord: i32,
    /// `+0x30`.
    dirty: bool,
}

/// The arpeggiators of a song as they run, tick by tick.
struct Run<F> {
    ppq: u16,
    /// The time (`false`) or gate (`true`) of a channel at a tick, where a
    /// clip drives it.
    control: F,
    random: Twister,
    groups: Vec<Group>,
    /// In the order FL makes them.
    voices: Vec<Voice>,
    /// The notes that come in, in the order they start.
    notes: Vec<(Settings, Played)>,
    out: Vec<(Played, Option<usize>)>,
    glides: Vec<Vec<(u64, f32)>>,
}

impl<F: Fn(u16, bool, u64) -> Option<i32>> Run<F> {
    fn new(ppq: u16, control: F) -> Self {
        Run {
            ppq,
            control,
            random: Twister::new(Twister::SEED),
            groups: Vec::new(),
            voices: Vec::new(),
            notes: Vec::new(),
            out: Vec::new(),
            glides: Vec::new(),
        }
    }

    fn add(&mut self, settings: Settings, played: &Played) {
        self.notes.push((settings, *played));
    }

    /// Run every tick that has something to do, and hand back the notes
    /// that sound with the index of their glide, and the glides.
    #[allow(clippy::type_complexity)]
    fn finish(mut self) -> (Vec<(Played, Option<usize>)>, Vec<Vec<(u64, f32)>>) {
        let notes = std::mem::take(&mut self.notes);
        let mut next = 0;
        let Some(first) = notes.first() else {
            return (self.out, self.glides);
        };
        let mut tick = first.1.tick as i64;
        loop {
            // Each tick FL lets go of the voices whose note has ended
            // (`FUN_00d0bac0`), starts the notes due (`FUN_00d293d0`), and
            // then runs the arpeggios (`FUN_00d0cbb0`, `FUN_00d2ab30`).
            for v in 0..self.voices.len() {
                if !self.voices[v].released && tick >= self.voices[v].end {
                    self.release(v, tick);
                }
            }
            while let Some((settings, played)) = notes.get(next).filter(|n| n.1.tick as i64 == tick)
            {
                self.start(*settings, played, tick);
                next += 1;
            }
            for v in 0..self.voices.len() {
                self.step(v, tick);
            }
            let due = (self.voices.iter()).filter_map(|voice| self.due(voice, tick));
            let coming = notes.get(next).map(|n| n.1.tick as i64);
            match due.chain(coming).min() {
                Some(at) => tick = at.max(tick + 1),
                None => break,
            }
        }
        for voice in &mut self.voices {
            if let Some(at) = voice.sounds.filter(|_| !voice.moved.is_empty()) {
                self.out[at].1 = Some(self.glides.len());
                self.glides.push(std::mem::take(&mut voice.moved));
            }
        }
        (self.out, self.glides)
    }

    /// The next tick this voice has something to do on, after `tick`.
    fn due(&self, voice: &Voice, tick: i64) -> Option<i64> {
        let end = (!voice.released).then_some(voice.end);
        let steps = match voice.state {
            1 | 3 if voice.gate > tick => Some(voice.gate),
            1 => Some(voice.step),
            // A glide moves on every tick past the gate.
            2 if voice.gate <= tick => Some(tick + 1),
            2 => Some(voice.gate),
            _ => None,
        };
        end.into_iter().chain(steps).filter(|&t| t != NEVER).min()
    }

    fn time(&self, group: usize, tick: i64) -> i32 {
        let group = &self.groups[group];
        (self.control)(group.channel, false, tick as u64).unwrap_or(group.settings.time)
    }

    fn gate(&self, group: usize, tick: i64) -> i32 {
        let group = &self.groups[group];
        (self.control)(group.channel, true, tick as u64).unwrap_or(group.settings.gate)
    }

    /// A note starts: `FUN_00d28910` and `FUN_00d28b00`.
    fn start(&mut self, settings: Settings, played: &Played, tick: i64) {
        let (channel, color) = (played.note.channel, played.note.color());
        let group =
            match (self.groups.iter()).position(|g| g.channel == channel && g.color == color) {
                Some(group) => group,
                None => {
                    self.groups.push(Group {
                        channel,
                        color,
                        settings,
                        held: Vec::new(),
                        master: None,
                        pattern: Vec::new(),
                        chord: 0,
                        dirty: true,
                    });
                    self.groups.len() - 1
                }
            };
        let fine = played.note.fine_pitch_cents();
        let init = f32::from(played.note.key) * 100.0;
        // A note of no length is let go of on the next tick.
        let length = played.length.unwrap_or(0).max(1);
        let v = self.voices.len();
        let mode = match settings.chord < 0 {
            true if self.groups[group].held.is_empty() => 1,
            true => 2,
            false => -1,
        };
        self.voices.push(Voice {
            played: *played,
            group,
            init,
            fine,
            end: tick + length as i64,
            released: false,
            mode,
            state: 0,
            index: -1,
            gate: NEVER,
            step: NEVER,
            glide_at: NEVER,
            offset: fine as f32,
            glide: fine as f32,
            rate: 0.0,
            entry: Entry {
                pitch: init,
                key: 0,
                source: v,
            },
            pitch: init,
            last: UNSET,
            next: UNSET,
            reindex: false,
            child: None,
            sounds: None,
            moved: Vec::new(),
        });
        if mode > 0 {
            let held = &self.groups[group].held;
            let at = held.partition_point(|&h| self.voices[h].init < init);
            self.groups[group].held.insert(at, v);
            self.touch(group);
        }
        if mode == 2 {
            return;
        }
        if mode > 0 {
            self.groups[group].master = Some(v);
        }
        let voice = &mut self.voices[v];
        (voice.step, voice.gate) = (tick, tick);
        let time = self.time(group, tick);
        let voice = &mut self.voices[v];
        voice.state = match (time < TIME_HOLD, settings.slide) {
            (true, false) => 1,
            (true, true) => {
                voice.glide_at = tick;
                2
            }
            (false, _) => 3,
        };
        if voice.state != 1 {
            voice.sounds = Some(self.out.len());
            let mut note = *played;
            note.length = None;
            self.out.push((note, None));
        }
    }

    /// The list changed: build the pattern again at the next step, and
    /// have the voices that have played a step find their place in it
    /// (`FUN_00feac10`, `FUN_00d2b5e0`).
    fn touch(&mut self, group: usize) {
        self.groups[group].dirty = true;
        for voice in self.voices.iter_mut().filter(|voice| voice.group == group) {
            voice.reindex = voice.last != UNSET;
        }
    }

    /// Let go of a voice: `FUN_00d241e0` through `FUN_00d23c00`.
    fn release(&mut self, v: usize, tick: i64) {
        if self.voices[v].released {
            return;
        }
        let group = self.voices[v].group;
        // A master goes on while its list holds a note.
        let keep = self.groups[group].master == Some(v) && !self.groups[group].held.is_empty();
        if keep {
            self.voices[v].end = NEVER;
        } else {
            self.voices[v].state = 0;
        }
        if self.voices[v].mode > 0 {
            self.voices[v].mode = -1;
            // `FUN_00d23b80`: off the list, in order, and the master with
            // the last.
            let held = &mut self.groups[group].held;
            if let Some(at) = held.iter().position(|&h| h == v) {
                held.remove(at);
            }
            if held.is_empty()
                && let Some(master) = self.groups[group].master
            {
                self.release(master, tick);
            }
            self.touch(group);
        }
        if self.voices[v].mode != 0 && self.groups[group].held.is_empty() {
            self.end_child(v, tick);
        }
        if !keep && !self.voices[v].released {
            self.voices[v].released = true;
            if let Some(at) = self.voices[v].sounds {
                self.end(at, tick);
            }
            if self.groups[group].master == Some(v) {
                self.groups[group].master = None;
            }
        }
    }

    /// Let go of the note in the output at `at`, unless it was already.
    fn end(&mut self, at: usize, tick: i64) {
        let played = &mut self.out[at].0;
        if played.length.is_none() {
            played.length = Some((tick as u64).saturating_sub(played.tick));
        }
    }

    fn end_child(&mut self, v: usize, tick: i64) {
        if let Some(child) = self.voices[v].child {
            self.end(child, tick);
        }
    }

    /// The arpeggio of a voice on one tick: `FUN_00d26f10`.
    fn step(&mut self, v: usize, tick: i64) {
        let voice = &self.voices[v];
        if voice.gate > tick {
            return;
        }
        let group = voice.group;
        match voice.state {
            1 => {
                // One held note whose pattern is that note alone sounds on
                // through the steps.
                if self.groups[group].chord == -1 && voice.child.is_some() {
                    if self.groups[group].dirty {
                        self.build(group);
                    }
                    let pattern = &self.groups[group].pattern;
                    if pattern.len() == 1 && self.voices[v].last == pattern[0].pitch {
                        if tick >= self.voices[v].step {
                            self.schedule(v, tick);
                        }
                        return;
                    }
                }
                if let Some(child) = self.voices[v].child.take() {
                    self.end(child, tick);
                }
                if self.voices[v].step <= tick {
                    let start = self.voices[v].step;
                    self.advance(v);
                    let note = self.child_note(v);
                    self.voices[v].child = Some(self.out.len());
                    let played = Played {
                        tick: start as u64,
                        length: None,
                        note,
                    };
                    self.out.push((played, None));
                    self.schedule(v, tick);
                }
            }
            2 => {
                if self.voices[v].glide_at <= tick {
                    // `FUN_00d26e40`.
                    let from = self.voices[v].offset;
                    self.voices[v].glide = from;
                    self.advance(v);
                    let voice = &mut self.voices[v];
                    let ticks = (voice.step - voice.gate + 1) as f32;
                    voice.rate = (voice.pitch - from) / ticks;
                    voice.glide_at = NEVER;
                }
                if self.voices[v].step <= tick {
                    self.schedule(v, tick);
                    self.voices[v].glide_at = self.voices[v].gate;
                }
                let voice = &mut self.voices[v];
                voice.glide += voice.rate;
                let offset = f64::from(voice.glide).round_ties_even() as f32;
                self.moved(v, tick, offset);
            }
            3 => {
                self.advance(v);
                let pitch = self.voices[v].pitch;
                self.moved(v, tick, pitch);
                self.voices[v].gate = NEVER;
            }
            _ => {}
        }
    }

    /// A voice that sounds itself has its pitch moved to `offset` cents on
    /// its own pitch. On the tick it starts that is the note's pitch.
    fn moved(&mut self, v: usize, tick: i64, offset: f32) {
        let voice = &mut self.voices[v];
        if offset == voice.offset && tick != voice.played.tick as i64 {
            return;
        }
        voice.offset = offset;
        let Some(at) = voice.sounds else {
            return;
        };
        if tick == voice.played.tick as i64 {
            // Whole keys go to the key, the rest to the fine pitch, as the
            // note's own fine pitch was.
            let cents = offset.round() as i32;
            let keys = cents.div_euclid(100);
            let note = &mut self.out[at].0.note;
            note.key = voice.played.note.key.saturating_add(keys as i16);
            note.fine_pitch = (120 + cents.rem_euclid(100) / 10) as u8;
        } else {
            let note = &self.out[at].0.note;
            let own = f32::from(note.key - voice.played.note.key) * 100.0
                + note.fine_pitch_cents() as f32;
            voice.moved.push((tick as u64, offset - own));
        }
    }

    /// The next step: `FUN_00d26eb0`.
    fn schedule(&mut self, v: usize, tick: i64) {
        let group = self.voices[v].group;
        let step = step_ticks(self.time(group, tick), self.ppq);
        let gate = gate_ticks(step, self.gate(group, tick));
        let voice = &mut self.voices[v];
        voice.gate = tick + gate;
        voice.step = tick + step;
    }

    /// Move a voice to its next note in the pattern: `FUN_00d26b70`.
    fn advance(&mut self, v: usize) {
        let group = self.voices[v].group;
        // `FUN_00d26320`: the voice's own levels, which a chord keeps.
        let own = Entry {
            pitch: self.voices[v].init,
            key: 0,
            source: v,
        };
        self.voices[v].entry = own;
        self.voices[v].pitch = own.pitch;
        if self.groups[group].dirty {
            self.build(group);
        }
        let g = &self.groups[group];
        let up = i32::from(g.settings.direction > 0);
        if std::mem::take(&mut self.voices[v].reindex) {
            let voice = &self.voices[v];
            let mut best = i32::MAX;
            let mut index = voice.index;
            for (k, entry) in g.pattern.iter().enumerate() {
                let off = |to: f32| (f64::from(entry.pitch - to).round_ties_even() as i64).abs();
                let last = off(voice.last) as i32;
                if last <= best {
                    index = k as i32;
                    best = last;
                }
                let next = off(voice.next) as i32;
                if next < best {
                    index = k as i32 - up;
                    best = next;
                }
            }
            self.voices[v].index = index;
        }
        let n = g.pattern.len() as i32;
        if n == 0 {
            return;
        }
        let index = if g.settings.direction == 5 {
            let draw = f64::from(self.random.next_u32()) * 2f64.powi(-32);
            (draw * f64::from(n)) as i32
        } else {
            (self.voices[v].index + n + up).rem_euclid(n)
        };
        let entry = g.pattern[index as usize];
        let next = g.pattern[((index + 1) % n) as usize].pitch;
        let chord = g.chord;
        let voice = &mut self.voices[v];
        voice.index = index;
        voice.last = entry.pitch;
        voice.next = next;
        if chord < 0 {
            voice.entry = entry;
            voice.pitch = entry.pitch - voice.init;
        } else {
            voice.entry = Entry {
                pitch: entry.pitch,
                key: entry.key,
                source: v,
            };
            voice.pitch = entry.pitch + voice.fine as f32;
        }
    }

    /// The note a master starts on a step, from what [`Run::advance`] left.
    fn child_note(&self, v: usize) -> Note {
        let voice = &self.voices[v];
        let own = voice.played.note;
        let source = self.voices[voice.entry.source].played.note;
        let chord = self.groups[voice.group].chord;
        let (key, fine_pitch) = if chord < 0 {
            // The held note's key, an octave on: its fine pitch is not in
            // `InitLevels.Pitch` and the step has none.
            (voice.entry.key, 120)
        } else {
            (i32::from(own.key) + voice.entry.key, own.fine_pitch)
        };
        Note {
            key: key.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            fine_pitch,
            pan: source.pan,
            velocity: source.velocity,
            mod_x: source.mod_x,
            mod_y: source.mod_y,
            ..own
        }
    }

    /// Build a group's pattern: `FUN_00feac50`.
    fn build(&mut self, group: usize) {
        let g = &self.groups[group];
        let settings = g.settings;
        // A chord out of FL's table is not built, and with nothing held
        // the held notes have none to give: a note alone stands in.
        let (chord, notes) = match settings.chord {
            c if c < 0 && !g.held.is_empty() => (c, g.held.len() as i32),
            c if c < 0 => (1, 1),
            c => (c, 1),
        };
        let per = notes * settings.repeat;
        let up = settings.range.max(1) * per;
        let len = match settings.direction {
            3 if up > 1 => 2 * up - 2,
            4 if up > 1 => 2 * up,
            _ => up,
        };
        let mut pattern = Vec::with_capacity(len.max(0) as usize);
        for i in 0..len {
            let j = match settings.direction {
                _ if up <= 1 => i,
                2 => up - i - 1,
                3 if i >= up => 2 * up - i - 2,
                4 if i >= up => 2 * up - i - 1,
                _ => i,
            };
            let mut octave = j / per;
            let note = (j % per) % notes;
            if settings.range == 0 {
                octave += note.min(5);
            }
            let (pitch, key, source) = match chord < 0 {
                true => {
                    let held = g.held[note as usize];
                    let voice = &self.voices[held];
                    (voice.init, i32::from(voice.played.note.key), held)
                }
                false => (0.0, 0, usize::MAX),
            };
            pattern.push(Entry {
                pitch: pitch + (octave * 1200) as f32,
                key: key + octave * 12,
                source,
            });
        }
        let g = &mut self.groups[group];
        g.pattern = pattern;
        g.chord = chord;
        g.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_knob_is_48ths_of_a_step_on_a_log_scale() {
        // 1024 is one step, 450 an eighth of one and 1446 four.
        let steps: Vec<_> = [450, 626, 766, 916, 1024, 1234, 1303, 1446]
            .map(step_48ths)
            .into();
        assert_eq!(steps, [6, 12, 20, 33, 48, 96, 121, 192]);
        // At PPQ 96 a step is 24 ticks, and the 48ths are cut down.
        assert_eq!(step_ticks(1024, 96), 24);
        assert_eq!(step_ticks(626, 96), 6);
        assert_eq!(step_ticks(916, 96), 16);
        assert_eq!(step_ticks(1234, 96), 48);
        // The top of the knob counts as the one below it.
        assert_eq!(step_ticks(1447, 96), step_ticks(1446, 96));
        assert_eq!(step_ticks(1024, 960), 240);
    }

    #[test]
    fn the_gate_is_48ths_of_the_step_and_one_tick_at_least() {
        assert_eq!(gate_ticks(24, 48), 24);
        assert_eq!(gate_ticks(24, 26), 13);
        assert_eq!(gate_ticks(6, 16), 2);
        assert_eq!(gate_ticks(6, 0), 1);
    }

    #[test]
    fn an_older_time_comes_onto_todays_scale() {
        // FUN_0117bc50 is the inverse of FUN_0117bbc0 for 48ths of a step.
        assert_eq!(older_time(48), 1024);
        assert_eq!(older_time(6), 450);
    }

    #[test]
    fn the_twister_is_mt19937() {
        // The reference output of MT19937 for its default seed.
        let mut twister = Twister::new(5489);
        let first: Vec<u32> = (0..3).map(|_| twister.next_u32()).collect();
        assert_eq!(first, [3_499_211_612, 581_869_302, 3_890_346_734]);
        let mut twister = Twister::new(5489);
        let tenth_thousand = (0..10_000).map(|_| twister.next_u32()).last();
        assert_eq!(tenth_thousand, Some(4_123_659_995));
    }

    fn note(tick: u64, length: u64, key: i16, velocity: u8) -> Played {
        Played {
            tick,
            length: Some(length),
            note: Note {
                key,
                velocity,
                ..Note::default()
            },
        }
    }

    fn settings(direction: i32, range: i32) -> Settings {
        Settings {
            direction,
            range,
            ..Settings::default()
        }
    }

    /// What one channel's notes make at PPQ 96, as tick, length and key.
    fn run(settings: Settings, notes: &[Played]) -> Vec<(u64, u64, i16)> {
        let mut run = Run::new(96, |_, _, _| None);
        for played in notes {
            run.add(settings, played);
        }
        let (out, _) = run.finish();
        let mut out: Vec<_> = (out.iter())
            .map(|(p, _)| (p.tick, p.length.unwrap(), p.note.key))
            .collect();
        out.sort();
        out
    }

    fn keys(out: &[(u64, u64, i16)]) -> Vec<i16> {
        out.iter().map(|n| n.2).collect()
    }

    /// C, E and G for two bars.
    fn chord() -> Vec<Played> {
        [60, 64, 67].map(|key| note(0, 384, key, 100)).into()
    }

    #[test]
    fn up_plays_the_held_notes_from_the_lowest_over_the_range() {
        let out = run(settings(1, 2), &chord());
        // A step a 16th, 24 ticks, for 384 ticks.
        assert_eq!(out.len(), 16);
        assert_eq!(keys(&out[..7]), [60, 64, 67, 72, 76, 79, 60]);
        assert!(out.iter().enumerate().all(|(i, n)| n.0 == 24 * i as u64));
        // The gate at 48 lasts the step.
        assert!(out.iter().all(|n| n.1 == 24));
    }

    #[test]
    fn down_starts_at_the_top_of_the_range() {
        let out = run(settings(2, 2), &chord());
        assert_eq!(keys(&out[..7]), [79, 76, 72, 67, 64, 60, 79]);
    }

    #[test]
    fn up_and_down_turns_on_the_ends_and_sticky_plays_them_twice() {
        let out = run(settings(3, 1), &chord());
        assert_eq!(keys(&out[..6]), [60, 64, 67, 64, 60, 64]);
        let out = run(settings(4, 1), &chord());
        assert_eq!(keys(&out[..8]), [60, 64, 67, 67, 64, 60, 60, 64]);
    }

    #[test]
    fn repeat_plays_each_round_again_before_the_next_octave() {
        let settings = Settings {
            repeat: 2,
            ..settings(1, 2)
        };
        let out = run(settings, &chord());
        assert_eq!(keys(&out[..7]), [60, 64, 67, 60, 64, 67, 72]);
    }

    #[test]
    fn range_0_puts_each_note_an_octave_over_the_one_before() {
        let out = run(settings(1, 0), &chord());
        assert_eq!(keys(&out[..4]), [60, 76, 91, 60]);
    }

    #[test]
    fn the_steps_follow_the_time_and_the_gate() {
        // 1234 is two steps, 48 ticks at PPQ 96, and a gate of 26 lasts
        // 26 of them.
        let settings = Settings {
            time: 1234,
            gate: 26,
            ..settings(1, 1)
        };
        let out = run(settings, &chord());
        assert_eq!(out[..3], [(0, 26, 60), (48, 26, 64), (96, 26, 67)]);
        assert_eq!(out.len(), 8);
        // 626 is a quarter of a step, 6 ticks, and its gate of 26 3.
        let settings = Settings {
            time: 626,
            ..settings
        };
        let out = run(settings, &chord());
        assert_eq!(out[..2], [(0, 3, 60), (6, 3, 64)]);
        assert_eq!(out.len(), 64);
    }

    #[test]
    fn each_step_takes_the_velocity_of_the_note_it_plays() {
        let mut run = Run::new(96, |_, _, _| None);
        for played in [note(0, 48, 64, 90), note(0, 48, 60, 30)] {
            run.add(settings(1, 1), &played);
        }
        let (out, _) = run.finish();
        let velocities: Vec<_> = out.iter().map(|(p, _)| p.note.velocity).collect();
        assert_eq!(velocities, [30, 90]);
    }

    #[test]
    fn a_note_left_alone_in_the_list_sounds_on_until_it_is_let_go_of() {
        // C is let go of on tick 30. On the step at 48 the pattern is E
        // alone, which sounds already, so it goes on to E's end.
        let out = run(
            settings(1, 1),
            &[note(0, 30, 60, 100), note(0, 50, 64, 100)],
        );
        assert_eq!(out, [(0, 24, 60), (24, 26, 64)]);
    }

    #[test]
    fn a_note_that_joins_takes_the_arpeggio_on_from_where_it_was() {
        // C and G, then E comes on the third step: the next note up from G
        // in C E G is the C an octave on, so the arpeggio goes C G C' E'.
        let notes = [
            note(0, 192, 60, 100),
            note(0, 192, 67, 100),
            note(48, 144, 64, 100),
        ];
        let out = run(settings(1, 2), &notes);
        assert_eq!(keys(&out[..5]), [60, 67, 72, 76, 79]);
    }

    #[test]
    fn one_held_note_sounds_on_through_its_steps() {
        let out = run(settings(1, 1), &[note(0, 96, 60, 100)]);
        assert_eq!(out, [(0, 96, 60)]);
        // Not so with the held notes asked for as -2.
        let settings = Settings {
            chord: -2,
            ..settings(1, 1)
        };
        let out = run(settings, &[note(0, 96, 60, 100)]);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn chord_0_runs_an_arpeggio_on_every_note_over_its_own_octaves() {
        let settings = Settings {
            chord: 0,
            ..settings(1, 2)
        };
        let out = run(settings, &[note(0, 48, 60, 100), note(0, 48, 64, 100)]);
        assert_eq!(out, [(0, 24, 60), (0, 24, 64), (24, 24, 72), (24, 24, 76)]);
    }

    #[test]
    fn random_draws_from_the_twister_for_every_step() {
        let out = run(settings(5, 2), &chord());
        let mut twister = Twister::new(Twister::SEED);
        let pattern = [60, 64, 67, 72, 76, 79];
        let want: Vec<i16> = (0..16)
            .map(|_| {
                let draw = f64::from(twister.next_u32()) * 2f64.powi(-32);
                pattern[(draw * 6.0) as usize]
            })
            .collect();
        assert_eq!(keys(&out), want);
    }

    #[test]
    fn slide_moves_the_first_note_from_step_to_step() {
        // Two steps, 48 ticks, with a gate of 24: the glide takes the 25
        // ticks from the gate to the next step, the step's own included.
        let settings = Settings {
            slide: true,
            time: 1234,
            gate: 24,
            ..settings(1, 1)
        };
        let mut run = Run::new(96, |_, _, _| None);
        for played in [note(0, 96, 60, 100), note(0, 96, 64, 100)] {
            run.add(settings, &played);
        }
        let (out, glides) = run.finish();
        assert_eq!(out.len(), 1);
        let (played, glide) = out[0];
        assert_eq!(
            (played.tick, played.length, played.note.key),
            (0, Some(96), 60)
        );
        let glide = &glides[glide.unwrap()];
        // Up 400 cents, 16 a tick from tick 24 to 48.
        let up: Vec<(u64, f32)> = (24..=48).map(|t| (t, 16.0 * (t - 23) as f32)).collect();
        assert_eq!(glide[..25], up[..]);
        // and back down from the gate of the second step on, until the
        // notes are let go of.
        let down: Vec<(u64, f32)> = (72..96)
            .map(|t| (t, 400.0 - 16.0 * (t - 71) as f32))
            .collect();
        assert_eq!(glide[25..], down[..]);
    }

    #[test]
    fn at_the_top_of_its_time_it_plays_the_first_note_once() {
        let settings = Settings {
            time: TIME_HOLD,
            ..settings(2, 1)
        };
        let out = run(settings, &chord());
        assert_eq!(out, [(0, 384, 67)]);
    }
}
