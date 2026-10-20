//! The mixer: insert buses, the generators and the voices that play into
//! them, the plugins in their effect slots, their volume and pan, the sends
//! between them, the sidechain inputs those sends feed, and the delay lines
//! that make up for the plugins that are late.

use crate::automation::Controller;
use crate::eq::InsertEq;
use crate::generator::{Cue, Hosted as Generator};
use crate::voice::Voice;
use crate::{Curves, Error, pool};
use model::{InsertMix, Project, Slot};
use plugins::{Effect, Fidelity};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

/// The samples a bus holds, and the most [`Mixer::process`] takes at once.
pub(crate) const BLOCK: usize = 512;

/// A part of the mix that the `profile` feature times on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The generator of a plugin channel, by the index its 64 has.
    Generator(u16),
    /// The plugin in a slot of an insert, by the loader's insert number.
    Slot { insert: u16, slot: u8 },
}

/// How long each part took since the last [`Spent::clear`], a part once for
/// every block it played, with the `profile` feature. Without it this
/// holds nothing and times nothing.
#[derive(Default)]
pub(crate) struct Spent {
    #[cfg(feature = "profile")]
    pub parts: Vec<(Part, std::time::Duration)>,
}

impl Spent {
    fn time<T>(&mut self, part: impl FnOnce() -> Part, run: impl FnOnce() -> T) -> T {
        #[cfg(feature = "profile")]
        {
            let started = std::time::Instant::now();
            let out = run();
            self.parts.push((part(), started.elapsed()));
            out
        }
        #[cfg(not(feature = "profile"))]
        {
            let _ = part;
            run()
        }
    }

    pub(crate) fn clear(&mut self) {
        #[cfg(feature = "profile")]
        self.parts.clear();
    }
}

/// The frames a tap takes to move along its delay line, and a slot with a
/// late plugin to switch.
const FADE: usize = 256;

/// Where a delay line is read.
#[derive(Clone, Copy, Default)]
struct Tap {
    /// How many frames back.
    delay: usize,
    /// How many frames back it was read before. That fades out over the
    /// frames `fade` has left.
    from: usize,
    fade: usize,
}

impl Tap {
    /// Read `delay` frames back from now on. With `fade` the tap gets
    /// there over [`FADE`] frames, and without it at once.
    fn aim(&mut self, delay: usize, fade: bool) {
        if delay == self.delay {
            return;
        }
        // A tap sent back to where it is fading from turns round on the
        // spot.
        let turned = self.fade > 0 && delay == self.from;
        self.fade = match (fade, turned) {
            (false, _) => 0,
            (true, true) => FADE - self.fade,
            (true, false) => FADE,
        };
        self.from = self.delay;
        self.delay = delay;
    }

    /// Whether the tap reads the frames that were written last and
    /// nothing else.
    fn direct(&self) -> bool {
        self.delay == 0 && self.fade == 0
    }
}

/// The last frames of a bus, for a tap to read late. A line without frames
/// belongs to a bus that nothing reads late.
#[derive(Default)]
struct Line {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Where the next frame goes.
    at: usize,
}

impl Line {
    /// A line that a tap reads up to `reach` frames back.
    fn new(reach: usize) -> Self {
        let frames = if reach == 0 { 0 } else { reach + BLOCK };
        Line {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            at: 0,
        }
    }

    /// How far back a tap can read once a block is written.
    fn reach(&self) -> usize {
        self.left.len().saturating_sub(BLOCK)
    }

    /// Take a block of no more than [`BLOCK`] frames in.
    fn write(&mut self, left: &[f32], right: &[f32]) {
        let size = self.left.len();
        if size == 0 {
            return;
        }
        let head = left.len().min(size - self.at);
        self.left[self.at..self.at + head].copy_from_slice(&left[..head]);
        self.right[self.at..self.at + head].copy_from_slice(&right[..head]);
        self.left[..left.len() - head].copy_from_slice(&left[head..]);
        self.right[..right.len() - head].copy_from_slice(&right[head..]);
        self.at = (self.at + left.len()) % size;
    }

    /// Add the block that was written last to `left` and `right` as the
    /// tap reads it, at `gain`, and move the tap's fade on by as many
    /// frames.
    fn add(&self, tap: &mut Tap, gain: f32, left: &mut [f32], right: &mut [f32]) {
        let size = self.left.len();
        // What the tap reads for the block's first frame.
        let back = |delay: usize| (self.at + size - left.len() - delay) % size;
        let (mut now, mut before) = (back(tap.delay), back(tap.from));
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (mut frame_left, mut frame_right) = (self.left[now], self.right[now]);
            if tap.fade > 0 {
                tap.fade -= 1;
                let t = 1.0 - tap.fade as f32 / FADE as f32;
                let (old_left, old_right) = (self.left[before], self.right[before]);
                frame_left = old_left + (frame_left - old_left) * t;
                frame_right = old_right + (frame_right - old_right) * t;
            }
            *l += frame_left * gain;
            *r += frame_right * gain;
            now = (now + 1) % size;
            before = (before + 1) % size;
        }
    }
}

/// What [`Mixer::approximated_slots`] says of a plugin whose state names a
/// sidechain input its insert does not have.
const NO_INPUT: &str = "the sidechain input has no insert behind it, so the plugin hears its own";

/// One sidechain input of an insert: what another insert puts out, for the
/// plugins here that listen to it. None of it is added to the bus.
struct Side {
    /// FL's number for the input: the place, from 1, of the insert it
    /// comes from among the inserts that send here.
    number: usize,
    /// The input for the block being mixed, [`BLOCK`] long. Silence when
    /// the insert it comes from has nothing to send.
    left: Vec<f32>,
    right: Vec<f32>,
}

/// Where the second output of a plugin goes: into the bus of an insert its
/// own insert sends to, as it leaves the plugin.
struct Aux {
    /// That insert's place in [`Mixer::strips`].
    dest: usize,
    /// The output for the block being mixed, [`BLOCK`] long.
    left: Vec<f32>,
    right: Vec<f32>,
    /// Its last frames, and where they are read to come in with that
    /// insert's sends.
    line: Line,
    tap: Tap,
}

/// A plugin the engine hosts, in the effect slot it sits in.
struct Hosted {
    /// The slot's number in its insert.
    slot: u8,
    effect: Box<dyn Effect>,
    /// A slot that is switched off passes its input on as it is.
    enabled: bool,
    /// How much of the slot's output is the plugin's. The rest is the
    /// slot's input.
    mix: f32,
    /// How many frames late the plugin is.
    latency: usize,
    /// The frames left of the fade a late plugin's slot is switched by:
    /// from its input to its output or back.
    fade: usize,
    /// The places in [`Strip::side`] of the sidechain inputs the plugin
    /// listens to, when its state names ones the insert has: the one of
    /// [`Effect::sidechain_inputs`] each, as Vocodex has two.
    side: [Option<usize>; 2],
    /// Where the plugin's second output goes, when it has one.
    aux: Option<Aux>,
    /// Whether the plugin hears of every tick of the song: a controller,
    /// which has outputs beside its sound, or a plugin that follows the
    /// song's place.
    ticks: bool,
}

impl Hosted {
    /// A plugin in a slot that is switched on, all wet, with its sidechain
    /// input and its second output not wired up yet.
    fn new(slot: u8, effect: Box<dyn Effect>) -> Self {
        Hosted {
            slot,
            latency: effect.latency(),
            ticks: !effect.controller_outputs().is_empty() || effect.follows_song(),
            effect,
            enabled: true,
            mix: 1.0,
            fade: 0,
            side: [None; 2],
            aux: None,
        }
    }

    /// Run a block of the insert's bus through the slot, in place, and say
    /// how many frames of it the plugin played. `sides` are the insert's
    /// sidechain inputs for the block, and `dry` is as long as the block
    /// and holds nothing that has to last.
    fn play(
        &mut self,
        sides: &[Side],
        left: &mut [f32],
        right: &mut [f32],
        dry: (&mut [f32], &mut [f32]),
    ) -> usize {
        // A slot on its way out plays the frames of its fade and no more.
        let run = if self.enabled {
            left.len()
        } else {
            self.fade.min(left.len())
        };
        if run == 0 {
            return 0;
        }
        for side in self
            .side
            .iter()
            .flatten()
            .filter_map(|side| sides.get(*side))
        {
            let (side_left, side_right) = (&side.left[..run], &side.right[..run]);
            self.effect
                .set_sidechain(side.number, side_left, side_right);
        }
        let (wet_left, wet_right) = (&mut left[..run], &mut right[..run]);
        if self.mix == 1.0 && self.fade == 0 {
            self.effect.process(wet_left, wet_right);
            return run;
        }
        let (dry_left, dry_right) = (&mut dry.0[..run], &mut dry.1[..run]);
        dry_left.copy_from_slice(wet_left);
        dry_right.copy_from_slice(wet_right);
        self.effect.process(wet_left, wet_right);
        if self.fade == 0 {
            let dry = 1.0 - self.mix;
            for (wet, v) in wet_left.iter_mut().zip(&*dry_left) {
                *wet = *wet * self.mix + v * dry;
            }
            for (wet, v) in wet_right.iter_mut().zip(&*dry_right) {
                *wet = *wet * self.mix + v * dry;
            }
            return run;
        }
        for frame in 0..run {
            let share = self.share();
            let dry = 1.0 - share;
            wet_left[frame] = wet_left[frame] * share + dry_left[frame] * dry;
            wet_right[frame] = wet_right[frame] * share + dry_right[frame] * dry;
        }
        run
    }

    /// How much of the slot's next frame is the plugin's, which moves a
    /// fade on by one frame.
    fn share(&mut self) -> f32 {
        if self.fade == 0 {
            return if self.enabled { self.mix } else { 0.0 };
        }
        self.fade -= 1;
        let t = 1.0 - self.fade as f32 / FADE as f32;
        self.mix * if self.enabled { t } else { 1.0 - t }
    }
}

/// A send from one strip to a later one.
struct Send {
    /// The destination's place in [`Mixer::strips`].
    dest: usize,
    gain: f32,
    /// Where the send reads the sender's [`Strip::sent`].
    tap: Tap,
    /// The place in the destination's [`Strip::side`] of the sidechain
    /// input the sender is, when a plugin there listens to it. That gets
    /// what the sender puts out, whatever the send's level.
    side: Option<usize>,
}

/// One insert that sound can reach.
struct Strip {
    /// The loader's insert number.
    index: u16,
    /// An insert that is switched off passes nothing on.
    enabled: bool,
    /// The hosted plugins of the insert's slots, first slot first.
    effects: Vec<Hosted>,
    /// Gain of the insert's volume.
    volume: f32,
    /// Left and right gain of its pan and stereo separation.
    pan: (f32, f32),
    /// What its pan and separation feed across: the gain of the right
    /// side of its bus in its left output, and of the left side in its
    /// right output.
    cross: (f32, f32),
    /// The raw pan and separation `pan` and `cross` come from, so that
    /// either can be set without the other.
    pan_raw: i32,
    separation_raw: i32,
    /// Bit 0 of the insert's flags: its gain is negative.
    polarity: bool,
    /// Bit 1 of its flags: its outputs change sides.
    swap: bool,
    /// The insert's three-band EQ, after its slots and before its volume.
    eq: InsertEq,
    /// Where the insert sends to. Every destination comes after its
    /// sender.
    sends: Vec<Send>,
    /// The insert's bus for the block being mixed, [`BLOCK`] long.
    left: Vec<f32>,
    right: Vec<f32>,
    /// Whether a voice plays into the insert.
    voiced: bool,
    /// How many frames after it was played sound gets to the bus: as many
    /// as the latest of the inserts that send here is late.
    arrival: usize,
    /// What the voices played into the insert, and where it is read to
    /// come in with the sends.
    played: Line,
    played_tap: Tap,
    /// What the insert put out, for the sends that are read late.
    sent: Line,
    /// The sidechain inputs the insert's plugins listen to.
    side: Vec<Side>,
    /// The loudest sample the insert has put out on the left and on the
    /// right since [`Mixer::take_peaks`] last asked.
    peak: (f32, f32),
    /// The strip's place in the order the routes were walked in, which is
    /// the order its senders' sends add up in.
    rank: usize,
    /// The places in [`Mixer::strips`] of the strips that send here, by
    /// their [`Strip::rank`].
    incoming: Vec<usize>,
    /// The generators that play into the insert: where they start in
    /// [`Mixer::generators`] and how many there are.
    generators: (usize, usize),
    /// What the voices and generators of the insert played, when that is
    /// read late, and a slot's input for a slot whose mix level is not all
    /// the way up. [`BLOCK`] long.
    dry_left: Vec<f32>,
    dry_right: Vec<f32>,
    /// What one generator puts out, before its channel's volume and pan.
    made_left: Vec<f32>,
    made_right: Vec<f32>,
    /// How long the insert's generators and slots took.
    spent: Spent,
}

impl Strip {
    /// How many frames late the plugins in the slots that are switched on
    /// make the bus.
    fn latency(&self) -> usize {
        let on = self.effects.iter().filter(|hosted| hosted.enabled);
        on.map(|hosted| hosted.latency).sum()
    }

    /// The most that can come to, with every slot switched on.
    fn max_latency(&self) -> usize {
        let hosted = self.effects.iter();
        hosted.map(|hosted| hosted.effect.max_latency()).sum()
    }
}

/// Work out [`Strip::arrival`] for every strip, with each strip's plugins
/// as late as `chain` says. Senders come first in `strips`.
fn arrivals(strips: &mut [Strip], chain: impl Fn(&Strip) -> usize) {
    for strip in strips.iter_mut() {
        strip.arrival = 0;
    }
    for at in 0..strips.len() {
        let (senders, later) = strips.split_at_mut(at + 1);
        let strip = &senders[at];
        let out = strip.arrival + chain(strip);
        for send in &strip.sends {
            let to = &mut later[send.dest - at - 1];
            to.arrival = to.arrival.max(out);
        }
    }
}

/// Add to `left` and `right` what `generators` put out, each at the gains
/// of its channel. `made` is as long as the block and holds one
/// generator's output at a time.
fn generate(
    generators: &mut [Generator],
    made: (&mut [f32], &mut [f32]),
    left: &mut [f32],
    right: &mut [f32],
    spent: &mut Spent,
) {
    let (made_left, made_right) = made;
    for hosted in generators {
        made_left.fill(0.0);
        made_right.fill(0.0);
        let channel = hosted.channel;
        spent.time(
            || Part::Generator(channel),
            || hosted.generator.process(made_left, made_right),
        );
        for (out, v) in left.iter_mut().zip(&*made_left) {
            *out += v * hosted.gains.0;
        }
        for (out, v) in right.iter_mut().zip(&*made_right) {
            *out += v * hosted.gains.1;
        }
    }
}

/// The fewest frames a block has for its strips to be mixed on the worker
/// threads.
const PARALLEL_FRAMES: usize = 64;

/// The strips of a run, each with the generators that play into it.
struct Pieces<'a> {
    strips: std::slice::IterMut<'a, Strip>,
    /// The generators of the strips not handed out yet, in strip order.
    generators: &'a mut [Generator],
}

impl<'a> Iterator for Pieces<'a> {
    type Item = (&'a mut Strip, &'a mut [Generator]);

    fn next(&mut self) -> Option<Self::Item> {
        let strip = self.strips.next()?;
        let generators = std::mem::take(&mut self.generators);
        let (mine, rest) = generators.split_at_mut(strip.generators.1.min(generators.len()));
        self.generators = rest;
        Some((strip, mine))
    }
}

/// Bring into the bus of the strip at `at` what comes to it before its own
/// plugins run: what the strips before it send and their plugins' second
/// outputs, in the order of their ranks and each sender's second outputs
/// first, then its voices. Voices whose sound is read late go to the
/// strip's dry buffer instead, for [`mix_strip`] to put through its line.
fn gather(strips: &mut [Strip], at: usize, voices: &mut [Voice], len: usize) {
    let (senders, rest) = strips.split_at_mut(at);
    let strip = &mut rest[0];
    for &from in &strip.incoming {
        let sender = &mut senders[from];
        let (to_left, to_right) = (&mut strip.left[..len], &mut strip.right[..len]);
        // The second outputs come in as their slots ran, which was before
        // the sender's sends.
        for aux in sender
            .effects
            .iter_mut()
            .filter_map(|hosted| hosted.aux.as_mut())
        {
            if aux.dest != at {
                continue;
            }
            if !aux.tap.direct() {
                aux.line.add(&mut aux.tap, 1.0, to_left, to_right);
                continue;
            }
            for (out, v) in to_left.iter_mut().zip(&aux.left[..len]) {
                *out += v;
            }
            for (out, v) in to_right.iter_mut().zip(&aux.right[..len]) {
                *out += v;
            }
        }
        let Strip {
            sends,
            sent,
            left: bus_left,
            right: bus_right,
            ..
        } = sender;
        let (bus_left, bus_right) = (&bus_left[..len], &bus_right[..len]);
        for send in sends.iter_mut().filter(|send| send.dest == at) {
            // The sidechain input is read where the send is, through a tap
            // of its own so that a fade moves on once.
            if let Some(side) = send.side.and_then(|side| strip.side.get_mut(side)) {
                let (side_left, side_right) = (&mut side.left[..len], &mut side.right[..len]);
                if send.tap.direct() {
                    side_left.copy_from_slice(bus_left);
                    side_right.copy_from_slice(bus_right);
                } else {
                    let mut tap = send.tap;
                    sent.add(&mut tap, 1.0, side_left, side_right);
                }
            }
            if !send.tap.direct() {
                sent.add(&mut send.tap, send.gain, to_left, to_right);
                continue;
            }
            for (out, v) in to_left.iter_mut().zip(bus_left) {
                *out += v * send.gain;
            }
            for (out, v) in to_right.iter_mut().zip(bus_right) {
                *out += v * send.gain;
            }
        }
    }
    // What was sent here is in the bus already. The voices go on top in
    // the order they started, and the generators after them, by way of
    // their line when the sends can come in late.
    let playing = voices.iter_mut().filter(|v| v.insert == strip.index);
    if strip.played.reach() == 0 {
        let (bus_left, bus_right) = (&mut strip.left[..len], &mut strip.right[..len]);
        for voice in playing {
            voice.render(bus_left, bus_right);
        }
    } else {
        let (dry_left, dry_right) = (&mut strip.dry_left[..len], &mut strip.dry_right[..len]);
        dry_left.fill(0.0);
        dry_right.fill(0.0);
        for voice in playing {
            voice.render(dry_left, dry_right);
        }
    }
}

/// Mix a block of a strip whose bus [`gather`] has filled: add its
/// generators, run the bus through its slots, put its volume and pan on and
/// keep what it puts out for the strips it sends to. Nothing here reaches
/// outside the strip, so the strips of a run can be mixed side by side.
fn mix_strip(strip: &mut Strip, generators: &mut [Generator], len: usize) {
    let (bus_left, bus_right) = (&mut strip.left[..len], &mut strip.right[..len]);
    let (dry_left, dry_right) = (&mut strip.dry_left[..len], &mut strip.dry_right[..len]);
    let made = (&mut strip.made_left[..len], &mut strip.made_right[..len]);
    let spent = &mut strip.spent;
    if strip.played.reach() == 0 {
        generate(generators, made, bus_left, bus_right, spent);
    } else {
        generate(generators, made, dry_left, dry_right, spent);
        strip.played.write(dry_left, dry_right);
        strip
            .played
            .add(&mut strip.played_tap, 1.0, bus_left, bus_right);
    }
    if !strip.enabled {
        return;
    }
    for slot in &mut strip.effects {
        let dry = (&mut *dry_left, &mut *dry_right);
        let part = Part::Slot {
            insert: strip.index,
            slot: slot.slot,
        };
        let run = spent.time(|| part, || slot.play(&strip.side, bus_left, bus_right, dry));
        // The plugin's second output goes into the bus of its insert, with
        // nothing for the frames the plugin sat out. [`gather`] adds it.
        let Some(aux) = &mut slot.aux else {
            continue;
        };
        let (aux_left, aux_right) = (&mut aux.left[..len], &mut aux.right[..len]);
        aux_left.fill(0.0);
        aux_right.fill(0.0);
        if let Some((made_left, made_right)) = slot.effect.aux_output() {
            let made = run.min(made_left.len()).min(made_right.len());
            aux_left[..made].copy_from_slice(&made_left[..made]);
            aux_right[..made].copy_from_slice(&made_right[..made]);
        }
        aux.line.write(aux_left, aux_right);
    }
    if strip.eq.running() {
        strip.eq.process(bus_left, bus_right);
    }
    // FL's insert gain matrix (`FUN_0127b050`, applied by `0x6a13a0`):
    // polarity turns the gain round, and the swap trades the outputs, so
    // the left output gets what the right one would have.
    let volume = if strip.polarity {
        -strip.volume
    } else {
        strip.volume
    };
    let (pan, cross) = match strip.swap {
        false => (strip.pan, strip.cross),
        true => ((strip.cross.1, strip.cross.0), (strip.pan.1, strip.pan.0)),
    };
    let (gain_left, gain_right) = (volume * pan.0, volume * pan.1);
    if cross == (0.0, 0.0) {
        bus_left.iter_mut().for_each(|v| *v *= gain_left);
        bus_right.iter_mut().for_each(|v| *v *= gain_right);
    } else {
        let from_right = volume * cross.0;
        let from_left = volume * cross.1;
        for (l, r) in bus_left.iter_mut().zip(bus_right.iter_mut()) {
            let (left, right) = (*l, *r);
            *l = left * gain_left + right * from_right;
            *r = right * gain_right + left * from_left;
        }
    }
    strip.peak = (
        raised(strip.peak.0, bus_left),
        raised(strip.peak.1, bus_right),
    );
    strip.sent.write(bus_left, bus_right);
}

/// A peak, lifted to the loudest sample of a block when that is louder.
fn raised(peak: f32, block: &[f32]) -> f32 {
    block.iter().fold(peak, |max, v| max.max(v.abs()))
}

/// An insert's place in the walk over the routes.
#[derive(Clone, Copy, PartialEq)]
enum Visit {
    /// Its sends are being followed.
    Open,
    Done,
}

/// The routes of the mixer, each insert listed after every insert that
/// sends to it.
struct Routes {
    /// Insert numbers and their destinations, senders first.
    order: Vec<(u16, Vec<u16>)>,
}

impl Routes {
    /// Walk the sends of every insert the project has and of every insert
    /// in `played`. A send that leads back to an insert on the way to it is
    /// a cycle, whatever its level.
    fn new(project: &Project, played: impl Iterator<Item = u16>) -> Result<Self, Error> {
        let inserts = (0..project.inserts.len()).filter_map(|p| project.insert_index(p));
        let mut seen = BTreeMap::new();
        let mut order = Vec::new();
        for index in inserts.chain(played) {
            visit(project, index, &mut seen, &mut order)?;
        }
        // The walk lists an insert once all its destinations are listed.
        order.reverse();
        Ok(Routes { order })
    }
}

fn visit(
    project: &Project,
    index: u16,
    seen: &mut BTreeMap<u16, Visit>,
    order: &mut Vec<(u16, Vec<u16>)>,
) -> Result<(), Error> {
    match seen.get(&index) {
        Some(Visit::Done) => return Ok(()),
        Some(Visit::Open) => return Err(Error::RouteCycle { insert: index }),
        None => {}
    }
    seen.insert(index, Visit::Open);
    let destinations = project.insert_mix(index).destinations();
    for dest in &destinations {
        visit(project, *dest, seen, order)?;
    }
    seen.insert(index, Visit::Done);
    order.push((index, destinations));
    Ok(())
}

/// The inserts of one project that sound can reach, senders first, with
/// their gains worked out and a bus each. Nothing here allocates once it is
/// built.
///
/// An insert's bus is its voices, what the generators of its plugin
/// channels put out at their channels' volume and pan, and what other
/// inserts send to it. The bus runs through the insert's effect slots in slot order, the insert's
/// volume and pan go on top, and the result is added to every destination
/// at that route's send level. An insert that is switched off passes
/// nothing on. The output is the master's bus with the main volume on it.
/// What reaches an insert that sends nowhere is not heard.
///
/// A slot does something when [`plugins::by_name`] has its plugin and the
/// plugin takes the slot's state. Such a slot puts out the plugin's output
/// and its own input, mixed by the slot's mix level, or its input alone
/// when it is switched off. Every other slot passes its input on.
///
/// A plugin that is late, as Fruity Compressor is by the 64 frames its
/// limiter looks ahead, makes its insert late, and the inserts that insert
/// sends to. Every other way to the master is held back to match, which is
/// FL's plugin delay compensation. An insert is as late as the plugins in
/// its slots that are switched on add up to. Sound gets to an insert's bus
/// when the latest insert that sends there has it out: a send from an
/// earlier one is read that much later off the sender's [`Strip::sent`],
/// and the insert's own voices off its [`Strip::played`]. So everything is
/// in step at the head of every chain, and the output is
/// [`Mixer::latency`] frames behind the clock.
///
/// The lines are as long as the plugins' [`Effect::max_latency`] can ask
/// for. When a slot's switch, a parameter or the tempo makes a plugin late
/// by another count, every tap that has to move fades from where it read
/// to where it reads now over [`FADE`] frames, and a slot with a late
/// plugin fades between its input and its plugin over the same frames
/// when it is switched. A slot whose plugin is on time switches at once.
///
/// A plugin whose state names a sidechain input listens to the insert
/// that [`InsertMix::sidechain_sources`] has at that number: FL counts the
/// inserts that send to the plugin's insert, and a "sidechain" route is one
/// of them with its send level at 0. The plugin is handed what that insert
/// puts out behind its volume and pan, whatever the level of the send, as
/// late as a send from there is read, so in step with the head of its own
/// chain. That it is the sound behind the fader is inferred. An insert
/// with no sound to send is a silent input. A number past the insert's
/// inputs is no input, and [`Mixer::approximated_slots`] counts the slot.
///
/// A plugin's second output, which is Fruity Stereo Shaper's difference,
/// is added to the bus of the insert its number counts to among the
/// inserts its own sends to, in rising order from 1, the last of them for
/// a higher number. It goes in as it leaves the plugin: the send's level,
/// the slot's mix level and the insert's volume and pan are not on it. FL
/// hands the plugin that insert's own buffer (`FUN_0132bb50`). How FL
/// counts to it is inferred from the "Left - Right splitter" preset, whose
/// 2 is then the first insert after the master.
///
/// A generator that is late is not made up for: it comes in with the
/// voices of its insert, as late as it says. None is late so far.
///
/// A plugin with [`Effect::controller_outputs`], which is Fruity Peak
/// Controller, is told every tick of the song by [`Mixer::tick`], which
/// hands back the outputs that moved. An insert with such a plugin that
/// drives a link is in the mixer whether sound reaches it or not, because
/// the plugin's LFO and its base levels drive their controls in silence
/// too. A slot that is switched off, or on an insert that is, hears of no
/// tick: that FL leaves it out is inferred from the plugin putting its LFO
/// back in step when it is switched on.
///
/// The strips sit in runs that send nothing to each other, the strips one
/// step from a strip with no route in first. A block brings into every
/// strip of a run what its senders put out and plays its voices, then
/// mixes the strips of the run side by side on the process's worker
/// threads ([`crate::pool`]): their generators, their slots, their volume
/// and pan. A strip's senders add into its bus in the order the routes
/// were walked in, as they would one strip at a time, so the output is the
/// same to the bit with any number of threads.
///
/// After its slots a strip runs its bus through its three-band EQ
/// ([`InsertEq`]), when a band has a gain, and then through FL's gain
/// matrix: its volume, its pan and its stereo separation
/// ([`Curves::insert_spread`]), with the gain negative when the insert's
/// polarity is reversed and the outputs traded when it swaps left and
/// right. `docs/mixer-insert.md` has where FL does each. FL glides the
/// matrix to a new setting over a block; here it moves at once.
pub(crate) struct Mixer {
    strips: Vec<Strip>,
    /// The master's place in `strips`, when sound can reach it and it is
    /// switched on.
    master: Option<usize>,
    /// Gain of the main volume.
    main: f32,
    /// The tempo the plugins were last told, in BPM.
    tempo: f64,
    /// Whether a block has been mixed. Until one has, a tap moves at once:
    /// there is nothing to fade from.
    started: bool,
    /// The generators of the plugin channels the engine hosts, by the
    /// place of the strip they play into and, for one strip, in the order
    /// their first notes come.
    generators: Vec<Generator>,
    /// Where each generator went in `generators`, by its place in the list
    /// [`Mixer::host`] was handed, which is what a [`Cue`] names.
    cued: Vec<usize>,
    /// The strips in runs that can be mixed side by side, as the first
    /// place and the one past the last: no strip in a run sends to another
    /// in it, so each run needs only the ones before it.
    waves: Vec<(usize, usize)>,
    /// The places of the strips by their [`Strip::rank`].
    ranked: Vec<usize>,
    /// How long each generator and slot took in the engine's last call.
    pub(crate) spent: Spent,
    /// The frames the generators may still sound for with every note let
    /// go: the longest tail of a note off that is not played out.
    tail: usize,
    /// The effect slots of the strips that hold a hosted plugin.
    pub hosted_slots: usize,
    /// The plugins in the other slots of the strips that have one, by
    /// their event 201 name, and the slots each is in.
    pub unhosted_slots: BTreeMap<String, usize>,
    /// The hosted slots whose plugin says another thing stands in for
    /// what its state asks, by what the plugin says of it, and the ones
    /// whose plugin asks for a sidechain input its insert does not have.
    pub approximated_slots: BTreeMap<&'static str, usize>,
    /// The sidechain inputs the strips have: every pair of an insert and
    /// an insert that sends to it which a hosted plugin listens to.
    pub sidechain_routes: usize,
    /// The hosted slots whose plugin is handed a sidechain input.
    pub sidechain_slots: usize,
    /// Whether a hosted plugin is a controller, which hears of the ticks.
    ticking: bool,
}

/// The insert a plugin's second output goes to: the one at `number`,
/// counted from 1, among the `destinations` of its own insert, and the
/// last of them for a higher number. `None` for number 0 and for an insert
/// that sends nowhere.
fn second_output(number: usize, destinations: &[u16]) -> Option<u16> {
    let last = destinations.len().checked_sub(1)?;
    let at = number.checked_sub(1)?;
    Some(destinations[at.min(last)])
}

/// What a plugin is told when the mixer makes it.
#[derive(Clone, Copy)]
pub(crate) struct Told<'a> {
    /// The rate the blocks will come at, in Hz.
    pub sample_rate: u32,
    /// The song's tempo in BPM.
    pub tempo: f64,
    /// The song's ticks to a beat.
    pub ppq: u16,
    /// FL's `Plugins/Fruity/Generators` directory, for a plugin that reads
    /// a file FL ships, when there is such a directory.
    pub plugin_dir: Option<&'a Path>,
}

/// The plugin of a slot with the slot's state in it, when the engine hosts
/// it: [`plugins::by_name`] has it and it takes the state. A slot without
/// a 213 keeps the plugin's defaults.
///
/// `told` is what the plugin is told: the rate, the tempo and the PPQ
/// before it gets its state, and FL's plugin directory after it, which may
/// read files. Without it the plugin keeps its own rate and tempo, which
/// is good enough to ask it about its parameters and its outputs.
fn plugin(slot: &Slot, told: Option<Told<'_>>) -> Option<Box<dyn Effect>> {
    let name = slot.plugin_name.as_deref()?;
    let mut effect = plugins::by_name(name)?;
    if let Some(told) = told {
        effect.set_sample_rate(told.sample_rate as f32);
        effect.set_tempo(told.tempo);
        effect.set_ppq(told.ppq);
    }
    let state = slot.plugin_state.as_deref();
    state.map_or(Ok(()), |state| effect.set_state(state)).ok()?;
    // After the state, so that a plugin reads the file its state names and
    // no other first.
    if let Some(dir) = told.and_then(|told| told.plugin_dir) {
        effect.set_resource_dir(dir);
    }
    Some(effect)
}

/// The plugin in a slot of an insert, as the mixer hosts it. `None` for a
/// slot the mixer passes through or leaves out.
pub(crate) fn hosted(project: &Project, insert: u16, slot: u8) -> Option<Box<dyn Effect>> {
    let strip = project.insert_mix(insert);
    strip.slot_enabled(slot).zip(strip.slot_mix_raw(slot))?;
    let slots = &strip.insert()?.slots;
    let slot = slots.iter().find(|s| s.index == i32::from(slot))?;
    plugin(slot, None)
}

/// The lowest and the highest value a parameter of a plugin takes, when
/// the plugin has the parameter. This leaves the parameter at its highest.
///
/// A plugin keeps a parameter inside the range of its control, so the two
/// ends are what it makes of the least and the most a float can be.
pub(crate) fn param_range(effect: &mut dyn Effect, index: usize) -> Option<(i32, i32)> {
    if index >= effect.param_count() {
        return None;
    }
    effect.set_param(index, f32::MIN);
    let low = effect.param(index);
    effect.set_param(index, f32::MAX);
    Some((low as i32, effect.param(index) as i32))
}

/// The plugins of an insert's slots that the engine hosts, first slot
/// first, told what `told` has. The name of every other plugin in a slot
/// is counted in `unhosted`, and so is a hosted one that turns the slot's
/// state down.
///
/// A slot without a plugin adds nothing, and nor does one outside the ten
/// slots an insert has.
fn effects(
    strip: InsertMix<'_>,
    told: Told<'_>,
    curves: &dyn Curves,
    unhosted: &mut BTreeMap<String, usize>,
) -> Vec<Hosted> {
    let slots = strip.insert().map_or(&[][..], |insert| &insert.slots);
    let mut slots: Vec<_> = slots.iter().collect();
    slots.sort_by_key(|slot| slot.index);
    let mut hosted = Vec::new();
    for slot in slots {
        let Some(name) = slot.plugin_name.as_deref().filter(|name| !name.is_empty()) else {
            continue;
        };
        let index = u8::try_from(slot.index).ok();
        let levels = index.and_then(|slot| strip.slot_enabled(slot).zip(strip.slot_mix_raw(slot)));
        let Some((index, (enabled, mix))) = index.zip(levels) else {
            continue;
        };
        match plugin(slot, Some(told)) {
            Some(effect) => hosted.push(Hosted {
                enabled,
                mix: curves.slot_mix(mix),
                ..Hosted::new(index, effect)
            }),
            None => *unhosted.entry(name.to_owned()).or_default() += 1,
        }
    }
    hosted
}

impl Mixer {
    /// `played` is the loader's number of every insert a voice plays into.
    /// An insert no voice plays into and nothing is sent to stays out,
    /// unless it is one of `controllers`: the inserts with a controller
    /// plugin that drives a link.
    ///
    /// A send at level 0 carries nothing and is left out, unless it is one
    /// of `automated`, the sends whose level an automation clip drives,
    /// each as its sender and its destination, or a plugin's second output
    /// goes the same way, or a plugin at the far end listens to it as its
    /// sidechain input.
    ///
    /// `told` is what the plugins are told: the rate the blocks will come
    /// at, the song's tempo and PPQ, and where FL keeps its plugins.
    pub fn new(
        project: &Project,
        played: &BTreeSet<u16>,
        automated: &BTreeSet<(u16, u16)>,
        controllers: &BTreeSet<u16>,
        told: Told<'_>,
        curves: &dyn Curves,
    ) -> Result<Self, Error> {
        let routes = Routes::new(project, played.union(controllers).copied())?;
        let mut reached: BTreeSet<u16> = played.union(controllers).copied().collect();
        let tempo = told.tempo;
        let mut strips: Vec<Strip> = Vec::new();
        let (mut hosted_slots, mut unhosted_slots) = (0, BTreeMap::new());
        let mut approximated_slots = BTreeMap::new();
        // Sends by insert number, until every strip has its place, and
        // the insert each plugin's second output goes to.
        let mut sends: Vec<Vec<(u16, f32)>> = Vec::new();
        let mut seconds: Vec<Vec<Option<u16>>> = Vec::new();
        // Every route of each strip, heard or not, for the ranks.
        let mut routes_out: Vec<Vec<u16>> = Vec::new();
        for (index, destinations) in routes.order {
            if !reached.contains(&index) {
                continue;
            }
            let strip = project.insert_mix(index);
            let mut to = Vec::new();
            let enabled = strip.enabled();
            let effects = effects(strip, told, curves, &mut unhosted_slots);
            // An insert that is switched off runs no plugin.
            let second = |hosted: &Hosted| {
                let number = hosted.effect.sidechain_output();
                second_output(number, &destinations).filter(|_| enabled)
            };
            let second: Vec<Option<u16>> = effects.iter().map(second).collect();
            routes_out.push(destinations.clone());
            if enabled {
                for dest in destinations {
                    let level = strip.send_level_raw(dest).unwrap_or(0);
                    let send = curves.send_gain(level);
                    let heard = send != 0.0 || automated.contains(&(index, dest));
                    if heard || second.contains(&Some(dest)) {
                        reached.insert(dest);
                        to.push((dest, send));
                    }
                }
            }
            sends.push(to);
            seconds.push(second);
            hosted_slots += effects.len();
            for hosted in &effects {
                if let Fidelity::Approximate(what) = hosted.effect.fidelity() {
                    *approximated_slots.entry(what).or_default() += 1;
                }
            }
            let (pan_raw, separation_raw) = (strip.pan_raw(), strip.separation_raw());
            let (pan, cross) = curves.insert_spread(pan_raw, separation_raw);
            let bands = [0, 1, 2].map(|band| strip.eq_band(band).expect("bands 0 to 2"));
            strips.push(Strip {
                index,
                enabled,
                effects,
                volume: curves.insert_gain(strip.volume_raw()),
                pan,
                cross,
                pan_raw,
                separation_raw,
                polarity: strip.polarity_reversed(),
                swap: strip.swapped(),
                eq: InsertEq::new(bands, told.sample_rate),
                sends: Vec::new(),
                left: vec![0.0; BLOCK],
                right: vec![0.0; BLOCK],
                voiced: played.contains(&index),
                arrival: 0,
                played: Line::default(),
                played_tap: Tap::default(),
                sent: Line::default(),
                side: Vec::new(),
                peak: (0.0, 0.0),
                rank: strips.len(),
                incoming: Vec::new(),
                generators: (0, 0),
                dry_left: vec![0.0; BLOCK],
                dry_right: vec![0.0; BLOCK],
                made_left: vec![0.0; BLOCK],
                made_right: vec![0.0; BLOCK],
                spent: Spent::default(),
            });
        }
        // Each strip goes after every strip on a route to it, as many
        // steps from a strip with no route in as the longest way there,
        // and in walk order among the strips that many steps in: then the
        // strips of one step can be mixed side by side. Every route counts,
        // the ones at level 0 too, since a sidechain input may ride one.
        let walked: BTreeMap<u16, usize> =
            strips.iter().map(|strip| strip.index).zip(0..).collect();
        let mut steps = vec![0; strips.len()];
        for (at, destinations) in routes_out.iter().enumerate() {
            for dest in destinations.iter().filter_map(|dest| walked.get(dest)) {
                steps[*dest] = steps[*dest].max(steps[at] + 1);
            }
        }
        let mut stepped: Vec<_> = steps
            .into_iter()
            .zip(strips)
            .zip(sends.into_iter().zip(seconds))
            .collect();
        stepped.sort_by_key(|((step, _), _)| *step);
        let (strips, (sends, seconds)): (Vec<Strip>, (Vec<_>, Vec<_>)) = stepped
            .into_iter()
            .map(|((_, strip), wired)| (strip, wired))
            .unzip();
        let mut strips = strips;
        let places: BTreeMap<u16, usize> =
            strips.iter().map(|strip| strip.index).zip(0..).collect();
        let place = |index: u16| places.get(&index).copied();
        let master = place(0).filter(|slot| strips[*slot].enabled);
        for ((strip, to), second) in strips.iter_mut().zip(sends).zip(seconds) {
            let placed = to.iter().filter_map(|(dest, gain)| {
                Some(Send {
                    dest: place(*dest)?,
                    gain: *gain,
                    tap: Tap::default(),
                    side: None,
                })
            });
            strip.sends = placed.collect();
            for (hosted, second) in strip.effects.iter_mut().zip(second) {
                hosted.aux = second.and_then(place).map(|dest| Aux {
                    dest,
                    left: vec![0.0; BLOCK],
                    right: vec![0.0; BLOCK],
                    line: Line::default(),
                    tap: Tap::default(),
                });
            }
        }

        // The sidechain inputs. A plugin listens to the insert its number
        // counts to among the inserts that send to its own.
        let (mut sidechain_routes, mut sidechain_slots) = (0, 0);
        for at in 0..strips.len() {
            let asked = |hosted: &Hosted| hosted.effect.sidechain_inputs();
            let asked: Vec<[usize; 2]> = strips[at].effects.iter().map(asked).collect();
            if asked.iter().flatten().all(|number| *number == 0) {
                continue;
            }
            let sources = project.insert_mix(strips[at].index).sidechain_sources();
            let asked = asked.into_iter().enumerate().flat_map(|(slot, numbers)| {
                numbers
                    .into_iter()
                    .enumerate()
                    .map(move |(which, n)| (slot, which, n))
            });
            // A slot counts once, whether it is fed one input or two.
            let (mut fed, mut lacking) = (Vec::new(), Vec::new());
            for (slot, which, number) in asked {
                let Some(at_source) = number.checked_sub(1) else {
                    continue;
                };
                let Some(source) = sources.get(at_source) else {
                    if !lacking.contains(&slot) {
                        lacking.push(slot);
                        *approximated_slots.entry(NO_INPUT).or_default() += 1;
                    }
                    continue;
                };
                if !fed.contains(&slot) {
                    fed.push(slot);
                    sidechain_slots += 1;
                }
                let sides = &mut strips[at].side;
                let known = sides.iter().position(|side| side.number == number);
                let side = known.unwrap_or(sides.len());
                strips[at].effects[slot].side[which] = Some(side);
                if known.is_some() {
                    continue;
                }
                strips[at].side.push(Side {
                    number,
                    left: vec![0.0; BLOCK],
                    right: vec![0.0; BLOCK],
                });
                sidechain_routes += 1;
                // An insert that no sound reaches, or that is switched
                // off, sends nothing: the input stays silent.
                let Some(from) = place(*source).filter(|from| strips[*from].enabled) else {
                    continue;
                };
                let sends = &mut strips[from].sends;
                match sends.iter_mut().find(|send| send.dest == at) {
                    Some(send) => send.side = Some(side),
                    None => sends.push(Send {
                        dest: at,
                        gain: 0.0,
                        tap: Tap::default(),
                        side: Some(side),
                    }),
                }
            }
        }
        let controller = |strip: &Strip| strip.effects.iter().any(|hosted| hosted.ticks);
        let mut mixer = Mixer {
            ticking: strips.iter().any(controller),
            strips,
            master,
            main: curves.insert_gain(project.main_volume_raw()),
            tempo,
            started: false,
            generators: Vec::new(),
            cued: Vec::new(),
            waves: Vec::new(),
            ranked: Vec::new(),
            spent: Spent::default(),
            tail: 0,
            hosted_slots,
            unhosted_slots,
            approximated_slots,
            sidechain_routes,
            sidechain_slots,
        };
        mixer.wire();
        mixer.lay_lines();
        Ok(mixer)
    }

    /// Work out what the strips' places give: who sends to each strip, the
    /// runs of strips that can be mixed side by side, and the strips by
    /// rank. Every destination comes after its sender.
    fn wire(&mut self) {
        let mut ranked: Vec<usize> = (0..self.strips.len()).collect();
        ranked.sort_by_key(|at| self.strips[*at].rank);
        for strip in &mut self.strips {
            strip.incoming.clear();
        }
        for &at in &ranked {
            let strip = &self.strips[at];
            let auxes = strip
                .effects
                .iter()
                .filter_map(|hosted| hosted.aux.as_ref());
            let dests: Vec<usize> = strip
                .sends
                .iter()
                .map(|send| send.dest)
                .chain(auxes.map(|aux| aux.dest))
                .collect();
            for dest in dests {
                assert!(dest > at, "a strip sends to one before it");
                let incoming = &mut self.strips[dest].incoming;
                if !incoming.contains(&at) {
                    incoming.push(at);
                }
            }
        }
        self.waves.clear();
        let mut start = 0;
        for at in 0..self.strips.len() {
            if self.strips[at].incoming.iter().any(|from| *from >= start) {
                self.waves.push((start, at));
                start = at;
            }
        }
        if start < self.strips.len() {
            self.waves.push((start, self.strips.len()));
        }
        self.ranked = ranked;
    }

    /// Give every strip the delay lines its voices, its sends and its
    /// plugins' second outputs can come to need, and aim their taps.
    fn lay_lines(&mut self) {
        arrivals(&mut self.strips, Strip::max_latency);
        let arrival: Vec<usize> = self.strips.iter().map(|strip| strip.arrival).collect();
        for strip in &mut self.strips {
            let reach = strip.sends.iter().map(|send| arrival[send.dest]).max();
            strip.sent = Line::new(reach.unwrap_or(0));
            strip.played = Line::new(strip.arrival);
            let hosted = strip.effects.iter_mut();
            for aux in hosted.filter_map(|hosted| hosted.aux.as_mut()) {
                aux.line = Line::new(arrival[aux.dest]);
            }
        }
        self.align();
    }

    /// Aim every tap so that the sends and the voices of each strip come
    /// in together, with the plugins as late as they are now.
    fn align(&mut self) {
        arrivals(&mut self.strips, Strip::latency);
        let fade = self.started;
        for at in 0..self.strips.len() {
            let (senders, later) = self.strips.split_at_mut(at + 1);
            let strip = &mut senders[at];
            let held = strip.arrival.min(strip.played.reach());
            strip.played_tap.aim(held, fade);
            let out = strip.arrival + strip.latency();
            for send in &mut strip.sends {
                let wait = later[send.dest - at - 1].arrival.saturating_sub(out);
                send.tap.aim(wait.min(strip.sent.reach()), fade);
            }
            // A second output is as late as the slots down to its plugin.
            let mut made = strip.arrival;
            for hosted in &mut strip.effects {
                made += if hosted.enabled { hosted.latency } else { 0 };
                if let Some(aux) = &mut hosted.aux {
                    let wait = later[aux.dest - at - 1].arrival.saturating_sub(made);
                    aux.tap.aim(wait.min(aux.line.reach()), fade);
                }
            }
        }
    }

    /// How many frames behind the clock the output is: how late the
    /// master's bus is with everything that reaches it in step.
    pub fn latency(&self) -> usize {
        self.master.map_or(0, |master| {
            let master = &self.strips[master];
            master.arrival + master.latency()
        })
    }

    /// How many inserts are held back for another one's plugins: the ones
    /// with a send that is read late, and the ones whose voices are.
    pub fn delayed(&self) -> usize {
        let held = |strip: &&Strip| {
            let late = strip.sends.iter().any(|send| send.tap.delay > 0);
            late || strip.voiced && strip.played_tap.delay > 0
        };
        self.strips.iter().filter(held).count()
    }

    /// Whether a voice that plays into this insert has a bus to play into.
    pub fn plays(&self, insert: u16) -> bool {
        self.strips.iter().any(|strip| strip.index == insert)
    }

    /// Take the generators of the song's plugin channels. Each plays into
    /// an insert that was among the `played` the mixer was built with.
    pub fn host(&mut self, generators: Vec<Generator>) {
        let strips = &self.strips;
        let place = |insert: u16| strips.iter().position(|strip| strip.index == insert);
        let mut placed: Vec<(usize, usize, Generator)> = generators
            .into_iter()
            .enumerate()
            .map(|(cued, hosted)| (place(hosted.insert).unwrap_or(usize::MAX), cued, hosted))
            .collect();
        placed.sort_by_key(|(at, cued, _)| (*at, *cued));
        self.cued = vec![0; placed.len()];
        for (now, (_, cued, _)) in placed.iter().enumerate() {
            self.cued[*cued] = now;
        }
        for (at, strip) in self.strips.iter_mut().enumerate() {
            let first = placed.partition_point(|(place, _, _)| *place < at);
            let count = placed[first..]
                .iter()
                .take_while(|(place, _, _)| *place == at)
                .count();
            strip.generators = (first, count);
        }
        self.generators = placed.into_iter().map(|(_, _, hosted)| hosted).collect();
        let controller = |hosted: &Generator| !hosted.generator.controller_outputs().is_empty();
        self.ticking |= self.generators.iter().any(controller);
    }

    /// How many generators the mixer hosts.
    pub fn generators(&self) -> usize {
        self.generators.len()
    }

    /// Hand a generator its note on, its note off or a slide, for the
    /// first frame of the next block.
    pub fn cue(&mut self, cue: &Cue) {
        let at = self.cued.get(cue.generator).copied();
        let Some(hosted) = at.and_then(|at| self.generators.get_mut(at)) else {
            return;
        };
        if hosted.hear(cue) {
            self.tail = self.tail.max(hosted.generator.tail());
        }
    }

    /// Whether a generator follows a clip on its channel's pitch.
    pub fn bending(&self) -> bool {
        let follows = |hosted: &Generator| hosted.bend.as_ref().is_some_and(|b| b.follows);
        self.generators.iter().any(follows)
    }

    /// Tell the generators that follow a clip on their channel's pitch
    /// what `shift` says the clip has moved it by, in cents, at output
    /// sample `at`.
    pub fn bend(&mut self, at: usize, shift: impl Fn(u16) -> f32) {
        for hosted in &mut self.generators {
            if hosted.bend.as_ref().is_some_and(|b| b.follows) {
                hosted.bend(at, shift(hosted.channel));
            }
        }
    }

    /// Whether `shift` moves the pitch of a generator that follows a clip
    /// on its channel's pitch from what it was last told.
    pub fn moves(&self, shift: impl Fn(u16) -> f32) -> bool {
        let moved = |hosted: &Generator| {
            let bend = hosted.bend.as_ref();
            bend.is_some_and(|b| b.follows && b.shift() != shift(hosted.channel))
        };
        self.generators.iter().any(moved)
    }

    /// Let go of every note the generators hold. They play their tails.
    pub fn release(&mut self) {
        for hosted in &mut self.generators {
            hosted.generator.all_notes_off();
            hosted.forget();
            self.tail = self.tail.max(hosted.generator.tail());
        }
    }

    /// Cut every note of the generators where it is.
    pub fn cut(&mut self) {
        for hosted in &mut self.generators {
            hosted.generator.reset();
            hosted.forget();
        }
        self.tail = 0;
    }

    /// Whether a generator may still sound once the notes stop coming:
    /// the tail of a note that was let go is not played out.
    pub fn ringing(&self) -> bool {
        self.tail > 0
    }

    /// Set the left and right gain that a plugin channel's volume and pan
    /// put on its generator, from the next block on. Nothing happens for a
    /// channel without a hosted generator, here and in the one below.
    pub fn set_channel_gains(&mut self, channel: u16, gains: (f32, f32)) {
        let hosted = self.generators.iter_mut();
        for hosted in hosted.filter(|hosted| hosted.channel == channel) {
            hosted.gains = gains;
        }
    }

    /// Set a parameter of a channel's generator, in the plugin's own
    /// units. A parameter that is there already is left alone.
    pub fn set_channel_param(&mut self, channel: u16, index: usize, value: f32) {
        let hosted = self.generators.iter_mut();
        for hosted in hosted.filter(|hosted| hosted.channel == channel) {
            if hosted.generator.param(index) != value {
                hosted.generator.set_param(index, value);
            }
        }
    }

    fn strip(&mut self, insert: u16) -> Option<&mut Strip> {
        self.strips.iter_mut().find(|strip| strip.index == insert)
    }

    /// Set the gain of an insert's volume from the next block on. Nothing
    /// happens for an insert sound cannot reach, here and in the two
    /// below.
    pub fn set_volume(&mut self, insert: u16, gain: f32) {
        if let Some(strip) = self.strip(insert) {
            strip.volume = gain;
        }
    }

    /// Set an insert's pan, as its 225 record stores it, with the
    /// separation it has.
    pub fn set_pan(&mut self, insert: u16, raw: i32, curves: &dyn Curves) {
        if let Some(strip) = self.strip(insert) {
            strip.pan_raw = raw;
            (strip.pan, strip.cross) = curves.insert_spread(raw, strip.separation_raw);
        }
    }

    /// Set an insert's stereo separation, as its 225 record stores it,
    /// with the pan it has.
    pub fn set_separation(&mut self, insert: u16, raw: i32, curves: &dyn Curves) {
        if let Some(strip) = self.strip(insert) {
            strip.separation_raw = raw;
            (strip.pan, strip.cross) = curves.insert_spread(strip.pan_raw, raw);
        }
    }

    /// Set a raw value of an insert's EQ, by its control number less
    /// 0x1FD0: see [`InsertEq::set_control`].
    pub fn set_eq(&mut self, insert: u16, offset: u16, raw: i32) {
        if let Some(strip) = self.strip(insert) {
            strip.eq.set_control(offset, raw);
        }
    }

    /// Set the gain of the send from insert `from` to insert `to`, when
    /// the mixer has that send.
    pub fn set_send(&mut self, from: u16, to: u16, gain: f32) {
        let dest = self.strips.iter().position(|strip| strip.index == to);
        let Some(strip) = self.strip(from) else {
            return;
        };
        let send = strip.sends.iter_mut().find(|send| Some(send.dest) == dest);
        if let Some(send) = send {
            send.gain = gain;
        }
    }

    fn slot(&mut self, insert: u16, slot: u8) -> Option<&mut Hosted> {
        let effects = &mut self.strip(insert)?.effects;
        effects.iter_mut().find(|hosted| hosted.slot == slot)
    }

    /// Switch a slot of an insert on or off from the next block on.
    /// Nothing happens for a slot without a hosted plugin, here and in the
    /// two below.
    ///
    /// A slot whose plugin is late fades to its new place over [`FADE`]
    /// frames, and so do the delays that made up for the plugin.
    pub fn set_slot_enabled(&mut self, insert: u16, slot: u8, enabled: bool) {
        let started = self.started;
        let Some(hosted) = self.slot(insert, slot) else {
            return;
        };
        if hosted.enabled == enabled {
            return;
        }
        hosted.enabled = enabled;
        if hosted.latency > 0 {
            // A fade that is under way turns round where it is. Before
            // the first block there is nothing to fade from.
            hosted.fade = if started { FADE - hosted.fade } else { 0 };
            self.align();
        }
    }

    /// Set how much of a slot's output is its plugin's, from 0 to 1.
    pub fn set_slot_mix(&mut self, insert: u16, slot: u8, mix: f32) {
        if let Some(hosted) = self.slot(insert, slot) {
            hosted.mix = mix;
        }
    }

    /// Set a parameter of a slot's plugin, in the plugin's own units. A
    /// parameter that is there already is left alone, so the plugin starts
    /// no ramp for it. When the parameter makes the plugin late by another
    /// count, the delays that make up for it follow.
    pub fn set_param(&mut self, insert: u16, slot: u8, index: usize, value: f32) {
        let Some(hosted) = self.slot(insert, slot) else {
            return;
        };
        if hosted.effect.param(index) == value {
            return;
        }
        hosted.effect.set_param(index, value);
        let latency = hosted.effect.latency();
        if latency != hosted.latency {
            hosted.latency = latency;
            self.align();
        }
    }

    /// Tell every plugin the tempo in BPM, when it is not the one they
    /// have.
    ///
    /// A plugin may ask for memory here, which is why a mixer is built at
    /// the slowest tempo its song gets to: after that no tempo asks for
    /// more.
    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm == self.tempo {
            return;
        }
        self.tempo = bpm;
        for hosted in &mut self.generators {
            hosted.generator.set_tempo(bpm);
        }
        let mut moved = false;
        for hosted in self.strips.iter_mut().flat_map(|strip| &mut strip.effects) {
            hosted.effect.set_tempo(bpm);
            let latency = hosted.effect.latency();
            moved |= latency != hosted.latency;
            hosted.latency = latency;
        }
        if moved {
            self.align();
        }
    }

    /// Whether a plugin here is a controller. The song's blocks then end
    /// where its ticks do, and [`Mixer::tick`] is called on every one.
    pub fn ticking(&self) -> bool {
        self.ticking
    }

    /// Tell every controller that tick `tick` of the song starts with the
    /// next block, and hand `each` the outputs of the ones that moved one,
    /// with the insert and the slot the plugin is in. A slot that is
    /// switched off is left out, and so is every slot of an insert that
    /// is.
    ///
    /// The generators that are controllers come after them, with the
    /// channel they are on.
    pub fn tick(&mut self, tick: u64, mut each: impl FnMut(Controller, &[f32])) {
        let tempo = self.tempo;
        for &at in &self.ranked {
            let strip = &mut self.strips[at];
            if !strip.enabled {
                continue;
            }
            let controllers = strip.effects.iter_mut();
            for hosted in controllers.filter(|hosted| hosted.ticks && hosted.enabled) {
                hosted.effect.tick(tick, tempo);
                if hosted.effect.controller_changed() {
                    let source = Controller::Slot(strip.index, hosted.slot);
                    each(source, hosted.effect.controller_outputs());
                }
            }
        }
        for hosted in &mut self.generators {
            if hosted.generator.controller_outputs().is_empty() {
                continue;
            }
            hosted.generator.tick(tick, tempo);
            if hosted.generator.controller_changed() {
                let source = Controller::Channel(hosted.channel);
                each(source, hosted.generator.controller_outputs());
            }
        }
    }

    /// Hand `each` the loudest sample every insert has put out since the
    /// last call, on the left and on the right, with the insert's number,
    /// and start again from silence. That is the insert's bus behind its
    /// volume and pan, and for the master in front of the main volume. An
    /// insert that is switched off puts out nothing.
    pub fn take_peaks(&mut self, mut each: impl FnMut(u16, (f32, f32))) {
        for &at in &self.ranked {
            let strip = &mut self.strips[at];
            each(strip.index, std::mem::take(&mut strip.peak));
        }
    }

    /// Mix one block of no more than [`BLOCK`] samples: move every voice
    /// and every generator on by the length of `left` and write what the
    /// master puts out. `left` and `right` are as long as each other.
    pub fn process(&mut self, voices: &mut [Voice], left: &mut [f32], right: &mut [f32]) {
        let len = left.len();
        self.started = true;
        self.tail = self.tail.saturating_sub(len);
        for strip in &mut self.strips {
            strip.left[..len].fill(0.0);
            strip.right[..len].fill(0.0);
            for side in &mut strip.side {
                side.left[..len].fill(0.0);
                side.right[..len].fill(0.0);
            }
        }
        // Too short a block costs more to share out than it saves.
        let pool = (len >= PARALLEL_FRAMES)
            .then(pool::shared)
            .filter(|pool| pool.workers() > 0);
        for &(start, end) in &self.waves {
            for at in start..end {
                gather(&mut self.strips, at, voices, len);
            }
            let first = self.strips[start].generators.0;
            let last = self.strips[start..end]
                .iter()
                .map(|strip| strip.generators.0 + strip.generators.1)
                .max()
                .unwrap_or(first)
                .max(first);
            let generators = &mut self.generators[first..last];
            let strips = &mut self.strips[start..end];
            let busy = |strip: &&mut Strip| strip.generators.1 > 0 || !strip.effects.is_empty();
            match pool {
                Some(pool) if strips.iter_mut().filter(busy).nth(1).is_some() => {
                    let pieces = Mutex::new(Pieces {
                        strips: strips.iter_mut(),
                        generators,
                    });
                    pool.run(&|| {
                        loop {
                            let piece = pieces.lock().unwrap_or_else(|p| p.into_inner()).next();
                            let Some((strip, generators)) = piece else {
                                break;
                            };
                            mix_strip(strip, generators, len);
                        }
                    });
                }
                _ => {
                    for (strip, generators) in (Pieces {
                        strips: strips.iter_mut(),
                        generators,
                    }) {
                        mix_strip(strip, generators, len);
                    }
                }
            }
        }
        #[cfg(feature = "profile")]
        for &at in &self.ranked {
            let parts = &mut self.strips[at].spent.parts;
            self.spent.parts.append(parts);
        }
        match self.master {
            Some(slot) => {
                let master = &self.strips[slot];
                for (out, v) in left.iter_mut().zip(&master.left) {
                    *out = v * self.main;
                }
                for (out, v) in right.iter_mut().zip(&master.right) {
                    *out = v * self.main;
                }
            }
            None => {
                left.fill(0.0 * self.main);
                right.fill(0.0 * self.main);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;
    use plugins::StateError;
    use std::sync::Arc;

    /// The most frames [`Late`] can be late.
    const MOST: usize = 300;

    /// A plugin that is as many frames late as its parameter 0 says, and
    /// does nothing else.
    struct Late {
        frames: usize,
        held: Vec<[f32; 2]>,
        at: usize,
    }

    impl Effect for Late {
        fn set_state(&mut self, _: &[u8]) -> Result<(), StateError> {
            Ok(())
        }

        fn set_sample_rate(&mut self, _: f32) {}

        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            let size = self.held.len();
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                self.held[self.at] = [*l, *r];
                [*l, *r] = self.held[(self.at + size - self.frames) % size];
                self.at = (self.at + 1) % size;
            }
        }

        fn param_count(&self) -> usize {
            1
        }

        fn param(&self, _: usize) -> f32 {
            self.frames as f32
        }

        fn set_param(&mut self, _: usize, value: f32) {
            self.frames = (value as usize).min(MOST);
        }

        fn reset(&mut self) {}

        fn latency(&self) -> usize {
            self.frames
        }

        fn max_latency(&self) -> usize {
            MOST
        }
    }

    /// A strip at unity that sends to the strips at `sends`, with a
    /// [`Late`] of `late` frames in its slot 0 when there is one.
    fn strip(index: u16, late: Option<usize>, sends: &[usize]) -> Strip {
        let flat = |frequency| model::EqBand {
            gain: 0,
            frequency,
            width: 17_500,
        };
        let hosted = |frames: usize| {
            let late = Late {
                frames,
                held: vec![[0.0; 2]; MOST + 1],
                at: 0,
            };
            Hosted::new(0, Box::new(late))
        };
        let send = |dest: &usize| Send {
            dest: *dest,
            gain: 1.0,
            tap: Tap::default(),
            side: None,
        };
        Strip {
            index,
            enabled: true,
            effects: late.map(hosted).into_iter().collect(),
            volume: 1.0,
            pan: (1.0, 1.0),
            cross: (0.0, 0.0),
            pan_raw: 0,
            separation_raw: 0,
            polarity: false,
            swap: false,
            eq: InsertEq::new([flat(5777), flat(33_145), flat(55_825)], 44_100),
            sends: sends.iter().map(send).collect(),
            left: vec![0.0; BLOCK],
            right: vec![0.0; BLOCK],
            voiced: index != 0,
            arrival: 0,
            played: Line::default(),
            played_tap: Tap::default(),
            sent: Line::default(),
            side: Vec::new(),
            peak: (0.0, 0.0),
            rank: 0,
            incoming: Vec::new(),
            generators: (0, 0),
            dry_left: vec![0.0; BLOCK],
            dry_right: vec![0.0; BLOCK],
            made_left: vec![0.0; BLOCK],
            made_right: vec![0.0; BLOCK],
            spent: Spent::default(),
        }
    }

    /// A mixer of these strips, the last of which is the master.
    fn mixer(mut strips: Vec<Strip>) -> Mixer {
        for (rank, strip) in strips.iter_mut().enumerate() {
            strip.rank = rank;
        }
        let mut mixer = Mixer {
            master: Some(strips.len() - 1),
            strips,
            main: 1.0,
            tempo: 120.0,
            started: false,
            generators: Vec::new(),
            cued: Vec::new(),
            waves: Vec::new(),
            ranked: Vec::new(),
            spent: Spent::default(),
            tail: 0,
            hosted_slots: 0,
            unhosted_slots: BTreeMap::new(),
            approximated_slots: BTreeMap::new(),
            sidechain_routes: 0,
            sidechain_slots: 0,
            ticking: false,
        };
        mixer.wire();
        mixer.lay_lines();
        mixer
    }

    /// Mix a block that starts with a click on each of `inserts`, and say
    /// where the output is not silent and what it is there.
    fn clicks(mixer: &mut Mixer, inserts: &[u16]) -> Vec<(usize, f32)> {
        let click = Arc::new(Sample {
            left: vec![1.0],
            right: None,
            rate: 48_000,
        });
        let voice = |insert: &u16| Voice::new(Arc::clone(&click), *insert, None, 1.0, (1.0, 1.0));
        let mut voices: Vec<Voice> = inserts.iter().map(voice).collect();
        let (mut left, mut right) = ([9.0; BLOCK], [9.0; BLOCK]);
        mixer.process(&mut voices, &mut left, &mut right);
        assert_eq!(left, right);
        let loud = left.iter().copied().enumerate();
        loud.filter(|(_, v)| *v != 0.0).collect()
    }

    #[test]
    fn the_delays_follow_a_plugin_whose_latency_moves() {
        // Insert 1 is 100 frames late and insert 2 is on time.
        let mut mixer = mixer(vec![
            strip(1, Some(100), &[2]),
            strip(2, None, &[2]),
            strip(0, None, &[]),
        ]);
        assert_eq!((mixer.latency(), mixer.delayed()), (100, 1));
        // The lines are as long as the plugin can come to ask for.
        let reach = |mixer: &Mixer| [0, 1, 2].map(|at| mixer.strips[at].sent.reach());
        assert_eq!(reach(&mixer), [MOST, MOST, 0]);
        let room = mixer.strips[1].sent.left.capacity();
        assert_eq!(clicks(&mut mixer, &[1, 2]), [(100, 2.0)]);

        // A parameter makes the plugin later, and the other insert follows
        // over the frames of a fade.
        mixer.set_param(1, 0, 0, 300.0);
        assert_eq!((mixer.latency(), mixer.delayed()), (300, 1));
        assert_eq!(clicks(&mut mixer, &[]), []);
        assert_eq!(clicks(&mut mixer, &[1, 2]), [(300, 2.0)]);
        // The master's own voices wait with them.
        assert_eq!(clicks(&mut mixer, &[0, 2]), [(300, 2.0)]);

        // Switched off, the slot makes nothing late.
        mixer.set_slot_enabled(1, 0, false);
        assert_eq!((mixer.latency(), mixer.delayed()), (0, 0));
        assert_eq!(clicks(&mut mixer, &[]), []);
        assert_eq!(clicks(&mut mixer, &[0, 1, 2]), [(0, 3.0)]);
        assert_eq!(reach(&mixer), [MOST, MOST, 0]);
        assert_eq!(mixer.strips[1].sent.left.capacity(), room);
    }

    #[test]
    fn a_send_enters_a_chain_as_late_as_its_own_insert_is() {
        // Insert 1 is 40 frames late and sends to insert 2, which is 60
        // late, and to the master. Insert 3 sends to the master alone.
        let mut mixer = mixer(vec![
            strip(1, Some(40), &[1, 3]),
            strip(2, Some(60), &[3]),
            strip(3, None, &[3]),
            strip(0, None, &[]),
        ]);
        assert_eq!((mixer.latency(), mixer.delayed()), (100, 3));
        // Insert 1 by both ways, insert 2's own voice and insert 3's.
        assert_eq!(clicks(&mut mixer, &[1]), [(100, 2.0)]);
        assert_eq!(clicks(&mut mixer, &[2]), [(100, 1.0)]);
        assert_eq!(clicks(&mut mixer, &[3]), [(100, 1.0)]);
        // Insert 1's send to the master waits for insert 2's 60 frames,
        // and insert 2's voices for insert 1's 40.
        let waits = |at: usize| {
            let strip = &mixer.strips[at];
            let sends: Vec<usize> = strip.sends.iter().map(|send| send.tap.delay).collect();
            (strip.played_tap.delay, sends)
        };
        assert_eq!(waits(0), (0, vec![0, 60]));
        assert_eq!(waits(1), (40, vec![0]));
        assert_eq!(waits(2), (0, vec![100]));
    }

    #[test]
    fn a_second_output_counts_the_inserts_its_own_sends_to() {
        assert_eq!(second_output(0, &[0, 2, 5]), None);
        assert_eq!(second_output(1, &[0, 2, 5]), Some(0));
        assert_eq!(second_output(2, &[0, 2, 5]), Some(2));
        // Past the last of them it is the last.
        assert_eq!(second_output(4, &[0, 2, 5]), Some(5));
        assert_eq!(second_output(1, &[]), None);
    }

    #[test]
    fn a_tap_fades_to_where_it_is_aimed_and_turns_round_on_the_way() {
        let mut line = Line::new(64);
        let mut tap = Tap::default();
        assert!(tap.direct());
        tap.aim(64, false);
        assert_eq!((tap.delay, tap.fade), (64, 0));
        // A ramp through the line, read 64 frames back and then, over the
        // frames of the fade, less and less of that and more of now.
        let ramp: Vec<f32> = (0..BLOCK).map(|n| n as f32).collect();
        line.write(&ramp, &ramp);
        let (mut left, mut right) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
        line.add(&mut tap, 1.0, &mut left, &mut right);
        assert_eq!((left[63], left[64], left[511]), (0.0, 0.0, 447.0));
        tap.aim(0, true);
        line.write(&ramp[..100], &ramp[..100]);
        let (mut left, mut right) = (vec![0.0; 100], vec![0.0; 100]);
        line.add(&mut tap, 2.0, &mut left, &mut right);
        // Frame 99 of the second block is 99 for the tap at 0 and 35 for
        // the tap at 64, and the fade is 100 frames in.
        let want = (35.0 + 64.0 * 100.0 / FADE as f32) * 2.0;
        assert!((left[99] - want).abs() < 1e-3, "{}", left[99]);
        assert_eq!((left, tap.fade), (right, FADE - 100));
        tap.aim(64, true);
        assert_eq!((tap.delay, tap.from, tap.fade), (64, 0, 100));
        tap.aim(32, true);
        assert_eq!((tap.delay, tap.from, tap.fade), (32, 64, FADE));
    }
}
